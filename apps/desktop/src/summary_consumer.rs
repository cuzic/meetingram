//! Summary Consumer: subscribes to `transcription.{session_id}.segment.finalized`
//! and `transcription.{session_id}.utterance.ended` via the Local Broker, collects
//! finalized transcript turns, and generates a summary when the session ends.
//!
//! Also exposes `generate_summary_now()` for manual user-triggered summary generation
//! (the "要約を生成" button), which reads from `SessionStore` directly as a fallback.
//!
//! See `docs/decouple-summary-transcription.md` §6.2.

use std::sync::{Arc, Mutex};

use local_broker::{session_stopped_subject, LocalBroker, RecvOutcome, Subscription};
use recorder_domain::SessionId;
use session_store::{SessionStore, Summary};
use summarize::{BuildSummarizerError, TranscriptTurn};
use transcript_event::{
    EventEnvelope, FinalizedTurns, ProtocolValidator, SummaryEvent, TranscriptEvent,
    UtteranceEndReason,
};

use crate::app_settings::AppSettings;
use crate::transcript;

/// Shared state for the Summary Consumer. Clone is cheap (Arc inside).
#[derive(Clone)]
pub struct SummaryConsumer {
    broker: LocalBroker,
    store: Arc<SessionStore>,
    llm: Arc<dyn crate::llm_factory::LlmFactory>,
    app_settings: Arc<Mutex<AppSettings>>,
}

impl SummaryConsumer {
    pub fn new(
        broker: LocalBroker,
        store: Arc<SessionStore>,
        llm: Arc<dyn crate::llm_factory::LlmFactory>,
        app_settings: Arc<Mutex<AppSettings>>,
    ) -> Self {
        Self {
            broker,
            store,
            llm,
            app_settings,
        }
    }

    /// Spawns an auto-summary task for `session_id`. Returns immediately; the
    /// task runs until `UtteranceEnded(SessionEnd)` (published only by the
    /// Windows-only `app_service::live_transcription`) **or**
    /// `session.{id}.stopped` (published by every platform's
    /// `apps/desktop/src/recording.rs::stop`) arrives, then generates the
    /// summary — see `stop_subject`'s use below. Before that second signal
    /// existed, this task waited for an event that macOS/dev-fallback builds
    /// never publish at all, meaning auto-summary silently never ran there
    /// (and the task leaked for the rest of the process's life). Does
    /// nothing if the broker is not configured (broker is always configured
    /// in this app, but the parameter is kept for future IPC mode).
    pub fn spawn_auto_summary(&self, session_id: SessionId) -> tokio::task::JoinHandle<()> {
        let this = self.clone();
        tokio::spawn(async move {
            this.run_auto_summary(session_id).await;
        })
    }

    async fn run_auto_summary(&self, session_id: SessionId) {
        let segment_subject = format!("transcription.{session_id}.segment.finalized");
        let utterance_subject = format!("transcription.{session_id}.utterance.ended");

        let mut segment_sub = Subscription::new(self.broker.clone(), segment_subject);
        let mut utterance_sub = Subscription::new(self.broker.clone(), utterance_subject);
        let mut stop_sub =
            Subscription::new(self.broker.clone(), session_stopped_subject(session_id));

        // 1. Load existing finalized segments from SessionStore (late-join support)
        let mut turns: FinalizedTurns<TranscriptTurn> = FinalizedTurns::new();
        load_finalized_from_store(&self.store, session_id, &mut turns);

        // 2. Initialize protocol validator
        let mut validator = ProtocolValidator::new(session_id);

        // 3. Subscribe to broker events
        loop {
            tokio::select! {
                outcome = segment_sub.recv() => {
                    match outcome {
                        RecvOutcome::Message(payload) => {
                            let envelope: EventEnvelope<TranscriptEvent> = match serde_json::from_slice(&payload) {
                                Ok(e) => e,
                                Err(err) => {
                                    tracing::warn!(%err, "summary_consumer: failed to deserialize SegmentFinalized");
                                    continue;
                                }
                            };
                            if let TranscriptEvent::SegmentFinalized { data, .. } = &envelope.body {
                                // Validate the event against the protocol
                                if let Err(err) = validator.validate(&envelope.body) {
                                    tracing::warn!(%session_id, %err, "summary_consumer: protocol validation error");
                                }
                                turns.insert_if_new(data.segment_id.clone(), TranscriptTurn {
                                    speaker: Some(data.speaker_label.clone()),
                                    text: data.text.clone(),
                                });
                            }
                        }
                        RecvOutcome::Lagged(n) => {
                            tracing::warn!(n, %session_id, "summary_consumer: lagged, reloading from store");
                            load_finalized_from_store(&self.store, session_id, &mut turns);
                        }
                        RecvOutcome::Closed => break,
                    }
                }
                outcome = utterance_sub.recv() => {
                    match outcome {
                        RecvOutcome::Message(payload) => {
                            let envelope: EventEnvelope<TranscriptEvent> = match serde_json::from_slice(&payload) {
                                Ok(e) => e,
                                Err(err) => {
                                    tracing::warn!(%err, "summary_consumer: failed to deserialize UtteranceEnded");
                                    continue;
                                }
                            };
                            if let TranscriptEvent::UtteranceEnded { reason: UtteranceEndReason::SessionEnd, .. } = &envelope.body {
                                // Validate the event against the protocol
                                if let Err(err) = validator.validate(&envelope.body) {
                                    tracing::warn!(%session_id, %err, "summary_consumer: protocol validation error");
                                }
                                break; // Session ended → generate summary
                            }
                        }
                        RecvOutcome::Closed => break,
                        RecvOutcome::Lagged(_) => {}
                    }
                }
                // See `spawn_auto_summary`'s doc comment — whichever of this
                // or the real `SessionEnd` above arrives first finalizes the
                // summary from whatever `turns` have been collected so far;
                // the other, if it still arrives afterward, is simply never
                // observed once this task has already exited.
                _ = stop_sub.recv() => break,
            }
        }

        // 3. Generate summary
        if turns.is_empty() {
            return;
        }

        self.generate_and_publish(session_id, turns.turns()).await;
    }

    /// Manual summary generation (user-triggered "要約を生成" button).
    /// Reads segments from `SessionStore` directly and generates the summary
    /// synchronously (within the async context). Publishes SummaryEvent results.
    pub async fn generate_summary_now(&self, session_id: SessionId) -> Result<String, String> {
        let segments = self
            .store
            .list_transcript_segments(session_id)
            .map_err(|e| format!("文字起こしの取得に失敗しました: {e}"))?;
        let turns = transcript::to_turns(&segments);
        if turns.is_empty() {
            return Err("要約対象の文字起こしがありません".to_string());
        }

        self.broker
            .publish(
                &transcript_event::summary_subject_for(
                    &SummaryEvent::Started { session_id },
                    session_id,
                ),
                &EventEnvelope::new(session_id, SummaryEvent::Started { session_id }),
            )
            .ok();

        match self.summarize(&turns).await {
            Ok((text, _)) => Ok(text),
            Err(e) => {
                self.broker
                    .publish(
                        &transcript_event::summary_subject_for(
                            &SummaryEvent::Failed {
                                session_id,
                                error: e.clone(),
                            },
                            session_id,
                        ),
                        &EventEnvelope::new(
                            session_id,
                            SummaryEvent::Failed {
                                session_id,
                                error: e.clone(),
                            },
                        ),
                    )
                    .ok();
                Err(e)
            }
        }
    }

    async fn generate_and_publish(&self, session_id: SessionId, turns: &[TranscriptTurn]) {
        self.broker
            .publish(
                &transcript_event::summary_subject_for(
                    &SummaryEvent::Started { session_id },
                    session_id,
                ),
                &EventEnvelope::new(session_id, SummaryEvent::Started { session_id }),
            )
            .ok();

        match self.summarize(turns).await {
            Ok((text, provider_model)) => {
                self.store
                    .insert_summary(&Summary {
                        session_id,
                        text: text.clone(),
                        provider_model: provider_model.clone(),
                        generated_at: chrono::Utc::now(),
                    })
                    .ok();

                self.broker
                    .publish(
                        &transcript_event::summary_subject_for(
                            &SummaryEvent::Completed {
                                session_id,
                                text: text.clone(),
                                provider_model: provider_model.clone(),
                            },
                            session_id,
                        ),
                        &EventEnvelope::new(
                            session_id,
                            SummaryEvent::Completed {
                                session_id,
                                text,
                                provider_model,
                            },
                        ),
                    )
                    .ok();
            }
            Err(e) => {
                self.broker
                    .publish(
                        &transcript_event::summary_subject_for(
                            &SummaryEvent::Failed {
                                session_id,
                                error: e.to_string(),
                            },
                            session_id,
                        ),
                        &EventEnvelope::new(
                            session_id,
                            SummaryEvent::Failed {
                                session_id,
                                error: e.to_string(),
                            },
                        ),
                    )
                    .ok();
            }
        }
    }

    async fn summarize(&self, turns: &[TranscriptTurn]) -> Result<(String, String), String> {
        let settings = self.app_settings.lock().unwrap().clone();
        let summary_template = settings.summary_template.clone();
        let backend = self
            .llm
            .summary(settings)
            .await
            .map_err(|error| summary_build_error_message(error).to_string())?;
        let options =
            crate::summary_template::summarize_options_for(backend.model, summary_template);
        let text = backend
            .summarizer
            .summarize(turns, &options)
            .await
            .map_err(|error| error.to_string())?;
        Ok((text, backend.provider_model))
    }
}

/// Reads all finalized segments for `session_id` from `store` and appends
/// whichever aren't already in `turns` (deduplicated via
/// `FinalizedTurns::insert_if_new`). Used both for the initial late-join load and
/// for lagged-recovery — both cases are "some finalized segments may already be
/// known, load everything and let dedup sort out what's new."
fn load_finalized_from_store(
    store: &SessionStore,
    session_id: SessionId,
    turns: &mut FinalizedTurns<TranscriptTurn>,
) {
    let segments = store
        .list_transcript_segments(session_id)
        .unwrap_or_default();
    for seg in &segments {
        if seg.is_final {
            let sid = transcript_event::segment_id_for_segment(
                seg.session_id,
                seg.track,
                seg.start_ms,
                seg.end_ms,
            );
            turns.insert_if_new(
                sid,
                TranscriptTurn {
                    speaker: Some(transcript::speaker_label(seg.track, seg.speaker)),
                    text: seg.text.clone(),
                },
            );
        }
    }
}

fn summary_build_error_message(error: BuildSummarizerError) -> &'static str {
    match error {
        BuildSummarizerError::MissingVertexCredentials => {
            "設定画面でGoogle Vertex AIの認証情報を設定してください"
        }
        BuildSummarizerError::MissingApiKey => "設定画面でAPIキーを設定してください",
        BuildSummarizerError::InvalidVertexCredentials => "認証情報の読み込みに失敗しました",
        BuildSummarizerError::Client => "AIクライアントの作成に失敗しました",
    }
}
