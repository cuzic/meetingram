//! Session-scoped graph extraction. The history actions are wired in task 6.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use credential_store::{CredentialStore, FallbackCredentialStore};
use discussion_graph::{
    build_consolidate, build_incremental, graph_updated_subject, parse_ops, store::GraphStore,
    AppliedOp, ConsolidateStyle, Graph, GraphUpdated, Op, ParseError, PatchSource, Prompt,
    DEFAULT_INPUT_LIMIT,
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
    // A past backend failure cannot be recovered from the current settings.
    disabled: HashMap<SessionId, DisabledReason>,
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
        if let Some(reason) = state.status.disabled_reason {
            self.disabled.insert(sid, reason);
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
    credentials: Arc<FallbackCredentialStore>,
    settings: Arc<Mutex<AppSettings>>,
    cli_temp_base: PathBuf,
    sessions: Mutex<Sessions>,
}

struct Backend {
    model: String,
    summarizer: Box<dyn Summarizer>,
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
        credentials: Arc<FallbackCredentialStore>,
        settings: Arc<Mutex<AppSettings>>,
        cli_temp_base: PathBuf,
    ) -> Self {
        Self {
            broker,
            database,
            store: Mutex::new(None),
            credentials,
            settings,
            cli_temp_base,
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
        let disabled_reason = sessions.disabled.remove(&sid);
        let session = sessions.active.entry(sid).or_default().clone();
        if disabled_reason.is_some() {
            session.state().status.disabled_reason = disabled_reason;
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
        if let Some(session) = sessions.active.get(&sid) {
            session.state().status.clone()
        } else {
            GraphStatus {
                disabled_reason: sessions.disabled.get(&sid).copied(),
                ..GraphStatus::default()
            }
        }
    }

    pub async fn pending_count(self: &Arc<Self>, sid: SessionId) -> Result<usize, &'static str> {
        self.db(move |store| store.pending_count(sid)).await
    }

    #[allow(dead_code)] // Read by the graph view in task 6.
    pub fn review_threshold(&self) -> f64 {
        self.settings
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .graph_review_threshold
            .unwrap_or(0.6)
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
        let credentials = self.credentials.clone();
        let cli_temp_base = self.cli_temp_base.clone();
        let (provider, backend) = tokio::task::spawn_blocking(move || {
            build_backend(credentials, settings, cli_temp_base)
        })
        .await
        .map_err(|_| DisabledReason::Backend)??;
        if let Some(cli) = provider.cli_backend() {
            if !cli.is_available().await {
                return Err(DisabledReason::Backend);
            }
        }
        Ok(backend)
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
        let settings = self
            .settings
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
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
        let settings = self
            .settings
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
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
                        if kind != "cancelled" {
                            run.log_failure(PatchSource::Flush, kind);
                        }
                    }
                }
                PostMeeting::Rebuild => {
                    match consumer.db(move |store| store.delete_session(sid)).await {
                        Ok(()) => run.flush(true).await,
                        Err(kind) => run.log_failure(PatchSource::Flush, kind),
                    }
                }
            }
        }))
    }
}

fn build_backend(
    credentials: Arc<impl CredentialStore + Send + Sync + 'static>,
    settings: AppSettings,
    cli_temp_base: PathBuf,
) -> Result<(SummaryProvider, Backend), DisabledReason> {
    let provider = credentials
        .load(
            summarize::CREDENTIAL_SERVICE,
            summarize::SELECTED_PROVIDER_ACCOUNT,
        )
        .ok()
        .map(|key| SummaryProvider::from_key(&key))
        .unwrap_or(SummaryProvider::Claude);
    if !provider.supports_graph() {
        return Err(DisabledReason::Provider);
    }
    let summary_model = credentials
        .load(
            summarize::CREDENTIAL_SERVICE,
            summarize::SELECTED_MODEL_ACCOUNT,
        )
        .ok();
    let model = summarize::resolve_model(
        provider,
        summary_model.as_deref(),
        settings.graph_model.as_deref(),
    );
    let summarizer = summarize::build_summarizer(
        provider.key(),
        credentials,
        settings.ollama_base_url,
        Some(cli_temp_base),
    )
    .map_err(|_| DisabledReason::Backend)?;
    Ok((provider, Backend { model, summarizer }))
}

struct Run<'a> {
    consumer: &'a Arc<GraphConsumer>,
    sid: SessionId,
    session: &'a Session,
    backend: &'a Backend,
    cancel: watch::Receiver<u64>,
    saved_flush_patch: bool,
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
            saved_flush_patch: false,
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
                    Err("cancelled") => return false,
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
        let count = self
            .consumer
            .db(move |store| {
                store.import_utterances(sid)?;
                store.pending_count(sid)
            })
            .await?;
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
            self.apply(source, prompt).await?;
            let count = self.consumer.pending_count(sid).await?;
            self.session.state().status.pending = count;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn flush(&mut self, consolidate_without_new_patch: bool) {
        self.saved_flush_patch = false;
        let mut failures = 0;
        loop {
            match self.increment(PatchSource::Flush, false).await {
                Ok(false) => break,
                Ok(true) => failures = 0,
                Err("cancelled") => return,
                Err(kind) => {
                    self.log_failure(PatchSource::Flush, kind);
                    failures += 1;
                    if failures >= FAILURE_LIMIT {
                        return;
                    }
                }
            }
        }

        if !consolidate_without_new_patch && !self.saved_flush_patch {
            return;
        }
        if let Err(kind) = self.consolidate().await {
            if kind != "cancelled" {
                self.log_failure(PatchSource::Consolidate, kind);
            }
        }
    }

    async fn consolidate(&mut self) -> Result<(), &'static str> {
        let sid = self.sid;
        let prompts = self
            .consumer
            .db(move |store| {
                let graph = store.load_graph(sid)?;
                if !graph.nodes.values().any(|node| node.is_live()) {
                    return Ok(Ok(Vec::new()));
                }
                Ok(build_consolidate(
                    &graph,
                    ConsolidateStyle::Strengthened,
                    INPUT_LIMIT,
                ))
            })
            .await?
            .map_err(|_| "input_limit")?;
        for prompt in prompts {
            self.apply(PatchSource::Consolidate, prompt).await?;
        }
        Ok(())
    }

    async fn apply(&mut self, source: PatchSource, prompt: Prompt) -> Result<(), &'static str> {
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
        let ops = match self.complete(&req).await? {
            Ok(ops) => ops,
            Err(error) => {
                let kind = parse_kind(error);
                self.log_failure(source, kind);
                req.user.push_str(RETRY_NOTE);
                req.user.push_str(kind);
                self.complete(&req).await?.map_err(parse_kind)?
            }
        };
        if self.cancel.has_changed().unwrap_or(true) {
            return Err("cancelled");
        }

        let sid = self.sid;
        let model = self.backend.model.clone();
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
                store.apply_patch(sid, source, Some(&model), &ops, &input, &references)
            })
            .await?;
        self.saved_flush_patch |= source == PatchSource::Flush;
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
    ) -> Result<Result<Vec<Op>, ParseError>, &'static str> {
        let response = tokio::select! {
            biased;
            _ = self.cancel.changed() => return Err("cancelled"),
            result = tokio::time::timeout(req.timeout, self.backend.summarizer.complete(req)) => {
                result.map_err(|_| "timeout")?.map_err(|error| error.kind())?
            }
        };
        Ok(parse_ops(&response))
    }

    fn log_failure(&self, source: PatchSource, kind: &'static str) {
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
            let credentials =
                Arc::new(FallbackCredentialStore::new(dir.path().join("credentials")).unwrap());
            let consumer = Arc::new(GraphConsumer::new(
                broker,
                path,
                credentials,
                Arc::new(Mutex::new(AppSettings::default())),
                dir.path().join("cli"),
            ));
            Self {
                _dir: dir,
                consumer,
                sid,
                store,
                updates,
            }
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
                summarizer: Box::new(FakeSummarizer(self)),
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
    }

    #[tokio::test]
    async fn continue_only_consolidates_after_a_new_extraction_patch() {
        let f = Fixture::new();
        // A previous increment leaves a live node without running consolidation.
        f.append("synthetic retranscribed turn", true);
        let (initial, _) = Fake::scripted(0..0, vec![ISSUE_PATCH], false);
        f.post(PostMeeting::Retranscribed, initial).await.unwrap();

        let (fake, mut calls) = Fake::new(false, false);
        f.post(PostMeeting::Continue, fake.clone()).await.unwrap();
        assert!(calls.try_recv().is_err());

        f.append("synthetic pending turn", false);
        f.post(PostMeeting::Continue, fake.clone()).await.unwrap();
        assert_eq!(calls.try_recv().unwrap().timeout, Duration::from_secs(60));
        assert_eq!(calls.try_recv().unwrap().timeout, Duration::from_secs(120));
        assert!(calls.try_recv().is_err());

        f.post(PostMeeting::Continue, fake).await.unwrap();
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
        )
        .unwrap();
        assert_eq!(backend.model, "graph-model");
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
