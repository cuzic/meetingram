use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use dioxus::prelude::*;
use recorder_domain::{CaptureState, SessionId};

use crate::{app_state::AppState, graph_consumer::GraphStatus};

use super::graph_view::{
    continue_is_pending, rebuild_confirmation, should_read_capture_state, should_refresh_pending,
    GraphView, HistoryPending,
};

fn is_current_recording(state: &AppState, session_id: SessionId) -> bool {
    state
        .current
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|active| active.session_id == session_id)
}

fn graph_enabled(state: &AppState) -> bool {
    state.graph_configured()
}

#[component]
pub(super) fn GraphStatusRow(session_id: SessionId, session_label: Option<String>) -> Element {
    let state = use_context::<Arc<AppState>>();
    let history = session_label.is_some();
    let mut snapshot = use_signal(|| None::<(GraphStatus, bool, bool)>);
    let mut refresh_requested = use_signal(|| false);
    let mut continue_request = use_signal(|| None::<(Instant, GraphStatus)>);
    let mut confirming = use_signal(|| false);

    use_future({
        let state = state.clone();
        move || {
            let state = state.clone();
            async move {
                let mut history_pending = HistoryPending::default();
                let mut capture_recording = None;
                let mut was_busy = false;
                let mut stopped = None;
                let mut refresh_pending = true;
                loop {
                    let current_recording = history && is_current_recording(&state, session_id);
                    // The stop event arrives after capture finalization, even with the graph disabled.
                    if current_recording && stopped.is_none() {
                        stopped = Some(
                            state
                                .broker
                                .subscribe(&local_broker::session_stopped_subject(session_id)),
                        );
                    }
                    if stopped
                        .as_mut()
                        .is_some_and(|receiver| receiver.try_recv().is_ok())
                    {
                        stopped = None;
                        refresh_requested.set(true);
                    }

                    let mut status = state.graph_consumer.status(session_id);
                    // Compare consumer status before substituting the history count for display.
                    let continue_finished =
                        continue_request
                            .peek()
                            .as_ref()
                            .is_some_and(|(started, previous)| {
                                !continue_is_pending(&status, previous, started.elapsed())
                            });
                    if continue_finished {
                        continue_request.set(None);
                        refresh_requested.set(true);
                    }

                    let busy = current_recording || status.running || status.rebuilding;
                    if should_read_capture_state(
                        history,
                        capture_recording.is_some(),
                        was_busy,
                        busy,
                        *refresh_requested.peek(),
                    ) {
                        capture_recording = Some(!matches!(
                            state.store.capture_state(session_id),
                            Ok(CaptureState::Finalized | CaptureState::Failed { .. })
                        ));
                        refresh_requested.set(false);
                        refresh_pending = true;
                    }
                    was_busy = busy;

                    let mut recording =
                        !history || current_recording || capture_recording == Some(true);
                    if should_refresh_pending(history, recording, &status, refresh_pending) {
                        history_pending
                            .update(state.graph_consumer.pending_count(session_id).await);
                        recording |= is_current_recording(&state, session_id);
                        status = state.graph_consumer.status(session_id);
                        refresh_pending = false;
                    }
                    if should_refresh_pending(history, recording, &status, true) {
                        if let Some(count) = history_pending.count {
                            status.pending = count;
                        }
                    }
                    snapshot.set(Some((status, recording, history_pending.failed)));
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
        }
    });

    let Some((status, recording, pending_failed)) = snapshot() else {
        return rsx! { p { class: "hint", "議論グラフの状態を取得しています" } };
    };
    let view = GraphView::new(
        &status,
        recording,
        graph_enabled(&state),
        continue_request().is_some(),
    );
    let label = view.label;
    let continue_state = state.clone();

    rsx! {
        div {
            p { class: "hint", "議論グラフ · {label}" }
            if history && !recording {
                if pending_failed {
                    p { class: "hint", "未抽出件数を取得できませんでした" }
                }
                button {
                    disabled: !view.can_continue || confirming(),
                    onclick: move |_| {
                        let status = continue_state.graph_consumer.status(session_id);
                        if view.can_continue
                            && !confirming()
                            && continue_request.peek().is_none()
                            && !is_current_recording(&continue_state, session_id)
                            && graph_enabled(&continue_state)
                            && !status.running
                            && !status.rebuilding
                            && continue_state.graph_consumer.continue_extraction(session_id)
                        {
                            continue_request.set(Some((Instant::now(), status)));
                        }
                    },
                    "続きを抽出"
                }
                button {
                    disabled: !view.can_rebuild || confirming(),
                    onclick: move |_| confirming.set(true),
                    "グラフを作り直す"
                }
                if let Some(reason) = view.action_reason {
                    span { class: "hint", "{reason}" }
                }
                if let Some(session_label) = session_label.filter(|_| confirming()) {
                    div { role: "alert",
                        p { "{rebuild_confirmation(&session_label)}" }
                        button {
                            disabled: !view.can_rebuild,
                            onclick: move |_| {
                                let status = state.graph_consumer.status(session_id);
                                let view = GraphView::new(
                                    &status,
                                    recording || is_current_recording(&state, session_id),
                                    graph_enabled(&state),
                                    continue_request.peek().is_some(),
                                );
                                if confirming() && view.can_rebuild
                                    && state.graph_consumer.rebuild(session_id)
                                {
                                    refresh_requested.set(true);
                                    snapshot.set(Some((state.graph_consumer.status(session_id), false, pending_failed)));
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
