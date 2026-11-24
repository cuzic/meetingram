//! Replays synthetic transcript rows on a shared recording clock.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use stt_api::{AudioChunk, SttError, SttEvent, SttProvider, SttSession, SttSessionConfig};
use tokio::sync::{mpsc, Notify};
use tokio::task::JoinHandle;
use tokio::time::Instant;

mod scripted_llm;
pub use scripted_llm::ScriptedSummarizer;

#[derive(Clone, Copy, Deserialize)]
enum Track {
    #[serde(rename = "自分")]
    SelfMic,
    #[serde(rename = "相手")]
    Remote,
}

impl Track {
    fn label(self) -> &'static str {
        match self {
            Self::SelfMic => "自分",
            Self::Remote => "相手",
        }
    }

    fn index(self) -> usize {
        match self {
            Self::SelfMic => 0,
            Self::Remote => 1,
        }
    }
}

#[derive(Deserialize)]
struct Utterance {
    row_id: i64,
    #[serde(rename = "speaker_label")]
    track: Track,
    start_ms: u64,
    end_ms: u64,
    text: String,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TourAction {
    LiveOutline,
    LiveEvidence,
    Consolidated,
    HistoryEvidence,
    DecisionHistory,
    Approve,
    Reject,
    GraphSettings,
    RebuildStarted,
    RebuildComplete,
    FirstSession,
    SecondSession,
}

#[derive(Deserialize)]
pub struct TourStep {
    pub name: String,
    pub action: TourAction,
}

#[derive(Deserialize)]
#[serde(default)]
pub struct Settings {
    pub speed: u64,
    pub latency_ms: u64,
    pub graph_batch_utterances: Option<usize>,
    pub graph_batch_seconds: Option<u64>,
    pub graph_review_threshold: Option<f64>,
    pub tour: Vec<TourStep>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            speed: 10,
            latency_ms: 300,
            graph_batch_utterances: None,
            graph_batch_seconds: None,
            graph_review_threshold: None,
            tour: Vec::new(),
        }
    }
}

pub struct Scenario {
    llm: Arc<scripted_llm::Script>,
    name: String,
    settings: Settings,
    rows: Arc<[Utterance]>,
}

impl Scenario {
    /// Loads a scenario only when MEETINGRAM_E2E_SCENARIO is set.
    pub fn from_environment() -> Result<Option<Self>, &'static str> {
        std::env::var_os("MEETINGRAM_E2E_SCENARIO")
            .as_deref()
            .map(|path| Self::load(Path::new(path)))
            .transpose()
    }

    pub fn load(directory: &Path) -> Result<Self, &'static str> {
        let name = directory
            .file_name()
            .ok_or("scenario directory must have a name")?;
        // Keep parser diagnostics out of app logs: they can include input text.
        let config = std::fs::read_to_string(directory.join("scenario.toml"))
            .map_err(|_| "cannot read scenario.toml")?;
        let settings: Settings = toml::from_str(&config).map_err(|_| "invalid scenario.toml")?;
        if settings.tour.iter().any(|step| {
            step.name.is_empty()
                || !step
                    .name
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        }) {
            return Err("invalid tour step name");
        }
        if settings.speed == 0 {
            return Err("scenario speed must be positive");
        }
        let input = std::fs::read_to_string(directory.join("utterances.jsonl"))
            .map_err(|_| "cannot read utterances.jsonl")?;
        let mut rows = input
            .lines()
            .map(serde_json::from_str::<Utterance>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "invalid utterances.jsonl")?;
        if rows.is_empty() || rows.iter().any(|row| row.start_ms > row.end_ms) {
            return Err("scenario must have utterances with valid timestamps");
        }
        rows.sort_by_key(|row| row.end_ms);
        Ok(Self {
            name: name.to_string_lossy().into_owned(),
            llm: Arc::new(scripted_llm::Script::load(directory)?),
            settings,
            rows: rows.into(),
        })
    }

    pub fn summarizer(&self) -> ScriptedSummarizer {
        ScriptedSummarizer::new(
            self.llm.clone(),
            self.rows.clone(),
            self.settings.latency_ms,
        )
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Each recording gets a new clock and cursor. Each reconnect reuses its provider.
    pub fn providers(&self) -> (Arc<ScriptedSttProvider>, Arc<ScriptedSttProvider>) {
        let playback = Arc::new(Playback {
            rows: self.rows.clone(),
            speed: self.settings.speed,
            state: Mutex::new(PlaybackState::default()),
            changed: Notify::new(),
        });
        (
            Arc::new(ScriptedSttProvider {
                playback: playback.clone(),
                track: Track::SelfMic,
            }),
            Arc::new(ScriptedSttProvider {
                playback,
                track: Track::Remote,
            }),
        )
    }
}

pub struct ScriptedSttProvider {
    playback: Arc<Playback>,
    track: Track,
}

#[derive(Default)]
struct PlaybackState {
    started: Option<Instant>,
    next: usize,
    senders: [Option<mpsc::UnboundedSender<SttEvent>>; 2],
    worker: Option<JoinHandle<()>>,
}

struct Playback {
    rows: Arc<[Utterance]>,
    speed: u64,
    state: Mutex<PlaybackState>,
    changed: Notify,
}

impl Playback {
    // One task dispatches both tracks so a late start or reconnect cannot
    // overtake a row waiting for its track's receiver.
    async fn run(self: Arc<Self>) {
        loop {
            let delay = {
                let mut state = self.state.lock().unwrap();
                let Some(row) = self.rows.get(state.next) else {
                    return;
                };
                match &state.senders[row.track.index()] {
                    Some(sender) if !sender.is_closed() => {
                        let end = Duration::from_millis(row.end_ms / self.speed);
                        let remaining = end.saturating_sub(state.started.unwrap().elapsed());
                        if remaining.is_zero() {
                            let event = SttEvent::FinalTranscript {
                                text: row.text.clone(),
                                words: None,
                                audio_start_ms: Some(row.start_ms / self.speed),
                                audio_end_ms: Some(row.end_ms / self.speed),
                                extra: Default::default(),
                            };
                            if sender.send(event).is_ok() {
                                state.next += 1;
                                continue;
                            }
                            None
                        } else {
                            Some(remaining)
                        }
                    }
                    _ => None,
                }
            };
            if let Some(delay) = delay {
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = self.changed.notified() => {}
                }
            } else {
                self.changed.notified().await;
            }
        }
    }
}

#[async_trait]
impl SttProvider for ScriptedSttProvider {
    async fn start_session(
        &self,
        config: SttSessionConfig,
    ) -> Result<(Box<dyn SttSession>, mpsc::UnboundedReceiver<SttEvent>), SttError> {
        if config.sample_rate_hz == 0 {
            return Err(SttError::PermanentError(
                "sample rate must be positive".into(),
            ));
        }
        let (sender, receiver) = mpsc::unbounded_channel();
        {
            let mut state = self.playback.state.lock().unwrap();
            state.started.get_or_insert_with(Instant::now);
            state.senders[self.track.index()] = Some(sender.clone());
            if state.worker.is_none() {
                state.worker = Some(tokio::spawn(self.playback.clone().run()));
            }
        }
        self.playback.changed.notify_one();
        Ok((
            Box::new(ScriptedSession {
                playback: self.playback.clone(),
                track: self.track,
                sender,
            }),
            receiver,
        ))
    }
}

struct ScriptedSession {
    playback: Arc<Playback>,
    track: Track,
    sender: mpsc::UnboundedSender<SttEvent>,
}

impl Drop for ScriptedSession {
    fn drop(&mut self) {
        let mut state = self.playback.state.lock().unwrap();
        let slot = &mut state.senders[self.track.index()];
        if slot
            .as_ref()
            .is_some_and(|sender| sender.same_channel(&self.sender))
        {
            *slot = None;
        }
        if state.senders.iter().all(Option::is_none) {
            if let Some(worker) = state.worker.take() {
                worker.abort();
            }
        }
    }
}

#[async_trait]
impl SttSession for ScriptedSession {
    async fn send_audio(&mut self, _chunk: AudioChunk<'_>) -> Result<(), SttError> {
        Ok(())
    }

    async fn finalize(self: Box<Self>) -> Result<(), SttError> {
        drop(self);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
