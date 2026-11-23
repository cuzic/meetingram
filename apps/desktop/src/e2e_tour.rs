use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use control_protocol::StatusDto;
use dioxus::prelude::*;
use e2e_scripted::{TourAction, TourStep};

use crate::app_state::AppState;

const STEP_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
type TourResult<T = ()> = Result<T, &'static str>;

pub(super) fn use_tour(state: Arc<AppState>) {
    use_future(move || {
        let state = state.clone();
        async move {
            let Some(scenario) = crate::E2E_SCENARIO.get().and_then(Option::as_ref) else {
                return;
            };
            let Some(directory) = std::env::var_os("MEETINGRAM_E2E_TOUR_DIR") else {
                return;
            };
            run(state, &PathBuf::from(directory), &scenario.settings().tour).await;
        }
    });
}

async fn run(state: Arc<AppState>, directory: &Path, steps: &[TourStep]) {
    let mut first = None;
    let mut second = None;
    for (index, step) in steps.iter().enumerate() {
        let result = async {
            execute(&state, step.action, &mut first, &mut second).await?;
            let number = index + 1;
            let ready = directory.join(format!("step-{number}-{}.ready", step.name));
            let shot = directory.join(format!("step-{number}.shot"));
            tokio::fs::write(ready, b"")
                .await
                .map_err(|_| "ready_write_failed")?;
            wait(STEP_TIMEOUT, || async {
                tokio::fs::try_exists(&shot)
                    .await
                    .ok()
                    .filter(|exists| *exists)
            })
            .await?;
            Ok::<_, &'static str>(())
        }
        .await;
        tracing::info!(
            step = step.name.as_str(),
            result = result.err().unwrap_or("ok"),
            "scripted tour"
        );
        if result.is_err() {
            return;
        }
    }
}

async fn wait<T, F: Future<Output = Option<T>>>(
    timeout: Duration,
    mut poll: impl FnMut() -> F,
) -> TourResult<T> {
    tokio::time::timeout(timeout, async {
        loop {
            if let Some(value) = poll().await {
                return value;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    })
    .await
    .map_err(|_| "timed_out")
}

async fn wait_status(
    state: &AppState,
    timeout: Duration,
    condition: impl Fn(&StatusDto) -> bool,
) -> TourResult<StatusDto> {
    wait(timeout, || async {
        let status = crate::control_server::status_dto(state).await;
        condition(&status).then_some(status)
    })
    .await
}

const HISTORY: &str = r#"button[title="過去のセッション"]"#;
const SETTINGS: &str = r#"button[title="設定"]"#;
const TABS: &str = r#"button[role="tab"]"#;
const OUTLINE: &str = r#"section[aria-label="アウトライン"]"#;
const NODES: &str = r#"section[aria-label="アウトライン"] button[aria-pressed]"#;
const EVIDENCE: &str = r#"section[aria-label="アウトライン"] > section[aria-label="根拠"]"#;
const REVIEW: &str = r#"section[aria-label="確認"]"#;

// DOM events use the same Dioxus callbacks as a click or a selection by hand.
async fn dom<T: serde::de::DeserializeOwned>(body: &str) -> Option<T> {
    let script = format!(
        r#"
        const visible = (selector) => Array.from(document.querySelectorAll(selector))
            .filter(element => element.getClientRects().length > 0);
        {body}
        "#
    );
    document::eval(&script).join().await.ok()
}

async fn click(selector: &str, label: &str) -> TourResult {
    let selector = serde_json::json!(selector);
    let label = serde_json::json!(label);
    let body = format!(
        r#"
        const button = visible({selector}).find(element => !element.disabled
            && (!{label} || element.textContent.trim() === {label}));
        if (!button) return false;
        button.scrollIntoView({{block: 'center'}});
        button.click();
        return true;
        "#
    );
    wait(STEP_TIMEOUT, || async {
        dom::<bool>(&body).await.filter(|done| *done)
    })
    .await?;
    Ok(())
}

async fn reveal(selector: &str) -> TourResult {
    let selector = serde_json::json!(selector);
    let body = format!("const element = visible({selector})[0]; if (!element) return false; element.scrollIntoView({{block: 'nearest'}}); return true;");
    wait(STEP_TIMEOUT, || async {
        dom::<bool>(&body).await.filter(|done| *done)
    })
    .await?;
    Ok(())
}

async fn select_session(selector: &str, id: &str) -> TourResult {
    let selector = serde_json::json!(selector);
    let id = serde_json::json!(id);
    let body = format!(
        r#"
        const select = visible({selector})[0];
        if (!select || !Array.from(select.options).some(option => option.value === {id})) return false;
        select.value = {id};
        select.dispatchEvent(new Event('change', {{bubbles: true}}));
        return true;
        "#
    );
    wait(STEP_TIMEOUT, || async {
        dom::<bool>(&body).await.filter(|done| *done)
    })
    .await?;
    Ok(())
}

const HISTORY_SELECTOR: &str = "#history-outline-session";
const OPERATIONS_SELECTOR: &str = "#history-graph-session";

async fn select_outline_session(id: &str) -> TourResult {
    select_session(HISTORY_SELECTOR, id).await?;
    let id = serde_json::json!(id);
    let body = format!(
        "return visible('[data-outline-session]').some(panel => panel.dataset.outlineSession === {id});"
    );
    wait(STEP_TIMEOUT, || async {
        dom::<bool>(&body).await.filter(|done| *done)
    })
    .await?;
    Ok(())
}

async fn history_outline(id: &str) -> TourResult {
    click(HISTORY, "").await?;
    click(TABS, "アウトライン").await?;
    select_outline_session(id).await?;
    reveal(OUTLINE).await
}

async fn review(state: &AppState, label: &str) -> TourResult {
    let buttons = format!("{REVIEW} button");
    let count_expression = format!("return visible({}).length;", serde_json::json!(buttons));
    let count = wait(STEP_TIMEOUT, || async {
        dom::<usize>(&count_expression)
            .await
            .filter(|count| *count > 2)
    })
    .await?;
    let rev = crate::control_server::status_dto(state)
        .await
        .graph
        .ok_or("graph_missing")?
        .rev;
    click(&buttons, label).await?;
    wait_status(state, STEP_TIMEOUT, |status| {
        status.graph.as_ref().is_some_and(|graph| graph.rev > rev)
    })
    .await?;
    wait(STEP_TIMEOUT, || async {
        dom::<usize>(&count_expression)
            .await
            .filter(|new_count| *new_count > 0 && *new_count < count)
    })
    .await?;
    reveal(&format!("{REVIEW} > h3")).await
}

async fn execute(
    state: &AppState,
    action: TourAction,
    first: &mut Option<String>,
    second: &mut Option<String>,
) -> TourResult {
    match action {
        TourAction::LiveOutline => {
            let status = wait_status(state, STEP_TIMEOUT, |status| {
                status.recording
                    && status.elapsed_ms >= 13_000
                    && status.graph.as_ref().is_some_and(|graph| graph.rev > 0)
            })
            .await?;
            *first = status.last_session_id;
            click(TABS, "アウトライン").await?;
            reveal(NODES).await?;
        }
        TourAction::LiveEvidence => {
            wait_status(state, Duration::from_secs(80), |status| {
                status.recording && status.elapsed_ms >= 60_000
            })
            .await?;
            click(NODES, "").await?;
            reveal(EVIDENCE).await?;
        }
        TourAction::Consolidated => {
            wait_status(state, Duration::from_secs(100), |status| {
                !status.recording
                    && status
                        .graph
                        .as_ref()
                        .is_some_and(|graph| graph.consolidated && !graph.running)
            })
            .await?;
        }
        TourAction::HistoryEvidence => {
            history_outline(first.as_deref().ok_or("first_session_missing")?).await?;
            click(
                r#"section[aria-label="アウトライン"] button[data-node-kind="issue"]"#,
                "",
            )
            .await?;
            reveal(EVIDENCE).await?;
        }
        TourAction::Approve => review(state, "承認").await?,
        TourAction::Reject => review(state, "却下").await?,
        TourAction::GraphSettings => {
            click("button", "← 戻る").await?;
            click(SETTINGS, "").await?;
            reveal("#graph-settings h2").await?;
        }
        TourAction::RebuildStarted => {
            click("button", "← 戻る").await?;
            click(HISTORY, "").await?;
            click(TABS, "グラフの操作").await?;
            select_session(
                OPERATIONS_SELECTOR,
                first.as_deref().ok_or("first_session_missing")?,
            )
            .await?;
            click("button", "グラフを作り直す").await?;
            let previous = crate::control_server::status_dto(state)
                .await
                .graph
                .ok_or("graph_missing")?
                .rev;
            click("button", "作り直す").await?;
            wait_status(state, STEP_TIMEOUT, |status| {
                status
                    .graph
                    .as_ref()
                    .is_some_and(|graph| graph.rev < previous && !graph.consolidated)
            })
            .await?;
        }
        TourAction::RebuildComplete => {
            wait_status(state, Duration::from_secs(90), |status| {
                status
                    .graph
                    .as_ref()
                    .is_some_and(|graph| graph.rev > 0 && graph.consolidated && !graph.running)
            })
            .await?;
            click(TABS, "アウトライン").await?;
            select_session(
                HISTORY_SELECTOR,
                first.as_deref().ok_or("first_session_missing")?,
            )
            .await?;
            reveal(NODES).await?;
        }
        TourAction::FirstSession => {
            let first = first.as_deref().ok_or("first_session_missing")?;
            let status = wait_status(state, Duration::from_secs(90), |status| {
                !status.recording
                    && status
                        .last_session_id
                        .as_deref()
                        .is_some_and(|id| id != first)
                    && status
                        .graph
                        .as_ref()
                        .is_some_and(|graph| graph.consolidated && !graph.running)
            })
            .await?;
            *second = status.last_session_id;
            // Reopen the outline so its session picker reloads the completed recordings.
            click(TABS, "グラフの操作").await?;
            reveal(OPERATIONS_SELECTOR).await?;
            click(TABS, "アウトライン").await?;
            select_outline_session(first).await?;
            reveal(NODES).await?;
            click(NODES, "").await?;
            reveal(EVIDENCE).await?;
        }
        TourAction::SecondSession => {
            select_session(
                HISTORY_SELECTOR,
                second.as_deref().ok_or("second_session_missing")?,
            )
            .await?;
            reveal(NODES).await?;
            let stale = dom::<bool>(&format!(
                "return visible({}).length > 0;",
                serde_json::json!(EVIDENCE)
            ))
            .await;
            if stale != Some(false) {
                return Err("previous_evidence_visible");
            }
            click(NODES, "").await?;
            reveal(EVIDENCE).await?;
        }
    }
    Ok(())
}
