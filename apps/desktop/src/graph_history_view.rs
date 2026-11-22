use std::sync::Arc;

use dioxus::prelude::*;
use recorder_domain::SessionId;

use crate::app_state::AppState;

use super::graph_status_view::GraphStatusRow;

#[component]
pub(super) fn GraphHistory(initial_session: Option<SessionId>) -> Element {
    let state = use_context::<Arc<AppState>>();
    let sessions = use_hook(|| {
        state
            .store
            .list_sessions()
            .unwrap_or_default()
            .into_iter()
            .map(|session| {
                let date = session.started_at.with_timezone(&chrono::Local);
                (
                    session.session_id,
                    date.format("%Y-%m-%d %H:%M:%S").to_string(),
                )
            })
            .collect::<Vec<_>>()
    });
    let mut selected = use_signal(|| {
        initial_session
            .filter(|id| sessions.iter().any(|(session_id, _)| session_id == id))
            .or_else(|| sessions.first().map(|(id, _)| *id))
    });
    if sessions.is_empty() {
        return rsx! {};
    }
    let selected_session = selected()
        .and_then(|id| sessions.iter().find(|(session_id, _)| *session_id == id))
        .cloned();
    let selected_value = selected_session
        .as_ref()
        .map(|(id, _)| id.to_string())
        .unwrap_or_default();

    rsx! {
        section { class: "history-container",
            div { class: "history-list",
                h2 { "議論グラフ" }
                label {
                    "グラフを操作するセッション"
                    select {
                        value: "{selected_value}",
                        onchange: move |event| selected.set(event.value().parse().ok()),
                        for (id, date) in sessions {
                            option { key: "{id}", value: "{id}", "{date}" }
                        }
                    }
                }
                if let Some((session_id, date)) = selected_session {
                    GraphStatusRow { key: "{session_id}", session_id, session_label: date }
                }
            }
        }
    }
}
