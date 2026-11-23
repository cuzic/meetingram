//! Session-scoped graph extraction. The history actions are wired in task 6.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use credential_store::CredentialStore;
#[cfg(any(windows, test))]
use discussion_graph::store::{ReviewDecision, ReviewTarget};
use discussion_graph::{
    build_consolidate_groups, build_consolidate_issues, build_incremental, graph_updated_subject,
    parse_ops, store::GraphStore, AppliedOp, Graph, GraphUpdated, IssuePromptStage, Op, ParseError,
    PatchContext, PatchSource, Prompt, DEFAULT_INPUT_LIMIT,
};
use local_broker::LocalBroker;
use recorder_domain::SessionId;
use summarize::{CompletionRequest, Summarizer, SummaryProvider};
use tokio::sync::{broadcast, watch};
use tokio::time::Instant;
use transcript_event::EventEnvelope;

use crate::app_settings::AppSettings;

#[derive(Clone, Copy)]
struct Window {
    utterances: usize,
    elapsed: Duration,
}

impl From<&AppSettings> for Window {
    fn from(settings: &AppSettings) -> Self {
        Self {
            utterances: settings.graph_batch_utterances.unwrap_or(6).max(1),
            elapsed: Duration::from_secs(settings.graph_batch_seconds.unwrap_or(45).max(1)),
        }
    }
}

impl Window {
    fn ready(self, pending: usize, elapsed: Duration) -> bool {
        pending > 0 && (pending >= self.utterances || elapsed >= self.elapsed)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisabledReason {
    Platform,
    Disabled,
    DisabledDuringRun,
    Provider,
    Backend,
}

#[derive(Clone, Debug, Default)]
pub struct GraphStatus {
    pub running: bool,
    pub live_halted: bool,
    pub pending: usize,
    pub disabled_reason: Option<DisabledReason>,
    pub rebuilding: bool,
    pub rev: u64,
    // A rebuild can reuse revision numbers from the deleted graph.
    pub graph_generation: u64,
    pub failure_count: u64,
    pub review_allowed: bool,
}

#[derive(Default)]
struct SessionState {
    status: GraphStatus,
    recording: bool,
}

#[derive(Default)]
struct Session {
    state: Mutex<SessionState>,
    execution: Arc<tokio::sync::Mutex<()>>,
    generation: watch::Sender<u64>,
}

impl Session {
    fn state(&self) -> std::sync::MutexGuard<'_, SessionState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }
}

#[derive(Default)]
struct Sessions {
    active: HashMap<SessionId, Arc<Session>>,
    // Preserve the reason and failure count when releasing the idle executor.
    failures: HashMap<SessionId, (Option<DisabledReason>, u64)>,
    revisions: HashMap<SessionId, (u64, u64)>,
    // Continue retries interrupted work, even if an earlier stage saved a patch.
    interrupted_flushes: BTreeSet<SessionId>,
}

impl Sessions {
    fn prune_idle(&mut self, sid: SessionId) {
        let Some(session) = self.active.get(&sid) else {
            return;
        };
        if Arc::strong_count(session) != 1 {
            return;
        }
        let state = session.state();
        if state.recording {
            return;
        }
        let GraphStatus {
            disabled_reason,
            failure_count,
            ..
        } = state.status;
        if disabled_reason.is_some() || failure_count > 0 {
            self.failures.insert(sid, (disabled_reason, failure_count));
        }
        drop(state);
        self.active.remove(&sid);
    }
}

struct SessionLease {
    consumer: Arc<GraphConsumer>,
    sid: SessionId,
    session: Option<Arc<Session>>,
}

impl std::ops::Deref for SessionLease {
    type Target = Session;

    fn deref(&self) -> &Session {
        self.session.as_deref().expect("session lease is active")
    }
}

impl Drop for SessionLease {
    fn drop(&mut self) {
        let mut sessions = self.consumer.sessions();
        // Release the reference while holding the same lock used to acquire leases.
        drop(self.session.take());
        sessions.prune_idle(self.sid);
    }
}

struct RunGuard<'a> {
    session: &'a Session,
    rebuild: bool,
}

impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        let mut state = self.session.state();
        state.status.running = false;
        if self.rebuild {
            state.status.rebuilding = false;
        }
    }
}

pub struct GraphConsumer {
    broker: LocalBroker,
    database: PathBuf,
    store: Mutex<Option<GraphStore>>,
    llm: Arc<dyn crate::llm_factory::LlmFactory>,
    settings: Arc<Mutex<AppSettings>>,
    sessions: Mutex<Sessions>,
}

pub(crate) struct Backend {
    pub(crate) model: String,
    pub(crate) summarizer: Arc<dyn Summarizer>,
}

#[derive(Clone, Copy)]
enum PostMeeting {
    Continue,
    #[cfg_attr(not(any(windows, test)), allow(dead_code))]
    Retranscribed,
    Rebuild,
}

const FAILURE_LIMIT: usize = 3;
const RETRY_NOTE: &str = "\nReturn the patch as a JSON object with an ops array. Previous error: ";
const INPUT_LIMIT: usize = DEFAULT_INPUT_LIMIT - RETRY_NOTE.len() - 16;

impl GraphConsumer {
    pub fn new(
        broker: LocalBroker,
        database: PathBuf,
        llm: Arc<dyn crate::llm_factory::LlmFactory>,
        settings: Arc<Mutex<AppSettings>>,
    ) -> Self {
        Self {
            broker,
            database,
            store: Mutex::new(None),
            llm,
            settings,
            sessions: Mutex::default(),
        }
    }

    fn sessions(&self) -> std::sync::MutexGuard<'_, Sessions> {
        self.sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn session(self: &Arc<Self>, sid: SessionId) -> SessionLease {
        let mut sessions = self.sessions();
        let previous_failure = sessions.failures.remove(&sid);
        let session = sessions.active.entry(sid).or_default().clone();
        if let Some((reason, count)) = previous_failure {
            let mut state = session.state();
            state.status.disabled_reason = reason;
            state.status.failure_count = count;
        }
        SessionLease {
            consumer: Arc::clone(self),
            sid,
            session: Some(session),
        }
    }

    // After the executor is released, read the pending count via pending_count.
    #[allow(dead_code)] // Read by the graph view in task 6.
    pub fn status(&self, sid: SessionId) -> GraphStatus {
        let sessions = self.sessions();
        let mut status = sessions.active.get(&sid).map_or_else(
            || {
                let (disabled_reason, failure_count) =
                    sessions.failures.get(&sid).copied().unwrap_or_default();
                GraphStatus {
                    disabled_reason,
                    failure_count,
                    ..GraphStatus::default()
                }
            },
            |session| session.state().status.clone(),
        );
        (status.rev, status.graph_generation) =
            sessions.revisions.get(&sid).copied().unwrap_or_default();
        status.review_allowed = sessions
            .active
            .get(&sid)
            .is_none_or(|session| !session.state().recording)
            && !status.rebuilding
            && !status.running
            && !sessions.interrupted_flushes.contains(&sid);
        status
    }

    pub(crate) async fn control_status(
        self: &Arc<Self>,
        sid: SessionId,
        recording: bool,
        configured: bool,
    ) -> control_protocol::GraphStatusDto {
        let status = self.status(sid);
        let enabled = configured && status.disabled_reason.is_none();
        let persisted = if cfg!(windows) {
            self.db(move |store| {
                Ok((
                    store.revision(sid)?,
                    store.pending_count(sid)?,
                    store.consolidation_completed(sid)?.is_some(),
                ))
            })
            .await
            .ok()
        } else {
            None
        };
        let (rev, pending, consolidated) = persisted.unwrap_or((status.rev, status.pending, false));
        control_protocol::GraphStatusDto {
            session_id: sid.to_string(),
            rev,
            enabled,
            running: status.running || status.rebuilding,
            pending: if recording { status.pending } else { pending },
            consolidated,
        }
    }

    #[cfg(any(windows, test))]
    pub async fn set_review(
        self: &Arc<Self>,
        sid: SessionId,
        generation: u64,
        listed_rev: u64,
        target: ReviewTarget<'_>,
        decision: ReviewDecision,
    ) -> Result<(), &'static str> {
        let session = self.session(sid);
        let execution = session.execution.clone().lock_owned().await;
        let status = self.status(sid);
        if generation != status.graph_generation {
            return Err("review_stale");
        }
        let (id, is_node) = match target {
            ReviewTarget::Node(id) => (id.to_owned(), true),
            ReviewTarget::Edge(id) => (id.to_owned(), false),
        };
        let consumer = self.clone();
        self.db(move |store| {
            // Keep the executor reserved even if the UI's future is dropped.
            let _session = session;
            let _execution = execution;
            let target = if is_node {
                ReviewTarget::Node(&id)
            } else {
                ReviewTarget::Edge(&id)
            };
            let rev = store.revision(sid)?;
            if rev != listed_rev {
                consumer.sessions().revisions.entry(sid).or_default().0 = rev;
                return Ok(Err("review_stale"));
            }
            if store.consolidation_completed(sid)?.is_none() {
                return Ok(Err("review_unavailable"));
            }
            if let ReviewTarget::Edge(id) = target {
                let graph = store.load_graph(sid)?;
                let live = graph.edges.get(id).is_some_and(|edge| {
                    [&edge.src, &edge.dst]
                        .into_iter()
                        .all(|id| graph.nodes.get(id).is_some_and(|node| node.is_live()))
                });
                if !live {
                    return Err(discussion_graph::store::StoreError::UnreviewableTarget);
                }
            }
            if !consumer.status(sid).review_allowed {
                return Ok(Err("review_unavailable"));
            }
            let rev = store.set_review(sid, target, decision)?;
            consumer.sessions().revisions.entry(sid).or_default().0 = rev;
            #[cfg(all(windows, feature = "e2e-scripted"))]
            if crate::scripted_stt_active() {
                let decision = match decision {
                    ReviewDecision::Approve => "approve",
                    ReviewDecision::Reject => "reject",
                };
                tracing::info!(decision, "scripted graph review saved");
            }
            Ok(Ok(()))
        })
        .await?
    }

    #[cfg(any(windows, test))]
    pub async fn load_outline(
        self: &Arc<Self>,
        sid: SessionId,
        history: bool,
    ) -> Result<(Graph, bool), &'static str> {
        let consumer = self.clone();
        self.db(move |store| {
            let graph = store.load_graph(sid)?;
            let review_available = history
                && store.consolidation_completed(sid)?.is_some()
                && consumer.status(sid).review_allowed;
            Ok((graph, review_available))
        })
        .await
    }

    #[cfg(test)]
    pub async fn load_graph(self: &Arc<Self>, sid: SessionId) -> Result<Graph, &'static str> {
        self.db(move |store| store.load_graph(sid)).await
    }

    /// Imports finalized rows in a write transaction before counting pending rows.
    /// For a recording session, use `status().pending`.
    pub async fn pending_count(self: &Arc<Self>, sid: SessionId) -> Result<usize, &'static str> {
        self.db(move |store| {
            store.import_utterances(sid)?;
            store.pending_count(sid)
        })
        .await
    }

    #[allow(dead_code)] // Read by the graph view in task 6.
    pub fn review_threshold(&self) -> f64 {
        self.effective_settings()
            .graph_review_threshold
            .unwrap_or(0.6)
    }

    fn effective_settings(&self) -> AppSettings {
        let mut settings = self
            .settings
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        self.llm.graph_settings(&mut settings);
        settings
    }

    fn enabled(windows: bool, settings: &AppSettings) -> Result<(), DisabledReason> {
        if !windows {
            return Err(DisabledReason::Platform);
        }
        if !settings.graph_enabled.unwrap_or(true) {
            return Err(DisabledReason::Disabled);
        }
        Ok(())
    }

    async fn backend(
        self: &Arc<Self>,
        windows: bool,
        settings: AppSettings,
    ) -> Result<Backend, DisabledReason> {
        Self::enabled(windows, &settings)?;
        self.llm.graph(settings).await
    }

    async fn db<T: Send + 'static>(
        self: &Arc<Self>,
        operation: impl FnOnce(&mut GraphStore) -> Result<T, discussion_graph::store::StoreError>
            + Send
            + 'static,
    ) -> Result<T, &'static str> {
        let consumer = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut store = consumer.store.lock().map_err(|_| "db_lock")?;
            if store.is_none() {
                *store = Some(GraphStore::open(&consumer.database).map_err(|_| "db_open")?);
            }
            operation(store.as_mut().ok_or("db_open")?).map_err(|_| "db")
        })
        .await
        .map_err(|_| "db_task")?
    }

    pub fn start_recording(self: &Arc<Self>, sid: SessionId) {
        let consumer = self.clone();
        drop(
            self.start_recording_with(sid, cfg!(windows), move |settings| async move {
                consumer.backend(cfg!(windows), settings).await
            }),
        );
    }

    fn start_recording_with<
        F: std::future::Future<Output = Result<Backend, DisabledReason>> + Send,
    >(
        self: &Arc<Self>,
        sid: SessionId,
        windows: bool,
        load_backend: impl FnOnce(AppSettings) -> F + Send + 'static,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let session = self.session(sid);
        session.state().recording = true;
        let settings = self.effective_settings();
        if let Err(reason) = Self::enabled(windows, &settings) {
            session.state().status.disabled_reason = Some(reason);
            return None;
        }

        // Reserve execution before capture can publish anything or finish.
        let execution = session.execution.clone().try_lock_owned().ok()?;
        let segments = self
            .broker
            .subscribe(&format!("transcription.{sid}.segment.finalized"));
        let stopped = self
            .broker
            .subscribe(&local_broker::session_stopped_subject(sid));
        let consumer = self.clone();
        let generation = session.generation.subscribe();
        Some(tokio::spawn(async move {
            let _execution = execution;
            let window = Window::from(&settings);
            let backend = match load_backend(settings).await {
                Ok(backend) => backend,
                Err(reason) => {
                    session.state().status.disabled_reason = Some(reason);
                    return;
                }
            };
            let mut run = Run::new(&consumer, sid, &session, &backend, generation);
            let _guard = run.mark_running(false);
            if run.live(window, segments, stopped).await {
                run.flush(true).await;
            }
        }))
    }

    pub fn recording_stopped(&self, sid: SessionId) {
        let mut sessions = self.sessions();
        if let Some(session) = sessions.active.get(&sid) {
            session.state().recording = false;
        }
        sessions.prune_idle(sid);
    }

    #[allow(dead_code)] // Called by the graph view in task 6.
    pub fn continue_extraction(self: &Arc<Self>, sid: SessionId) -> bool {
        self.post_meeting(sid, PostMeeting::Continue)
    }

    #[cfg_attr(not(any(windows, test)), allow(dead_code))]
    pub fn retranscribed(self: &Arc<Self>, sid: SessionId) -> bool {
        self.post_meeting(sid, PostMeeting::Retranscribed)
    }

    #[allow(dead_code)] // Called by the graph view in task 6.
    pub fn rebuild(self: &Arc<Self>, sid: SessionId) -> bool {
        self.post_meeting(sid, PostMeeting::Rebuild)
    }

    fn post_meeting(self: &Arc<Self>, sid: SessionId, action: PostMeeting) -> bool {
        let consumer = self.clone();
        self.post_meeting_with(sid, action, cfg!(windows), move |settings| async move {
            consumer.backend(cfg!(windows), settings).await
        })
        .is_some()
    }

    fn post_meeting_with<
        F: std::future::Future<Output = Result<Backend, DisabledReason>> + Send,
    >(
        self: &Arc<Self>,
        sid: SessionId,
        action: PostMeeting,
        windows: bool,
        load_backend: impl FnOnce(AppSettings) -> F + Send + 'static,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let session = self.session(sid);
        let settings = self.effective_settings();
        let rebuild = matches!(action, PostMeeting::Rebuild);
        let generation = {
            let mut state = session.state();
            if state.recording || state.status.rebuilding {
                return None;
            }
            if let Err(reason) = Self::enabled(windows, &settings) {
                state.status.disabled_reason = Some(reason);
                return None;
            }
            if rebuild {
                state.status.rebuilding = true;
            }
            session.generation.subscribe()
        };
        let consumer = self.clone();
        Some(tokio::spawn(async move {
            let backend = match load_backend(settings).await {
                Ok(backend) => backend,
                Err(reason) => {
                    let mut state = session.state();
                    state.status.disabled_reason = Some(reason);
                    state.status.failure_count += 1;
                    if rebuild {
                        state.status.rebuilding = false;
                    }
                    return;
                }
            };
            if rebuild {
                session
                    .generation
                    .send_modify(|generation| *generation += 1);
            }
            let _execution = session.execution.lock().await;
            if !rebuild && generation.has_changed().unwrap_or(true) {
                return;
            }
            let cancel = if rebuild {
                session.generation.subscribe()
            } else {
                generation
            };
            let mut run = Run::new(&consumer, sid, &session, &backend, cancel);
            let _guard = run.mark_running(rebuild);
            match action {
                PostMeeting::Continue => run.flush(false).await,
                PostMeeting::Retranscribed => {
                    if let Err(kind) = run.increment(PatchSource::Flush, true).await {
                        if !matches!(kind, "cancelled" | "disabled") {
                            run.log_failure(PatchSource::Flush, kind);
                        }
                    }
                }
                PostMeeting::Rebuild => {
                    let revisions = consumer.clone();
                    match consumer
                        .db(move |store| {
                            store.delete_session(sid)?;
                            let mut sessions = revisions.sessions();
                            let (rev, generation) = sessions.revisions.entry(sid).or_default();
                            *rev = 0;
                            *generation += 1;
                            Ok(())
                        })
                        .await
                    {
                        Ok(()) => run.flush(true).await,
                        Err(kind) => run.log_failure(PatchSource::Flush, kind),
                    }
                }
            }
        }))
    }
}

pub(crate) fn build_backend(
    credentials: Arc<impl CredentialStore + Send + Sync + 'static>,
    settings: AppSettings,
    cli_temp_base: PathBuf,
    herdr: &crate::llm_factory::HerdrConnections,
) -> Result<(SummaryProvider, Backend), DisabledReason> {
    let (provider, summary_model) = if let Some(key) = settings.graph_provider.as_deref() {
        let provider = SummaryProvider::ALL
            .into_iter()
            .find(|provider| provider.key() == key)
            .ok_or(DisabledReason::Provider)?;
        (provider, None)
    } else {
        let provider = credentials
            .load(
                summarize::CREDENTIAL_SERVICE,
                summarize::SELECTED_PROVIDER_ACCOUNT,
            )
            .ok()
            .map(|key| SummaryProvider::from_key(&key))
            .unwrap_or(SummaryProvider::Claude);
        let model = credentials
            .load(
                summarize::CREDENTIAL_SERVICE,
                summarize::SELECTED_MODEL_ACCOUNT,
            )
            .ok();
        (provider, model)
    };
    if !provider.supports_graph() {
        return Err(DisabledReason::Provider);
    }
    let model = summarize::resolve_model(
        provider,
        summary_model.as_deref(),
        settings.graph_model.as_deref(),
    );
    let summarizer =
        crate::llm_factory::build_selected(provider, credentials, settings, cli_temp_base, herdr)
            .map_err(|_| DisabledReason::Backend)?;
    Ok((provider, Backend { model, summarizer }))
}

struct Run<'a> {
    consumer: &'a Arc<GraphConsumer>,
    sid: SessionId,
    session: &'a Session,
    backend: &'a Backend,
    cancel: watch::Receiver<u64>,
}

impl<'a> Run<'a> {
    fn new(
        consumer: &'a Arc<GraphConsumer>,
        sid: SessionId,
        session: &'a Session,
        backend: &'a Backend,
        cancel: watch::Receiver<u64>,
    ) -> Self {
        Self {
            consumer,
            sid,
            session,
            backend,
            cancel,
        }
    }

    fn mark_running(&self, rebuild: bool) -> RunGuard<'a> {
        let mut state = self.session.state();
        state.status.running = true;
        state.status.disabled_reason = None;
        RunGuard {
            session: self.session,
            rebuild,
        }
    }

    async fn live(
        &mut self,
        window: Window,
        mut segments: broadcast::Receiver<Vec<u8>>,
        mut stopped: broadcast::Receiver<Vec<u8>>,
    ) -> bool {
        let mut last = Instant::now();
        let mut deadline = None;
        let mut failures = 0;
        loop {
            tokio::select! {
                biased;
                event = stopped.recv() => return !matches!(event, Err(broadcast::error::RecvError::Closed)),
                _ = self.cancel.changed() => return false,
                event = segments.recv() => {
                    if matches!(event, Err(broadcast::error::RecvError::Closed)) {
                        return false;
                    }
                }
                _ = async {
                    match deadline {
                        Some(at) => tokio::time::sleep_until(at).await,
                        None => std::future::pending().await,
                    }
                } => {}
            }

            let result = self.import().await;
            match stopped.try_recv() {
                Ok(_) | Err(broadcast::error::TryRecvError::Lagged(_)) => return true,
                Err(broadcast::error::TryRecvError::Closed) => return false,
                Err(broadcast::error::TryRecvError::Empty) => {}
            }
            let pending = result.as_ref().copied().unwrap_or_default();
            if failures >= FAILURE_LIMIT {
                deadline = None;
                continue;
            }

            if result.is_err() || window.ready(pending, last.elapsed()) {
                let outcome = match result {
                    Ok(_) => self.extract(PatchSource::Live, false).await,
                    Err(kind) => Err(kind),
                };
                match outcome {
                    Ok(_) => failures = 0,
                    Err("cancelled" | "disabled") => return false,
                    Err(kind) => {
                        failures += 1;
                        self.log_failure(PatchSource::Live, kind);
                    }
                }
                last = Instant::now();
            }
            let mut state = self.session.state();
            state.status.live_halted = failures >= FAILURE_LIMIT;
            deadline = if state.status.pending == 0 || failures >= FAILURE_LIMIT {
                None
            } else if failures == 0 && state.status.pending >= window.utterances {
                Some(last)
            } else {
                last.checked_add(window.elapsed)
            };
        }
    }

    async fn import(&self) -> Result<usize, &'static str> {
        let sid = self.sid;
        let count = self.consumer.pending_count(sid).await?;
        self.session.state().status.pending = count;
        Ok(count)
    }

    async fn increment(
        &mut self,
        source: PatchSource,
        retranscribed: bool,
    ) -> Result<bool, &'static str> {
        self.import().await?;
        self.extract(source, retranscribed).await
    }

    async fn extract(
        &mut self,
        source: PatchSource,
        retranscribed: bool,
    ) -> Result<bool, &'static str> {
        let sid = self.sid;
        let prompt = self
            .consumer
            .db(move |store| {
                let pending = store.pending_utterances(sid)?;
                let pending: Vec<_> = pending
                    .iter()
                    .filter(|row| !retranscribed || row.is_retranscribed)
                    .collect();
                let Some(first) = pending.first() else {
                    return Ok(Ok(None));
                };
                let is_retranscribed = first.is_retranscribed;
                let ids: Vec<_> = pending
                    .iter()
                    .take_while(|row| row.is_retranscribed == is_retranscribed)
                    .map(|row| row.uid.as_str())
                    .collect();
                let graph = store.load_graph(sid)?;
                let recent = recent_nodes(&graph);
                Ok(
                    build_incremental(&graph, &ids, &recent, is_retranscribed, INPUT_LIMIT)
                        .map(Some),
                )
            })
            .await?
            .map_err(|_| "input_limit")?;
        if let Some(prompt) = prompt {
            if prompt.new_uids.is_empty() {
                return Err("input_limit");
            }
            self.apply(source, prompt, PatchContext::Other).await?;
            let count = self
                .consumer
                .db(move |store| store.pending_count(sid))
                .await?;
            self.session.state().status.pending = count;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn flush(&mut self, consolidate_without_new_patch: bool) {
        let interrupted = !self
            .consumer
            .sessions()
            .interrupted_flushes
            .insert(self.sid);
        let mut failures = 0;
        loop {
            match self.increment(PatchSource::Flush, false).await {
                Ok(false) => break,
                Ok(true) => failures = 0,
                Err("cancelled" | "disabled") => return,
                Err(kind) => {
                    self.log_failure(PatchSource::Flush, kind);
                    failures += 1;
                    if failures >= FAILURE_LIMIT {
                        return;
                    }
                }
            }
        }

        if !consolidate_without_new_patch && !interrupted {
            let sid = self.sid;
            match self
                .consumer
                .db(move |store| {
                    Ok(store.consolidation_completed(sid)?.is_some()
                        && store.is_consolidated(sid)?)
                })
                .await
            {
                Ok(true) => {
                    self.consumer
                        .sessions()
                        .interrupted_flushes
                        .remove(&self.sid);
                    return;
                }
                Ok(false) => {}
                Err(kind) => {
                    self.log_failure(PatchSource::Consolidate, kind);
                    return;
                }
            }
        }
        match self.consolidate().await {
            Ok(()) => {
                self.consumer
                    .sessions()
                    .interrupted_flushes
                    .remove(&self.sid);
            }
            Err("cancelled" | "disabled") => {}
            Err(kind) => self.log_failure(PatchSource::Consolidate, kind),
        }
    }

    async fn consolidate(&mut self) -> Result<(), &'static str> {
        let sid = self.sid;
        let graph = self.consumer.db(move |store| store.load_graph(sid)).await?;
        if !graph.nodes.values().any(|node| node.is_live()) {
            return Ok(());
        }

        let issues = build_consolidate_issues(&graph, INPUT_LIMIT).map_err(|_| "input_limit")?;
        let graph = if let Some(prompt) = issues {
            if prompt.omitted_issues > 0 {
                tracing::debug!(
                    omitted = prompt.omitted_issues,
                    "issues omitted from consolidation"
                );
            }
            self.apply(
                PatchSource::Consolidate,
                prompt,
                PatchContext::IssueConsolidation,
            )
            .await?;
            self.consumer.db(move |store| store.load_graph(sid)).await?
        } else {
            graph
        };

        let (groups, omitted) = build_consolidate_groups(&graph, INPUT_LIMIT);
        if omitted > 0 {
            tracing::debug!(omitted, "nodes omitted from consolidation");
        }
        for prompt in groups {
            self.apply(PatchSource::Consolidate, prompt, PatchContext::Other)
                .await?;
        }
        self.consumer
            .db(move |store| store.mark_consolidation_completed(sid))
            .await
    }

    async fn apply(
        &mut self,
        source: PatchSource,
        prompt: Prompt,
        context: PatchContext,
    ) -> Result<(), &'static str> {
        let started = Instant::now();
        let timeout = if source == PatchSource::Consolidate {
            120
        } else {
            60
        };
        let mut req = CompletionRequest::new(
            prompt.system,
            prompt.user,
            self.backend.model.clone(),
            Duration::from_secs(timeout),
        );
        let ops = match self.complete(&req, prompt.issue_stage).await? {
            Ok(ops) => ops,
            Err(error) => {
                let kind = parse_kind(error);
                self.log_failure(source, kind);
                req.user.push_str(RETRY_NOTE);
                req.user.push_str(kind);
                self.complete(&req, prompt.issue_stage)
                    .await?
                    .map_err(parse_kind)?
            }
        };
        if self.cancel.has_changed().unwrap_or(true) {
            return Err("cancelled");
        }

        let sid = self.sid;
        let model = self.backend.model.clone();
        let consumer = self.consumer.clone();
        let result = self
            .consumer
            .db(move |store| {
                let input: Vec<_> = prompt.new_uids.iter().map(String::as_str).collect();
                let references: Vec<_> = prompt
                    .allowed_uids
                    .iter()
                    .map(String::as_str)
                    .filter(|uid| !input.contains(uid))
                    .collect();
                let result = if context == PatchContext::IssueConsolidation {
                    store.apply_patch_with_context(
                        sid,
                        source,
                        Some(&model),
                        &ops,
                        &input,
                        &references,
                        context,
                    )
                } else {
                    store.apply_patch(sid, source, Some(&model), &ops, &input, &references)
                }?;
                consumer.sessions().revisions.entry(sid).or_default().0 = result.graph.rev;
                Ok(result)
            })
            .await?;
        let added = result
            .applied
            .iter()
            .filter(|op| matches!(op, AppliedOp::AddNode { .. } | AppliedOp::AddEdge { .. }))
            .count();
        let merged = result
            .applied
            .iter()
            .filter(|op| matches!(op, AppliedOp::Merge { .. }))
            .count();
        let rejected = result.rejected.values().sum();
        let rev = result.graph.rev;
        tracing::debug!(session_id = %sid, rev, ?source, operations = result.applied.len(),
            rejected, elapsed_ms = started.elapsed().as_millis(), "graph patch saved");
        let event = EventEnvelope::new(
            sid,
            GraphUpdated {
                rev,
                source,
                added,
                merged,
                rejected,
            },
        );
        if let Err(error) = self
            .consumer
            .broker
            .publish(&graph_updated_subject(sid), &event)
        {
            if !matches!(error, local_broker::BrokerError::NoSubscribers(_)) {
                self.log_failure(source, "publish");
            }
        }
        Ok(())
    }

    async fn complete(
        &mut self,
        req: &CompletionRequest,
        issue_stage: Option<IssuePromptStage>,
    ) -> Result<Result<Vec<Op>, ParseError>, &'static str> {
        let response = tokio::select! {
            biased;
            _ = self.cancel.changed() => return Err("cancelled"),
            result = async {
                let enabled = self.consumer.settings.lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .graph_enabled
                    .unwrap_or(true);
                if !enabled {
                    self.session.state().status.disabled_reason = Some(DisabledReason::DisabledDuringRun);
                    return Err("disabled");
                }
                if let Some(issue_stage) = issue_stage {
                    tracing::debug!(session_id = %self.sid, ?issue_stage, "graph issue consolidation request");
                }
                tokio::time::timeout(req.timeout, self.backend.summarizer.complete(req))
                    .await
                    .map_err(|_| "timeout")?
                    .map_err(|error| error.kind())
            } => result?,
        };
        Ok(parse_ops(&response))
    }

    fn log_failure(&self, source: PatchSource, kind: &'static str) {
        self.session.state().status.failure_count += 1;
        tracing::warn!(session_id = %self.sid, ?source, kind, "graph operation failed");
    }
}

fn parse_kind(error: ParseError) -> &'static str {
    match error {
        ParseError::NoObject => "no_object",
        ParseError::InvalidJson => "invalid_json",
        ParseError::InvalidOps => "invalid_ops",
        _ => "parse",
    }
}

fn recent_nodes(graph: &Graph) -> Vec<BTreeSet<String>> {
    let mut recent = BTreeMap::<u64, BTreeSet<String>>::new();
    for node in graph.nodes.values() {
        if node.updated_rev > graph.rev.saturating_sub(10) {
            recent
                .entry(node.updated_rev)
                .or_default()
                .insert(node.nid.clone());
        }
    }
    for edge in graph.edges.values() {
        if edge.created_rev > graph.rev.saturating_sub(10) {
            let touched = recent.entry(edge.created_rev).or_default();
            touched.insert(edge.src.clone());
            touched.insert(edge.dst.clone());
        }
    }
    recent.into_values().collect()
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_window_requires_pending_rows_and_either_threshold() {
        let window = Window::from(&AppSettings::default());
        assert!(!window.ready(0, Duration::from_secs(45)));
        assert!(!window.ready(5, Duration::from_secs(44)));
        assert!(window.ready(6, Duration::from_secs(0)));
        assert!(window.ready(1, Duration::from_secs(45)));
    }

    use std::io::Write;
    use std::ops::Range;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use chrono::Utc;
    use recorder_domain::{
        AudioManifest, CaptureManifest, ConsentManifest, RemoteSourceKind, SessionManifest,
        TrackKind,
    };
    use session_store::{SessionStore, TranscriptSegment};
    use summarize::{SummarizeOptions, SummarizerError, TranscriptTurn};
    use tokio::sync::{mpsc, Semaphore};

    struct Fixture {
        _dir: tempfile::TempDir,
        consumer: Arc<GraphConsumer>,
        sid: SessionId,
        store: SessionStore,
        updates: broadcast::Receiver<Vec<u8>>,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("test.sqlite");
            let store = SessionStore::open(&path).unwrap();
            let sid = SessionId::new();
            let now = Utc::now();
            store
                .create_session(&SessionManifest {
                    schema_version: 1,
                    session_id: sid,
                    started_at: now,
                    ended_at: None,
                    platform: "test".into(),
                    app_version: "0".into(),
                    capture: CaptureManifest {
                        microphone_device_id: "synthetic-mic".into(),
                        remote_source_id: "synthetic-output".into(),
                        remote_source_kind: RemoteSourceKind::EndpointLoopback,
                    },
                    audio: AudioManifest {
                        sample_rate: 48_000,
                        segment_duration_ms: 30_000,
                        tracks: vec![TrackKind::SelfMic],
                    },
                    consent: ConsentManifest {
                        confirmed_by_user: true,
                        confirmed_at: now,
                    },
                })
                .unwrap();
            let broker = LocalBroker::new();
            let updates = broker.subscribe(&graph_updated_subject(sid));
            let credentials = Arc::new(
                credential_store::FallbackCredentialStore::new(dir.path().join("credentials"))
                    .unwrap(),
            );
            let consumer = Arc::new(GraphConsumer::new(
                broker,
                path,
                Arc::new(crate::llm_factory::ProductionLlm {
                    credentials,
                    cli_temp_base: dir.path().join("cli"),
                    herdr: Default::default(),
                }),
                Arc::new(Mutex::new(AppSettings::default())),
            ));
            Self {
                _dir: dir,
                consumer,
                sid,
                store,
                updates,
            }
        }

        fn reopen_consumer(&self) -> Arc<GraphConsumer> {
            Arc::new(GraphConsumer::new(
                LocalBroker::new(),
                self.consumer.database.clone(),
                self.consumer.llm.clone(),
                self.consumer.settings.clone(),
            ))
        }

        fn append(&self, text: &str, retranscribed: bool) {
            self.store
                .insert_transcript_segment(&TranscriptSegment {
                    session_id: self.sid,
                    track: Some(TrackKind::SelfMic),
                    speaker: None,
                    text: text.into(),
                    start_ms: None,
                    end_ms: None,
                    is_final: true,
                    is_retranscribed: retranscribed,
                })
                .unwrap();
        }

        fn finalized(&self) {
            self.consumer
                .broker
                .publish_bytes(
                    &format!("transcription.{}.segment.finalized", self.sid),
                    Vec::new(),
                )
                .unwrap();
        }

        fn stopped(&self) {
            self.consumer
                .broker
                .publish_bytes(&local_broker::session_stopped_subject(self.sid), Vec::new())
                .unwrap();
            self.consumer.recording_stopped(self.sid);
        }

        fn start(&self, fake: Arc<Fake>) -> tokio::task::JoinHandle<()> {
            self.consumer
                .start_recording_with(self.sid, true, |settings| async move {
                    Ok(fake.backend(&settings))
                })
                .unwrap()
        }

        fn post(&self, action: PostMeeting, fake: Arc<Fake>) -> tokio::task::JoinHandle<()> {
            self.consumer
                .post_meeting_with(self.sid, action, true, |settings| async move {
                    Ok(fake.backend(&settings))
                })
                .unwrap()
        }

        fn events(&mut self) -> Vec<GraphUpdated> {
            std::iter::from_fn(|| self.updates.try_recv().ok())
                .map(|bytes| {
                    serde_json::from_slice::<EventEnvelope<GraphUpdated>>(&bytes)
                        .unwrap()
                        .body
                })
                .collect()
        }

        async fn wait_for_status(&self, predicate: impl Fn(&GraphStatus) -> bool) {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if predicate(&self.consumer.status(self.sid)) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .unwrap();
        }
    }

    const EMPTY_PATCH: &str = r#"{"ops":[]}"#;
    const ISSUE_PATCH: &str = r#"{"ops":[{"op":"add_node","tmp":"issue","kind":"issue","text":"Synthetic issue","confidence":1,"grounds":[{"uid":"u1"}]}]}"#;

    struct Fake {
        calls: mpsc::UnboundedSender<CompletionRequest>,
        provider_failures: Range<usize>,
        responses: Vec<&'static str>,
        first_block: Option<Semaphore>,
        disable_on_call: Option<(usize, Arc<Mutex<AppSettings>>)>,
        active: AtomicUsize,
        count: AtomicUsize,
        max_active: AtomicUsize,
    }

    impl Fake {
        fn new(
            failed: bool,
            block_first: bool,
        ) -> (Arc<Self>, mpsc::UnboundedReceiver<CompletionRequest>) {
            Self::scripted(
                0..if failed { usize::MAX } else { 0 },
                Vec::new(),
                block_first,
            )
        }

        fn scripted(
            provider_failures: Range<usize>,
            responses: Vec<&'static str>,
            block_first: bool,
        ) -> (Arc<Self>, mpsc::UnboundedReceiver<CompletionRequest>) {
            let (calls, receiver) = mpsc::unbounded_channel();
            let fake = Self {
                calls,
                provider_failures,
                responses,
                first_block: block_first.then(|| Semaphore::new(0)),
                disable_on_call: None,
                active: AtomicUsize::new(0),
                count: AtomicUsize::new(0),
                max_active: AtomicUsize::new(0),
            };
            (Arc::new(fake), receiver)
        }

        fn backend(self: Arc<Self>, settings: &AppSettings) -> Backend {
            Backend {
                model: summarize::resolve_model(
                    SummaryProvider::Claude,
                    Some("summary-model"),
                    settings.graph_model.as_deref(),
                ),
                summarizer: Arc::new(FakeSummarizer(self)),
            }
        }
    }

    struct ActiveCall<'a>(&'a AtomicUsize);

    impl Drop for ActiveCall<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    struct FakeSummarizer(Arc<Fake>);

    #[async_trait::async_trait]
    impl Summarizer for FakeSummarizer {
        async fn summarize(
            &self,
            _: &[TranscriptTurn],
            _: &SummarizeOptions,
        ) -> Result<String, SummarizerError> {
            unreachable!()
        }

        async fn complete(&self, request: &CompletionRequest) -> Result<String, SummarizerError> {
            let fake = &self.0;
            let active = fake.active.fetch_add(1, Ordering::SeqCst) + 1;
            let _guard = ActiveCall(&fake.active);
            fake.max_active.fetch_max(active, Ordering::SeqCst);
            let count = fake.count.fetch_add(1, Ordering::SeqCst);
            let _ = fake.calls.send(CompletionRequest::new(
                request.system.clone(),
                request.user.clone(),
                request.model.clone(),
                request.timeout,
            ));
            if count == 0 {
                if let Some(block) = &fake.first_block {
                    block.acquire().await.unwrap().forget();
                }
            }
            if let Some((call, settings)) = &fake.disable_on_call {
                if count == *call {
                    settings.lock().unwrap().graph_enabled = Some(false);
                }
            }
            if fake.provider_failures.contains(&count) {
                Err(SummarizerError::CompletionProvider)
            } else {
                let response_index = if count >= fake.provider_failures.end {
                    count - fake.provider_failures.len()
                } else {
                    count
                };
                Ok(fake
                    .responses
                    .get(response_index)
                    .copied()
                    .unwrap_or(EMPTY_PATCH)
                    .into())
            }
        }
    }

    async fn next_call(
        calls: &mut mpsc::UnboundedReceiver<CompletionRequest>,
    ) -> CompletionRequest {
        tokio::time::timeout(Duration::from_secs(5), calls.recv())
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test(start_paused = true)]
    async fn live_window_waits_for_timer_and_serializes_calls() {
        let mut f = Fixture::new();
        let (fake, mut calls) = Fake::new(false, true);
        let live = f.start(fake.clone());
        f.append("synthetic first turn", false);
        f.finalized();
        f.wait_for_status(|status| status.pending == 1).await;
        tokio::time::advance(Duration::from_secs(44)).await;
        assert!(calls.try_recv().is_err());
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(next_call(&mut calls).await.timeout, Duration::from_secs(60));
        for _ in 0..6 {
            f.append("synthetic queued turn", false);
            f.finalized();
        }
        fake.first_block.as_ref().unwrap().add_permits(1);
        assert!(next_call(&mut calls)
            .await
            .user
            .contains("synthetic queued turn"));
        f.stopped();
        live.await.unwrap();
        assert_eq!(fake.max_active.load(Ordering::SeqCst), 1);
        assert_eq!(f.events()[0].source, PatchSource::Live);
    }

    #[tokio::test(start_paused = true)]
    async fn three_live_failures_stop_calls_but_keep_importing() {
        let f = Fixture::new();
        f.consumer.settings.lock().unwrap().graph_batch_utterances = Some(1);
        let (fake, mut calls) = Fake::scripted(0..3, Vec::new(), false);
        let live = f.start(fake.clone());
        for count in 1..=3 {
            f.append("synthetic turn", false);
            f.finalized();
            next_call(&mut calls).await;
            assert_eq!(f.consumer.status(f.sid).pending, count);
        }
        f.wait_for_status(|status| status.live_halted).await;
        f.append("synthetic turn after halt", false);
        f.finalized();
        f.wait_for_status(|status| status.pending == 4).await;
        assert_eq!(fake.count.load(Ordering::SeqCst), 3);
        f.stopped();
        live.await.unwrap();
        assert!(next_call(&mut calls)
            .await
            .user
            .contains("synthetic turn after halt"));
        assert_eq!(f.consumer.pending_count(f.sid).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn post_meeting_actions_are_rejected_while_recording() {
        let f = Fixture::new();
        let (fake, _) = Fake::new(false, false);
        let live = f.start(fake.clone());
        for action in [
            PostMeeting::Continue,
            PostMeeting::Rebuild,
            PostMeeting::Retranscribed,
        ] {
            assert!(f
                .consumer
                .post_meeting_with(f.sid, action, true, |_| async {
                    panic!("post-meeting backend loaded during recording")
                })
                .is_none());
        }
        f.stopped();
        live.await.unwrap();
        f.post(PostMeeting::Continue, fake.clone()).await.unwrap();
        f.post(PostMeeting::Rebuild, fake).await.unwrap();
    }

    #[tokio::test]
    async fn unreadable_response_is_retried_once_with_error_kind() {
        for second in [EMPTY_PATCH, "unreadable"] {
            let f = Fixture::new();
            f.append("synthetic turn", false);
            let (fake, mut calls) = Fake::scripted(0..0, vec!["unreadable", second], false);
            // A single increment isolates parse retries from the flush failure budget.
            let backend = fake.backend(&AppSettings::default());
            let session = f.consumer.session(f.sid);
            let mut run = Run::new(
                &f.consumer,
                f.sid,
                &session,
                &backend,
                session.generation.subscribe(),
            );
            let result = run.increment(PatchSource::Flush, false).await;
            assert_eq!(
                result,
                if second == EMPTY_PATCH {
                    Ok(true)
                } else {
                    Err("no_object")
                }
            );
            let first = calls.try_recv().unwrap();
            let retry = calls.try_recv().unwrap();
            assert_eq!(
                retry.user,
                format!("{}\nReturn the patch as a JSON object with an ops array. Previous error: no_object", first.user)
            );
            assert_eq!(
                f.consumer.pending_count(f.sid).await.unwrap(),
                usize::from(second != EMPTY_PATCH)
            );
            assert!(calls.try_recv().is_err());
        }
    }

    #[tokio::test]
    async fn disabling_during_recording_stops_live_calls_and_continue_can_resume() {
        let mut f = Fixture::new();
        f.consumer.settings.lock().unwrap().graph_batch_utterances = Some(1);
        let (fake, mut calls) = Fake::scripted(0..0, vec![ISSUE_PATCH], false);
        let live = f.start(fake.clone());
        f.append("synthetic first turn", false);
        f.finalized();
        next_call(&mut calls).await;
        f.wait_for_status(|status| status.pending == 0).await;

        f.consumer.settings.lock().unwrap().graph_enabled = Some(false);
        f.append("synthetic turn after disabling", false);
        f.finalized();
        tokio::select! {
            _ = f.wait_for_status(|status| status.disabled_reason.is_some()) => {}
            call = calls.recv() => panic!("unexpected live call: {}", call.unwrap().timeout.as_secs()),
        }
        live.await.unwrap();
        f.consumer.recording_stopped(f.sid);
        assert!(calls.try_recv().is_err());
        assert_eq!(f.events().len(), 1);
        assert_eq!(
            f.consumer.status(f.sid).disabled_reason,
            Some(DisabledReason::DisabledDuringRun)
        );

        f.consumer.settings.lock().unwrap().graph_enabled = Some(true);
        f.post(PostMeeting::Continue, fake).await.unwrap();
        assert_eq!(calls.try_recv().unwrap().timeout, Duration::from_secs(60));
        assert_eq!(calls.try_recv().unwrap().timeout, Duration::from_secs(120));
        assert!(calls.try_recv().is_err());
        assert_eq!(f.consumer.pending_count(f.sid).await.unwrap(), 0);
        assert_eq!(f.consumer.status(f.sid).disabled_reason, None);
    }

    #[tokio::test]
    async fn disabling_before_stop_blocks_flush_and_consolidation() {
        for pending_at_stop in [true, false] {
            let mut f = Fixture::new();
            f.consumer.settings.lock().unwrap().graph_batch_utterances = Some(1);
            let (fake, mut calls) = Fake::scripted(0..0, vec![ISSUE_PATCH], false);
            let live = f.start(fake);
            f.append("synthetic first turn", false);
            f.finalized();
            next_call(&mut calls).await;
            f.wait_for_status(|status| status.pending == 0).await;

            f.consumer.settings.lock().unwrap().graph_enabled = Some(false);
            if pending_at_stop {
                f.append("synthetic turn just before stop", false);
            }
            f.stopped();
            live.await.unwrap();
            assert!(
                calls.try_recv().is_err(),
                "pending at stop: {pending_at_stop}"
            );
            let updates = f.events();
            assert_eq!(updates.len(), 1);
            assert_eq!(updates[0].source, PatchSource::Live);
            assert_eq!(
                f.consumer.status(f.sid).disabled_reason,
                Some(DisabledReason::DisabledDuringRun)
            );
        }
    }

    #[tokio::test]
    async fn stopped_consolidates_when_live_extraction_has_caught_up() {
        let mut f = Fixture::new();
        f.consumer.settings.lock().unwrap().graph_batch_utterances = Some(1);
        let (fake, mut calls) = Fake::scripted(0..0, vec![ISSUE_PATCH], false);
        let live = f.start(fake);
        f.append("synthetic turn extracted during recording", false);
        f.finalized();
        assert_eq!(next_call(&mut calls).await.timeout, Duration::from_secs(60));
        f.wait_for_status(|status| status.pending == 0).await;
        assert_eq!(f.consumer.pending_count(f.sid).await.unwrap(), 0);
        let updates = f.events();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].source, PatchSource::Live);
        assert!(calls.try_recv().is_err());

        f.stopped();
        live.await.unwrap();
        assert_eq!(calls.try_recv().unwrap().timeout, Duration::from_secs(120));
        assert!(calls.try_recv().is_err());
        let updates = f.events();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].source, PatchSource::Consolidate);
    }

    #[tokio::test]
    async fn consolidation_uses_a_longer_timeout_than_extraction() {
        let mut f = Fixture::new();
        f.append("synthetic turn", false);
        let (fake, mut calls) = Fake::scripted(0..0, vec![ISSUE_PATCH], false);
        f.post(PostMeeting::Continue, fake).await.unwrap();
        assert_eq!(calls.try_recv().unwrap().timeout, Duration::from_secs(60));
        assert_eq!(calls.try_recv().unwrap().timeout, Duration::from_secs(120));
        assert!(calls.try_recv().is_err());
        assert_eq!(f.events().last().unwrap().source, PatchSource::Consolidate);
    }

    #[tokio::test]
    async fn consolidation_skips_graphs_without_live_nodes() {
        let mut f = Fixture::new();
        f.append("synthetic turn without graph nodes", false);
        let (fake, mut calls) = Fake::new(false, false);
        f.post(PostMeeting::Continue, fake).await.unwrap();
        assert_eq!(calls.try_recv().unwrap().timeout, Duration::from_secs(60));
        assert!(calls.try_recv().is_err());
        assert_eq!(f.events().len(), 1);
        assert!(!f.consumer.load_outline(f.sid, true).await.unwrap().1);
    }

    #[tokio::test]
    async fn pending_count_imports_finalized_rows() {
        let f = Fixture::new();
        f.append("synthetic finalized turn without a notification", false);
        assert_eq!(f.consumer.pending_count(f.sid).await.unwrap(), 1);
        assert_eq!(f.consumer.pending_count(f.sid).await.unwrap(), 1);
    }

    const TWO_ISSUES: &str = r#"{"ops":[
        {"op":"add_node","tmp":"a","kind":"issue","text":"Synthetic issue A","confidence":1,"grounds":[{"uid":"u1"}]},
        {"op":"add_node","tmp":"b","kind":"issue","text":"Synthetic issue B","confidence":1,"grounds":[{"uid":"u1"}]}
    ]}"#;
    const ATTACH_ISSUE: &str = r#"{"ops":[
        {"op":"add_edge","src":"n2","dst":"n1","kind":"sub_issue_of","confidence":1,"grounds":[{"uid":"u1"}]}
    ]}"#;

    #[tokio::test]
    async fn consolidation_builds_groups_after_saving_issue_patch() {
        let mut f = Fixture::new();
        f.append("synthetic turn with two issues", false);
        let (fake, mut calls) = Fake::scripted(0..0, vec![TWO_ISSUES, ATTACH_ISSUE], false);
        f.post(PostMeeting::Continue, fake).await.unwrap();

        assert_eq!(calls.try_recv().unwrap().timeout, Duration::from_secs(60));
        let issues = calls.try_recv().unwrap();
        assert_eq!(issues.timeout, Duration::from_secs(120));
        assert!(!issues.user.contains(r#""kind":"sub_issue_of""#));
        let groups = calls.try_recv().unwrap();
        assert_eq!(groups.timeout, Duration::from_secs(120));
        assert!(groups.user.contains(r#""kind":"sub_issue_of""#));
        assert!(calls.try_recv().is_err());
        let events = f.events();
        assert_eq!(events.len(), 3);
        assert_eq!(events[1].source, PatchSource::Consolidate);
        assert_eq!(events[1].added, 1);
        assert_eq!(events[2].source, PatchSource::Consolidate);
    }

    #[tokio::test]
    async fn flush_and_consolidation_failures_increment_failure_count() {
        for (provider_failures, expected_count) in [(0..3, 3), (1..2, 1)] {
            let f = Fixture::new();
            f.append("synthetic utterance", false);
            let (fake, _) = Fake::scripted(provider_failures, vec![ISSUE_PATCH], false);
            f.post(PostMeeting::Continue, fake).await.unwrap();

            assert!(!f.consumer.sessions().active.contains_key(&f.sid));
            let status = f.consumer.status(f.sid);
            assert_eq!(status.failure_count, expected_count);
            assert!(!f.consumer.load_outline(f.sid, true).await.unwrap().1);
        }
    }

    #[tokio::test]
    async fn human_reviews_are_saved_and_protected_during_consolidation() {
        use discussion_graph::{Review, Status};

        let mut f = Fixture::new();
        f.append("synthetic turn with two issues", false);
        let (fake, _) = Fake::scripted(0..0, vec![TWO_ISSUES, ATTACH_ISSUE], false);
        assert!(!f.consumer.load_outline(f.sid, true).await.unwrap().1);
        assert!(f
            .consumer
            .set_review(
                f.sid,
                0,
                0,
                ReviewTarget::Node("n1"),
                ReviewDecision::Approve
            )
            .await
            .is_err());
        f.post(PostMeeting::Continue, fake).await.unwrap();
        assert!(f.consumer.load_outline(f.sid, true).await.unwrap().1);
        let before = f.consumer.load_graph(f.sid).await.unwrap();
        let edge_id = before.edges.values().next().unwrap().eid.clone();

        for (offset, target) in [ReviewTarget::Node("n1"), ReviewTarget::Edge(&edge_id)]
            .into_iter()
            .enumerate()
        {
            f.consumer
                .set_review(
                    f.sid,
                    0,
                    before.rev + offset as u64,
                    target,
                    ReviewDecision::Approve,
                )
                .await
                .unwrap();
        }

        let db = session_store::open_with_pragmas(&f.consumer.database).unwrap();
        for (rev, kind, review) in [
            (before.rev + 1, "node", "approved"),
            (before.rev + 2, "edge", "approved"),
        ] {
            let (source, ops): (String, String) = db
                .query_row(
                    "SELECT source, ops_json FROM graph_patches WHERE rev = ?1",
                    [rev],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(source, "review");
            let ops: serde_json::Value = serde_json::from_str(&ops).unwrap();
            assert_eq!(ops[0]["kind"], kind);
            assert_eq!(ops[0]["review"], review);
        }
        let reviewed = f.consumer.load_graph(f.sid).await.unwrap();
        assert_eq!(reviewed.nodes["n1"].review, Review::Approved);
        assert_eq!(reviewed.edges[&edge_id].review, Review::Approved);
        assert_eq!(f.consumer.status(f.sid).rev, reviewed.rev);

        const CHANGE_REVIEWED: &str = r#"{"ops":[
            {"op":"set_status","nid":"n1","status":"resolved","grounds":[{"uid":"u1"}]},
            {"op":"remove_edge","src":"n2","dst":"n1","kind":"sub_issue_of","grounds":[{"uid":"u1"}]}
        ]}"#;
        let ops = parse_ops(CHANGE_REVIEWED).unwrap();
        let result = discussion_graph::apply_patch_with_context(
            &reviewed,
            &ops,
            &["u1"],
            PatchContext::IssueConsolidation,
        );
        assert_eq!(
            result
                .rejected
                .get(&discussion_graph::Rejection::RemoveEdgeReviewed),
            Some(&1)
        );

        f.events();
        f.append("synthetic turn to trigger another consolidation", false);
        let (fake, _) = Fake::scripted(0..0, vec![EMPTY_PATCH, CHANGE_REVIEWED], false);
        f.post(PostMeeting::Continue, fake).await.unwrap();
        let events = f.events();
        assert!(events
            .iter()
            .any(|event| event.source == PatchSource::Consolidate && event.rejected == 2));
        let graph = f.consumer.load_graph(f.sid).await.unwrap();
        assert_eq!(graph.nodes["n1"].status, Status::Open);
        assert_eq!(graph.nodes["n1"].review, Review::Approved);
        assert_eq!(graph.edges[&edge_id].review, Review::Approved);
    }

    #[tokio::test]
    async fn review_waits_for_the_session_executor() {
        let f = Fixture::new();
        f.append("synthetic turn", false);
        let (fake, _) = Fake::scripted(0..0, vec![ISSUE_PATCH], false);
        f.post(PostMeeting::Continue, fake).await.unwrap();
        let session = f.consumer.session(f.sid);
        let execution = session.execution.lock().await;
        let before = f.consumer.status(f.sid).rev;
        let review = f.consumer.set_review(
            f.sid,
            0,
            before,
            ReviewTarget::Node("n1"),
            ReviewDecision::Approve,
        );
        tokio::pin!(review);
        assert!(
            tokio::time::timeout(Duration::from_millis(200), &mut review)
                .await
                .is_err()
        );
        assert_eq!(f.consumer.status(f.sid).rev, before);
        drop(execution);
        review.await.unwrap();
        assert_eq!(f.consumer.status(f.sid).rev, before + 1);
    }

    async fn review_fixture() -> (Fixture, Graph) {
        let f = Fixture::new();
        f.append("synthetic turn with two issues", false);
        let (fake, _) = Fake::scripted(0..0, vec![TWO_ISSUES, ATTACH_ISSUE], false);
        f.post(PostMeeting::Continue, fake).await.unwrap();
        let graph = f.consumer.load_graph(f.sid).await.unwrap();
        (f, graph)
    }

    #[tokio::test]
    async fn review_rejects_a_rebuilt_graph_even_when_revision_and_node_id_match() {
        let (f, listed) = review_fixture().await;
        let (fake, _) = Fake::scripted(0..0, vec![TWO_ISSUES, ATTACH_ISSUE], false);
        f.post(PostMeeting::Rebuild, fake).await.unwrap();
        let (rebuilt, consolidated) = f.consumer.load_outline(f.sid, true).await.unwrap();
        assert!(consolidated);
        assert_eq!(rebuilt.rev, listed.rev);
        assert_ne!(f.consumer.status(f.sid).graph_generation, 0);
        assert_eq!(
            f.consumer
                .set_review(
                    f.sid,
                    0,
                    listed.rev,
                    ReviewTarget::Node("n1"),
                    ReviewDecision::Approve
                )
                .await,
            Err("review_stale")
        );
        assert_eq!(f.consumer.load_graph(f.sid).await.unwrap(), rebuilt);
    }

    #[tokio::test]
    async fn review_is_unavailable_during_recording_rebuilding_or_extraction() {
        for (recording, rebuilding, running) in [
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            let (f, listed) = review_fixture().await;
            let session = f.consumer.session(f.sid);
            {
                let mut state = session.state();
                state.recording = recording;
                state.status.rebuilding = rebuilding;
                state.status.running = running;
            }
            assert!(!f.consumer.status(f.sid).review_allowed);
            assert!(!f.consumer.load_outline(f.sid, true).await.unwrap().1);
            assert_eq!(
                f.consumer
                    .set_review(
                        f.sid,
                        0,
                        listed.rev,
                        ReviewTarget::Node("n1"),
                        ReviewDecision::Approve
                    )
                    .await,
                Err("review_unavailable")
            );
            assert_eq!(f.consumer.load_graph(f.sid).await.unwrap(), listed);
        }
    }

    #[tokio::test]
    async fn review_rejects_edges_with_a_merged_or_rejected_endpoint() {
        for (status, review) in [("merged", "auto"), ("open", "rejected")] {
            let (f, listed) = review_fixture().await;
            let edge = listed.edges.values().next().unwrap();
            // A merge normally redirects edges. Keep this edge attached to a
            // dead endpoint to exercise the consumer's defensive check.
            let db = session_store::open_with_pragmas(&f.consumer.database).unwrap();
            db.execute(
                "UPDATE graph_nodes SET status = ?1, review = ?2 WHERE nid = ?3",
                (status, review, &edge.src),
            )
            .unwrap();
            assert!(f
                .consumer
                .set_review(
                    f.sid,
                    0,
                    listed.rev,
                    ReviewTarget::Edge(&edge.eid),
                    ReviewDecision::Approve
                )
                .await
                .is_err());
            let graph = f.consumer.load_graph(f.sid).await.unwrap();
            assert_eq!(graph.rev, listed.rev);
            assert_eq!(
                graph.edges[&edge.eid].review,
                discussion_graph::Review::Auto
            );
        }
    }

    #[tokio::test]
    async fn queued_review_rejects_a_changed_revision() {
        let (f, listed) = review_fixture().await;
        let session = f.consumer.session(f.sid);
        let execution = session.execution.lock().await;
        let edge = listed.edges.values().next().unwrap();
        let review = f.consumer.set_review(
            f.sid,
            0,
            listed.rev,
            ReviewTarget::Edge(&edge.eid),
            ReviewDecision::Approve,
        );
        tokio::pin!(review);
        std::future::poll_fn(|cx| {
            assert!(std::future::Future::poll(review.as_mut(), cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;

        let sid = f.sid;
        f.consumer
            .db(move |store| {
                store.set_review(sid, ReviewTarget::Node("n1"), ReviewDecision::Approve)
            })
            .await
            .unwrap();
        drop(execution);
        assert_eq!(review.await, Err("review_stale"));
        let graph = f.consumer.load_graph(f.sid).await.unwrap();
        assert_eq!(graph.rev, listed.rev + 1);
        assert_eq!(
            graph.edges[&edge.eid].review,
            discussion_graph::Review::Auto
        );
    }

    #[tokio::test]
    async fn review_remains_available_after_retranscription_flush() {
        const LOW_CONFIDENCE_ISSUE: &str = r#"{"ops":[{"op":"add_node","tmp":"issue","kind":"issue","text":"Synthetic issue","confidence":0.4,"grounds":[{"uid":"u1"}]}]}"#;
        let f = Fixture::new();
        f.append("synthetic first turn", false);
        let (fake, _) = Fake::scripted(0..0, vec![LOW_CONFIDENCE_ISSUE], false);
        f.post(PostMeeting::Continue, fake).await.unwrap();
        let (listed, available) = f.consumer.load_outline(f.sid, true).await.unwrap();
        assert!(available);
        assert!(listed.nodes["n1"].confidence < f.consumer.review_threshold());

        f.append("synthetic retranscribed turn", true);
        let (fake, _) = Fake::new(false, false);
        f.post(PostMeeting::Retranscribed, fake).await.unwrap();

        let (graph, available) = f.consumer.load_outline(f.sid, true).await.unwrap();
        assert!(graph.rev > listed.rev);
        assert!(available);
        f.consumer
            .set_review(
                f.sid,
                0,
                graph.rev,
                ReviewTarget::Node("n1"),
                ReviewDecision::Approve,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn unfinished_second_stage_cannot_be_reviewed_even_after_reopening() {
        let f = Fixture::new();
        f.append("synthetic turn with two issues", false);
        let (fake, _) = Fake::scripted(2..3, vec![TWO_ISSUES, ATTACH_ISSUE], false);
        f.post(PostMeeting::Continue, fake).await.unwrap();
        assert_eq!(f.consumer.status(f.sid).failure_count, 1);
        let sid = f.sid;
        assert!(f
            .consumer
            .db(move |store| store.is_consolidated(sid))
            .await
            .unwrap());

        // Check the persisted state independently of the interrupted run.
        for consumer in [f.reopen_consumer(), f.consumer.clone()] {
            let (graph, available) = consumer.load_outline(sid, true).await.unwrap();
            assert!(!available);
            assert_eq!(
                consumer
                    .set_review(
                        sid,
                        0,
                        graph.rev,
                        ReviewTarget::Node("n1"),
                        ReviewDecision::Approve
                    )
                    .await,
                Err("review_unavailable")
            );
        }
    }

    #[tokio::test]
    async fn interrupted_second_stage_blocks_review_with_an_older_completion() {
        let (f, _) = review_fixture().await;
        f.append("synthetic follow-up turn", false);
        let (fake, _) = Fake::scripted(2..3, vec![EMPTY_PATCH, EMPTY_PATCH], false);
        f.post(PostMeeting::Continue, fake).await.unwrap();
        assert_eq!(f.consumer.status(f.sid).failure_count, 1);

        let (graph, available) = f.consumer.load_outline(f.sid, true).await.unwrap();
        assert!(!available);
        assert_eq!(
            f.consumer
                .set_review(
                    f.sid,
                    0,
                    graph.rev,
                    ReviewTarget::Node("n1"),
                    ReviewDecision::Approve
                )
                .await,
            Err("review_unavailable")
        );
    }

    #[tokio::test]
    async fn review_list_uses_saved_consolidation_after_reopening() {
        let (f, listed) = review_fixture().await;
        let reopened = f.reopen_consumer();
        let (graph, consolidated) = reopened.load_outline(f.sid, true).await.unwrap();
        assert!(consolidated);
        assert_eq!(graph, listed);
        reopened
            .set_review(
                f.sid,
                0,
                graph.rev,
                ReviewTarget::Node("n1"),
                ReviewDecision::Reject,
            )
            .await
            .unwrap();
        let (graph, consolidated) = reopened.load_outline(f.sid, true).await.unwrap();
        assert!(consolidated);
        assert!(!graph.nodes["n1"].is_live());
    }

    #[tokio::test]
    async fn status_revision_tracks_each_saved_patch_and_survives_executor_release() {
        let mut f = Fixture::new();
        f.append("synthetic turn", true);
        assert_eq!(f.consumer.status(f.sid).rev, 0);
        let graph = f.consumer.load_graph(f.sid).await.unwrap();
        assert!(
            graph.utterances.is_empty(),
            "reading must not import finalized rows"
        );

        let (fake, _) = Fake::scripted(0..0, vec![ISSUE_PATCH], false);
        f.post(PostMeeting::Retranscribed, fake).await.unwrap();
        assert_eq!(f.consumer.status(f.sid).rev, 1);

        let (fake, _) = Fake::new(false, false);
        f.post(PostMeeting::Continue, fake).await.unwrap();
        let revisions: Vec<_> = f.events().iter().map(|update| update.rev).collect();
        assert_eq!(revisions, [1, 2]);
        assert_eq!(f.consumer.status(f.sid).rev, 2);
        let graph = f.consumer.load_graph(f.sid).await.unwrap();
        assert_eq!(graph.rev, 2);
    }

    #[tokio::test]
    async fn rebuilding_resets_revision_and_advances_graph_generation() {
        let f = Fixture::new();
        f.append("synthetic turn", true);
        let (fake, _) = Fake::scripted(0..0, vec![ISSUE_PATCH], false);
        f.post(PostMeeting::Retranscribed, fake).await.unwrap();
        let before = f.consumer.status(f.sid);
        assert_eq!(before.rev, 1);

        let (fake, mut calls) = Fake::new(false, true);
        let rebuild = f.post(PostMeeting::Rebuild, fake.clone());
        next_call(&mut calls).await;
        let after = f.consumer.status(f.sid);
        assert_eq!(after.rev, 0);
        assert_eq!(after.graph_generation, before.graph_generation + 1);
        assert_eq!(f.consumer.load_graph(f.sid).await.unwrap().rev, 0);

        fake.first_block.as_ref().unwrap().add_permits(1);
        rebuild.await.unwrap();
    }

    #[tokio::test]
    async fn repeated_backend_failure_survives_idle_executor_pruning() {
        let f = Fixture::new();
        for failure_count in 1..=2 {
            f.consumer
                .post_meeting_with(f.sid, PostMeeting::Continue, true, |_| async {
                    Err(DisabledReason::Backend)
                })
                .unwrap()
                .await
                .unwrap();
            assert!(!f.consumer.sessions().active.contains_key(&f.sid));
            let status = f.consumer.status(f.sid);
            assert_eq!(status.pending, 0);
            assert_eq!(status.rev, 0);
            assert_eq!(status.disabled_reason, Some(DisabledReason::Backend));
            assert_eq!(status.failure_count, failure_count);
        }
    }

    #[tokio::test]
    async fn disabling_between_consolidation_stages_leaves_consolidation_pending() {
        let mut f = Fixture::new();
        f.append("synthetic turn", false);
        let (fake, _) = Fake::scripted(0..0, vec![TWO_ISSUES, ATTACH_ISSUE], false);
        let mut fake = Arc::try_unwrap(fake).ok().unwrap();
        fake.disable_on_call = Some((1, f.consumer.settings.clone()));
        let fake = Arc::new(fake);
        f.post(PostMeeting::Continue, fake.clone()).await.unwrap();
        assert_eq!(fake.count.load(Ordering::SeqCst), 2);
        assert_eq!(
            f.consumer.status(f.sid).disabled_reason,
            Some(DisabledReason::DisabledDuringRun)
        );
        assert_eq!(f.events().len(), 2);

        f.consumer.settings.lock().unwrap().graph_enabled = Some(true);
        let (resume, _) = Fake::new(false, false);
        f.post(PostMeeting::Continue, resume).await.unwrap();
        let updates = f.events();
        assert!(!updates.is_empty());
        assert!(updates
            .iter()
            .all(|update| update.source == PatchSource::Consolidate));
    }

    #[tokio::test]
    async fn only_issue_consolidation_can_replace_a_saved_parent() {
        let mut f = Fixture::new();
        f.append("synthetic turn with three issues", true);
        let extraction = r#"{"ops":[
            {"op":"add_node","tmp":"old","kind":"issue","text":"Synthetic old parent","confidence":1,"grounds":[{"uid":"u1"}]},
            {"op":"add_node","tmp":"new","kind":"issue","text":"Synthetic new parent","confidence":1,"grounds":[{"uid":"u1"}]},
            {"op":"add_node","tmp":"child","kind":"issue","text":"Synthetic child","confidence":1,"grounds":[{"uid":"u1"}]},
            {"op":"add_edge","src":"child","dst":"old","kind":"sub_issue_of","confidence":1,"grounds":[{"uid":"u1"}]}
        ]}"#;
        let remove_original = r#"{"ops":[
            {"op":"remove_edge","src":"n3","dst":"n1","kind":"sub_issue_of","grounds":[{"uid":"u1"}]}
        ]}"#;
        let issues = r#"{"ops":[
            {"op":"remove_edge","src":"n3","dst":"n1","kind":"sub_issue_of","grounds":[{"uid":"u1"}]},
            {"op":"add_edge","src":"n3","dst":"n2","kind":"sub_issue_of","confidence":1,"grounds":[{"uid":"u1"}]}
        ]}"#;
        let groups = r#"{"ops":[
            {"op":"remove_edge","src":"n3","dst":"n2","kind":"sub_issue_of","grounds":[{"uid":"u1"}]}
        ]}"#;
        // A retranscription takes the incremental path without starting consolidation.
        let (fake, _) = Fake::scripted(0..0, vec![extraction], false);
        f.post(PostMeeting::Retranscribed, fake).await.unwrap();

        f.append("synthetic follow-up", false);

        let (fake, _) = Fake::scripted(0..0, vec![remove_original, issues, groups], false);
        f.post(PostMeeting::Continue, fake).await.unwrap();

        let sid = f.sid;
        let graph = f
            .consumer
            .db(move |store| store.load_graph(sid))
            .await
            .unwrap();
        let parents: Vec<_> = graph
            .edges
            .values()
            .filter(|edge| edge.src == "n3" && edge.kind == discussion_graph::EdgeKind::SubIssueOf)
            .map(|edge| edge.dst.as_str())
            .collect();
        assert_eq!(parents, ["n2"]);

        let events = f.events();
        assert_eq!(events.len(), 4);
        assert_eq!(events[1].source, PatchSource::Flush);
        assert_eq!(events[1].rejected, 1);
        assert_eq!(events[2].source, PatchSource::Consolidate);
        assert_eq!(events[2].rejected, 0);
        assert_eq!(events[3].source, PatchSource::Consolidate);
        assert_eq!(events[3].rejected, 1);
    }

    #[tokio::test]
    async fn consolidation_retries_each_unreadable_stage_once() {
        let mut f = Fixture::new();
        f.append("synthetic turn with two issues", false);
        let (fake, mut calls) = Fake::scripted(
            0..0,
            vec![
                TWO_ISSUES,
                "invalid JSON",
                ATTACH_ISSUE,
                "invalid JSON",
                EMPTY_PATCH,
            ],
            false,
        );
        f.post(PostMeeting::Continue, fake).await.unwrap();

        assert_eq!(calls.try_recv().unwrap().timeout, Duration::from_secs(60));
        let issues = assert_consolidation_retry(&mut calls);
        assert!(!issues.user.contains(r#""kind":"sub_issue_of""#));
        let groups = assert_consolidation_retry(&mut calls);
        assert!(groups.user.contains(r#""kind":"sub_issue_of""#));
        assert!(calls.try_recv().is_err());
        let events = f.events();
        assert_eq!(events.len(), 3);
        assert_eq!(events[1].source, PatchSource::Consolidate);
        assert_eq!(events[1].added, 1);
        assert_eq!(events[2].source, PatchSource::Consolidate);
    }

    fn assert_consolidation_retry(
        calls: &mut mpsc::UnboundedReceiver<CompletionRequest>,
    ) -> CompletionRequest {
        let first = calls.try_recv().unwrap();
        let retry = calls
            .try_recv()
            .expect("unreadable response should be retried");
        assert_eq!(first.timeout, Duration::from_secs(120));
        assert_eq!(retry.timeout, first.timeout);
        assert_eq!(retry.user, format!("{}{RETRY_NOTE}no_object", first.user));
        first
    }

    #[tokio::test]
    async fn invalid_issue_response_skips_groups_and_can_resume_without_a_new_patch() {
        let mut f = Fixture::new();
        f.append("synthetic turn with two issues", false);
        let (initial, mut initial_calls) = Fake::scripted(
            0..0,
            vec![TWO_ISSUES, "invalid JSON", "invalid JSON"],
            false,
        );
        f.post(PostMeeting::Continue, initial).await.unwrap();
        assert_eq!(
            initial_calls.try_recv().unwrap().timeout,
            Duration::from_secs(60)
        );
        assert_consolidation_retry(&mut initial_calls);
        assert!(initial_calls.try_recv().is_err());
        let events = f.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].source, PatchSource::Flush);
        assert_eq!(f.consumer.pending_count(f.sid).await.unwrap(), 0);

        let (fake, mut calls) = Fake::new(false, false);
        f.post(PostMeeting::Continue, fake.clone()).await.unwrap();
        assert_eq!(calls.try_recv().unwrap().timeout, Duration::from_secs(120));
        assert_eq!(calls.try_recv().unwrap().timeout, Duration::from_secs(120));
        assert!(calls.try_recv().is_err());

        f.post(PostMeeting::Continue, fake.clone()).await.unwrap();
        assert!(calls.try_recv().is_err());

        f.append("synthetic new turn after consolidation", false);
        f.post(PostMeeting::Continue, fake).await.unwrap();
        assert_eq!(calls.try_recv().unwrap().timeout, Duration::from_secs(60));
        assert_eq!(calls.try_recv().unwrap().timeout, Duration::from_secs(120));
        assert_eq!(calls.try_recv().unwrap().timeout, Duration::from_secs(120));
        assert!(calls.try_recv().is_err());
    }

    #[tokio::test]
    async fn continue_consolidates_a_session_without_a_completion_mark() {
        let f = Fixture::new();
        f.append("synthetic retranscribed turn", true);
        let (initial, _) = Fake::scripted(0..0, vec![ISSUE_PATCH], false);
        f.post(PostMeeting::Retranscribed, initial).await.unwrap();

        let (fake, mut calls) = Fake::new(false, false);
        f.post(PostMeeting::Continue, fake).await.unwrap();
        assert_eq!(calls.try_recv().unwrap().timeout, Duration::from_secs(120));
        assert!(calls.try_recv().is_err());
    }

    #[test]
    fn run_guard_drop_recovers_a_poisoned_state() {
        let session = Session::default();
        session.state.lock().unwrap().status.running = true;
        let _ = std::panic::catch_unwind(|| {
            let _state = session.state.lock().unwrap();
            panic!("poison the session state");
        });
        drop(RunGuard {
            session: &session,
            rebuild: false,
        });
        assert!(
            !session
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .status
                .running
        );
    }

    #[tokio::test]
    async fn finished_sessions_release_their_executor_but_keep_disabled_reason() {
        let f = Fixture::new();
        let task = f
            .consumer
            .start_recording_with(f.sid, true, |_| async { Err(DisabledReason::Backend) })
            .unwrap();
        task.await.unwrap();
        assert!(f.consumer.sessions().active.contains_key(&f.sid));
        f.consumer.recording_stopped(f.sid);
        assert_eq!(
            f.consumer.status(f.sid).disabled_reason,
            Some(DisabledReason::Backend)
        );
        assert!(!f.consumer.sessions().active.contains_key(&f.sid));

        let (fake, _) = Fake::new(false, false);
        let live = f.start(fake);
        f.stopped();
        live.await.unwrap();
        assert_eq!(f.consumer.status(f.sid).disabled_reason, None);
        assert!(!f.consumer.sessions().active.contains_key(&f.sid));
    }

    #[tokio::test]
    async fn only_stopped_flushes_partial_window_and_rows_saved_after_session_end() {
        for count in 1..=5 {
            let mut f = Fixture::new();
            let (fake, mut calls) = Fake::scripted(0..0, vec![ISSUE_PATCH], false);
            let live = f.start(fake);
            for _ in 0..count {
                f.append("synthetic partial window", false);
            }
            f.finalized();
            f.wait_for_status(|status| status.pending == count).await;
            let event = transcript_event::TranscriptEvent::UtteranceEnded {
                session_id: f.sid,
                segment_id: None,
                reason: transcript_event::UtteranceEndReason::SessionEnd,
            };
            let _ = f.consumer.broker.publish(
                &transcript_event::subject_for(&event, f.sid),
                &EventEnvelope::new(f.sid, event),
            );
            tokio::task::yield_now().await;
            // The last row has no finalized event, as with a pending interim saved during shutdown.
            f.append("synthetic final segment saved at stop", false);
            assert!(calls.try_recv().is_err());
            f.stopped();
            live.await.unwrap();
            let updates = f.events();
            assert_eq!(updates.len(), 2);
            assert_eq!(updates[0].source, PatchSource::Flush);
            assert_eq!(updates[1].source, PatchSource::Consolidate);
            assert!(next_call(&mut calls)
                .await
                .user
                .contains("synthetic final segment saved at stop"));
            assert_eq!(f.consumer.pending_count(f.sid).await.unwrap(), 0);
        }
    }

    #[tokio::test]
    async fn failed_flush_is_bounded_and_skips_consolidation() {
        let mut f = Fixture::new();
        f.append("synthetic unextracted turn", false);
        let (fake, mut calls) = Fake::new(true, false);
        f.post(PostMeeting::Continue, fake).await.unwrap();
        for _ in 0..3 {
            assert_eq!(next_call(&mut calls).await.timeout, Duration::from_secs(60));
        }
        assert!(calls.try_recv().is_err());
        assert!(f.events().is_empty());
        assert_eq!(f.consumer.pending_count(f.sid).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn failed_flush_leaves_consolidation_pending_without_a_new_patch() {
        let mut f = Fixture::new();
        f.append("synthetic issue turn", false);
        let (initial, _) = Fake::scripted(0..0, vec![ISSUE_PATCH], false);
        f.post(PostMeeting::Continue, initial).await.unwrap();
        assert_eq!(f.events().last().unwrap().source, PatchSource::Consolidate);

        f.append("synthetic pending turn", false);
        let (failing, mut failed_calls) = Fake::new(true, false);
        f.post(PostMeeting::Continue, failing).await.unwrap();
        assert_failed_flush_skips_consolidation(&f, &mut failed_calls).await;
        assert!(f.events().is_empty());

        // Mark the remaining turn extracted in the store so the next Continue
        // cannot trigger consolidation by saving a flush patch of its own.
        let sid = f.sid;
        f.consumer
            .db(move |store| store.apply_patch(sid, PatchSource::Flush, None, &[], &["u2"], &[]))
            .await
            .unwrap();
        assert_eq!(f.consumer.pending_count(sid).await.unwrap(), 0);

        let (retry, mut calls) = Fake::new(false, false);
        f.post(PostMeeting::Continue, retry).await.unwrap();
        assert_eq!(calls.try_recv().unwrap().timeout, Duration::from_secs(120));
        assert!(calls.try_recv().is_err());
        let events = f.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].source, PatchSource::Consolidate);
    }

    #[tokio::test]
    async fn stopped_skips_consolidation_after_three_flush_failures() {
        let mut f = Fixture::new();
        f.consumer.settings.lock().unwrap().graph_batch_utterances = Some(1);
        let (fake, mut calls) = Fake::scripted(1..4, vec![ISSUE_PATCH], false);
        let live = f.start(fake);
        f.append("synthetic turn extracted during recording", false);
        f.finalized();
        assert_eq!(next_call(&mut calls).await.timeout, Duration::from_secs(60));
        f.wait_for_status(|status| status.pending == 0).await;
        let updates = f.events();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].source, PatchSource::Live);
        assert_eq!(updates[0].added, 1);

        // A row saved during shutdown can reach the DB without a finalized event.
        f.append("synthetic pending turn at stop", false);
        f.stopped();
        live.await.unwrap();
        assert_failed_flush_skips_consolidation(&f, &mut calls).await;
        assert!(f.events().is_empty());
    }

    #[tokio::test]
    async fn rebuild_skips_consolidation_after_three_flush_failures() {
        let mut f = Fixture::new();
        f.append("synthetic original turn", false);
        // These rows are extracted separately, so the first patch creates a live
        // node before the remaining row hits the failure limit.
        f.append("synthetic retranscribed turn", true);
        let (fake, mut calls) = Fake::scripted(1..4, vec![ISSUE_PATCH], false);
        f.post(PostMeeting::Rebuild, fake).await.unwrap();

        assert_eq!(next_call(&mut calls).await.timeout, Duration::from_secs(60));
        assert_failed_flush_skips_consolidation(&f, &mut calls).await;
        let updates = f.events();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].source, PatchSource::Flush);
        assert_eq!(updates[0].added, 1);
    }

    async fn assert_failed_flush_skips_consolidation(
        f: &Fixture,
        calls: &mut mpsc::UnboundedReceiver<CompletionRequest>,
    ) {
        for _ in 0..3 {
            assert_eq!(next_call(calls).await.timeout, Duration::from_secs(60));
        }
        assert!(
            calls.try_recv().is_err(),
            "only the three extraction calls should run after the first patch"
        );
        assert_eq!(f.consumer.pending_count(f.sid).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn flush_extracts_the_row_after_an_oversize_utterance() {
        let mut f = Fixture::new();
        f.append(&"あ".repeat(DEFAULT_INPUT_LIMIT * 2), false);
        f.append("synthetic later turn", false);
        let (fake, mut calls) = Fake::new(false, false);
        f.post(PostMeeting::Continue, fake).await.unwrap();
        assert!(std::iter::from_fn(|| calls.try_recv().ok())
            .any(|call| call.timeout == Duration::from_secs(60)
                && call.user.contains("synthetic later turn")));
        let updates = f.events();
        assert_eq!(updates[0].rev, 1);
        assert_eq!(f.consumer.pending_count(f.sid).await.unwrap(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn continue_requests_use_one_executor_and_rebuild_cancels_running_flush() {
        let f = Fixture::new();
        f.append("synthetic turn for rebuild", false);
        let (fake, mut calls) = Fake::scripted(0..0, vec![EMPTY_PATCH, ISSUE_PATCH], true);
        let flush = f.post(PostMeeting::Continue, fake.clone());
        next_call(&mut calls).await;
        let (loaded_tx, loaded_rx) = tokio::sync::oneshot::channel();
        let queued_fake = fake.clone();
        let queued = f
            .consumer
            .post_meeting_with(f.sid, PostMeeting::Continue, true, |settings| async move {
                loaded_tx.send(()).unwrap();
                Ok(queued_fake.backend(&settings))
            })
            .unwrap();
        loaded_rx.await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(1), calls.recv())
                .await
                .is_err(),
            "a second completion started while the first was blocked"
        );
        let rebuild = f.post(PostMeeting::Rebuild, fake.clone());
        assert!(f.consumer.status(f.sid).rebuilding);
        assert!(f
            .consumer
            .post_meeting_with(f.sid, PostMeeting::Rebuild, true, |_| async {
                panic!("second rebuild admitted")
            })
            .is_none());
        flush.await.unwrap();
        queued.await.unwrap();
        rebuild.await.unwrap();
        assert_eq!(fake.count.load(Ordering::SeqCst), 3);
        assert!(!f.consumer.sessions().active.contains_key(&f.sid));
        assert_eq!(fake.max_active.load(Ordering::SeqCst), 1);
        assert!(!f.consumer.status(f.sid).rebuilding);
        assert_eq!(f.consumer.pending_count(f.sid).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn retranscription_only_sends_new_gap_rows_without_reference_turns() {
        let mut f = Fixture::new();
        f.append("synthetic earlier context", false);
        let (initial, _) = Fake::new(false, false);
        f.post(PostMeeting::Continue, initial).await.unwrap();
        f.events();
        f.append("synthetic ordinary pending turn", false);
        f.append("synthetic gap turn", true);
        let (fake, mut calls) = Fake::new(false, false);
        f.post(PostMeeting::Retranscribed, fake).await.unwrap();
        let call = next_call(&mut calls).await;
        assert!(call.user.contains("synthetic gap turn"));
        assert!(!call.user.contains("synthetic earlier context"));
        assert!(!call.user.contains("synthetic ordinary pending turn"));
        assert!(calls.try_recv().is_err());
        assert_eq!(f.events().len(), 1);
        assert_eq!(f.consumer.pending_count(f.sid).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn platform_settings_and_backend_gate_extraction() {
        let f = Fixture::new();
        assert!(f
            .consumer
            .start_recording_with(f.sid, false, |_| async { panic!("backend called") })
            .is_none());
        assert_eq!(
            f.consumer.status(f.sid).disabled_reason,
            Some(DisabledReason::Platform)
        );
        f.consumer.recording_stopped(f.sid);
        for reason in [DisabledReason::Backend, DisabledReason::Provider] {
            f.consumer
                .start_recording_with(f.sid, true, move |_| async move { Err(reason) })
                .unwrap()
                .await
                .unwrap();
            assert_eq!(f.consumer.status(f.sid).disabled_reason, Some(reason));
            f.consumer.recording_stopped(f.sid);
        }
        f.consumer.settings.lock().unwrap().graph_enabled = Some(false);
        for action in [
            PostMeeting::Retranscribed,
            PostMeeting::Rebuild,
            PostMeeting::Continue,
        ] {
            assert!(f
                .consumer
                .post_meeting_with(f.sid, action, true, |_| async { panic!("backend called") })
                .is_none());
            assert_eq!(
                f.consumer.status(f.sid).disabled_reason,
                Some(DisabledReason::Disabled)
            );
        }
    }

    struct SelectedProvider(&'static str);

    impl CredentialStore for SelectedProvider {
        fn save(&self, _: &str, _: &str, _: &str) -> Result<(), credential_store::StoreError> {
            unreachable!()
        }

        fn load(
            &self,
            service: &str,
            account: &str,
        ) -> Result<String, credential_store::StoreError> {
            if account == summarize::SELECTED_PROVIDER_ACCOUNT {
                return Ok(self.0.into());
            }
            if account == summarize::SELECTED_MODEL_ACCOUNT {
                return Ok("summary-model".into());
            }
            Err(credential_store::StoreError::NotFound {
                service: service.into(),
                account: account.into(),
            })
        }

        fn delete(&self, _: &str, _: &str) -> Result<(), credential_store::StoreError> {
            unreachable!()
        }
    }

    #[test]
    fn codex_and_missing_api_keys_prevent_backend_creation() {
        let dir = tempfile::tempdir().unwrap();
        for (provider, reason) in [
            ("codex", DisabledReason::Provider),
            ("openai", DisabledReason::Backend),
        ] {
            let result = build_backend(
                Arc::new(SelectedProvider(provider)),
                AppSettings::default(),
                dir.path().into(),
                &Default::default(),
            );
            assert_eq!(result.err(), Some(reason));
        }
        let settings = AppSettings {
            graph_model: Some("graph-model".into()),
            ..Default::default()
        };
        let (_, backend) = build_backend(
            Arc::new(SelectedProvider("claude-cli")),
            settings,
            dir.path().into(),
            &Default::default(),
        )
        .unwrap();
        assert_eq!(backend.model, "graph-model");
    }

    #[test]
    fn explicit_graph_provider_uses_its_default_model() {
        let dir = tempfile::tempdir().unwrap();
        let settings = AppSettings {
            graph_provider: Some("claude-cli".into()),
            graph_model: Some(String::new()),
            ..Default::default()
        };
        let (provider, backend) = build_backend(
            Arc::new(SelectedProvider("codex")),
            settings,
            dir.path().into(),
            &Default::default(),
        )
        .unwrap();
        assert_eq!(provider, SummaryProvider::ClaudeCli);
        assert_eq!(backend.model, provider.default_model());
    }

    #[derive(Clone, Default)]
    struct Log(Arc<Mutex<Vec<u8>>>);

    impl Write for Log {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().write(bytes)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn sends_model_override_and_keeps_transcript_out_of_logs() {
        let f = Fixture::new();
        let log = Log::default();
        let writer = log.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::set_global_default(subscriber).unwrap();
        let text = "private synthetic sentence";
        f.append(text, false);
        f.consumer.settings.lock().unwrap().graph_model = Some("graph-model".into());
        let (fake, mut calls) = Fake::new(false, false);
        let backend = fake.backend(&f.consumer.settings.lock().unwrap());
        let session = f.consumer.session(f.sid);
        let generation = session.generation.subscribe();
        let mut run = Run::new(&f.consumer, f.sid, &session, &backend, generation);
        run.flush(false).await;
        let call = next_call(&mut calls).await;
        assert_eq!(call.model, "graph-model");
        assert!(call.user.contains(text));
        let log = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
        assert!(log
            .lines()
            .any(|line| line.contains("graph patch saved") && line.contains(&f.sid.to_string())));
        assert!(!log.contains(text));
    }
}
