use std::{sync::Arc, time::Duration};

use dioxus::prelude::*;
use recorder_domain::{CaptureState, SessionId};

use crate::{app_state::AppState, graph_consumer::GraphStatus};

use super::graph_view::{should_refresh_pending, GraphView};

fn is_recording(state: &AppState, session_id: SessionId) -> bool {
    if state
        .current
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|active| active.session_id == session_id)
    {
        return true;
    }
    !matches!(
        state.store.capture_state(session_id),
        Ok(CaptureState::Finalized | CaptureState::Failed { .. })
    )
}

fn graph_enabled(state: &AppState) -> bool {
    state
        .app_settings
        .lock()
        .unwrap()
        .graph_enabled
        .unwrap_or(true)
}

#[component]
pub(super) fn GraphStatusRow(session_id: SessionId, history: bool) -> Element {
    let state = use_context::<Arc<AppState>>();
    let mut snapshot = use_signal(|| (None::<Result<GraphStatus, ()>>, false));
    let mut refresh_pending = use_signal(|| false);
    let mut confirming = use_signal(|| false);

    use_future({
        let state = state.clone();
        move || {
            let state = state.clone();
            async move {
                let mut history_pending = None;
                loop {
                    let mut recording = !history || is_recording(&state, session_id);
                    let mut status = state.graph_consumer.status(session_id);
                    let refresh_requested = *refresh_pending.peek();
                    if refresh_requested {
                        refresh_pending.set(false);
                    }
                    let idle_history = should_refresh_pending(history, recording, &status, true);

                    // Refresh on entry and once a session becomes idle after an operation.
                    if !idle_history || refresh_requested {
                        history_pending = None;
                    }
                    if should_refresh_pending(
                        history,
                        recording,
                        &status,
                        history_pending.is_none(),
                    ) {
                        let pending = state.graph_consumer.pending_count(session_id).await;
                        recording = is_recording(&state, session_id);
                        status = state.graph_consumer.status(session_id);
                        if should_refresh_pending(history, recording, &status, true) {
                            history_pending = Some(pending);
                        }
                    }
                    let status = match history_pending {
                        Some(Ok(pending)) => {
                            status.pending = pending;
                            Ok(status)
                        }
                        Some(Err(_)) => Err(()),
                        None => Ok(status),
                    };
                    snapshot.set((Some(status), recording));
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
        }
    });

    let (status, recording) = snapshot();
    let status = match status {
        Some(Ok(status)) => status,
        Some(Err(())) => {
            return rsx! { p { class: "hint", "未抽出件数を取得できませんでした" } };
        }
        None => return rsx! { p { class: "hint", "議論グラフの状態を取得しています" } },
    };
    let view = GraphView::new(&status, recording, graph_enabled(&state));
    let label = view.label;
    let continue_state = state.clone();

    rsx! {
        div {
            p { class: "hint", "議論グラフ · {label}" }
            if history && !recording {
                button {
                    disabled: !view.can_continue || confirming(),
                    onclick: move |_| {
                        let status = continue_state.graph_consumer.status(session_id);
                        if !is_recording(&continue_state, session_id)
                            && graph_enabled(&continue_state)
                            && !status.running
                            && !status.rebuilding
                            && continue_state.graph_consumer.continue_extraction(session_id)
                        {
                            refresh_pending.set(true);
                            snapshot.set((Some(Ok(continue_state.graph_consumer.status(session_id))), false));
                        }
                    },
                    "続きを抽出"
                }
                button {
                    disabled: !view.can_rebuild || confirming(),
                    onclick: move |_| confirming.set(true),
                    "グラフを作り直す"
                }
                if confirming() {
                    div { role: "alert",
                        p { "グラフを作り直すと、承認・却下の結果も消えます。" }
                        button {
                            disabled: !view.can_rebuild,
                            onclick: move |_| {
                                let status = state.graph_consumer.status(session_id);
                                let view = GraphView::new(
                                    &status,
                                    is_recording(&state, session_id),
                                    graph_enabled(&state),
                                );
                                if confirming() && view.can_rebuild
                                    && state.graph_consumer.rebuild(session_id)
                                {
                                    refresh_pending.set(true);
                                    snapshot.set((Some(Ok(state.graph_consumer.status(session_id))), false));
                                }
                                confirming.set(false);
                            },
                            "作り直す"
                        }
                        button { onclick: move |_| confirming.set(false), "キャンセル" }
                    }
                }
            }
        }
    }
}
