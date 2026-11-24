use std::{sync::Arc, time::Duration};

use dioxus::prelude::*;
use discussion_graph::{
    ChangeState, DecisionChangeAction, DecisionDisplay, DecisionHistory, NodeKind,
};
use recorder_domain::SessionId;

use super::graph_outline_view::{evidence, evidence_view};
use crate::{app_state::AppState, graph_consumer::DecisionHistoryOperation};

pub(super) fn changes_view(
    history: &DecisionHistory,
    session_id: SessionId,
    generation: u64,
    can_review: bool,
) -> Element {
    rsx! {
        if can_review || !history.changes.is_empty() {
            section { aria_label: "決定・宿題の変更", "data-decision-changes": true,
                h3 { "決定・宿題の変更" }
                for (label, display) in [
                    ("現在の決定・宿題（変更確認済み）", DecisionDisplay::Current),
                    ("変更の確認待ち", DecisionDisplay::Pending),
                ] {
                    {outcomes(history, label, display)}
                }
                for change in &history.changes {
                    {change_view(history, change, if can_review {
                        rsx! { ReviewChange {
                            key: "{generation}-{history.graph.rev}-{change.id}-{change.state}",
                            session_id, generation, rev: history.graph.rev,
                            id: change.id, state: change.state,
                        } }
                    } else {
                        rsx! {}
                    })}
                }
                if can_review {
                    AddChange {
                        key: "{generation}", session_id, generation,
                        rev: history.graph.rev,
                        nodes: history.graph.nodes.values()
                            .filter(|node| node.is_live() && matches!(node.kind, NodeKind::Decision | NodeKind::Action))
                            .map(|node| (node.nid.clone(), node.kind, node.text.clone())).collect::<Vec<_>>(),
                        utterances: history.graph.utterances.values().map(|u| {
                            let mut text: String = u.text.chars().take(90).collect();
                            if text.len() < u.text.len() {
                                text.push('…');
                            }
                            (u.uid.clone(), u.row_id, format!("{} · {}", u.speaker_label, text))
                        }).collect::<Vec<_>>(),
                    }
                    if history.graph.nodes.values().any(|node| node.status == discussion_graph::Status::Merged
                        && matches!(node.kind, NodeKind::Decision | NodeKind::Action))
                    {
                        p { class: "hint", "以前の整理で統合された決定・宿題があります。変更前の項目が選べない場合は、グラフを作り直してください。" }
                    }
                }
            }
        }
    }
}

fn outcomes(history: &DecisionHistory, label: &str, display: DecisionDisplay) -> Element {
    let nodes: Vec<_> = history
        .graph
        .nodes
        .values()
        .filter(|node| node.is_live() && history.display(&node.nid) == display)
        .collect();
    rsx! {
        if !nodes.is_empty() {
            h4 { "{label}" }
            ul {
                for node in nodes {
                    li { key: "{node.nid}", "{node.text}" }
                }
            }
        }
    }
}

fn change_view(
    history: &DecisionHistory,
    change: &discussion_graph::DecisionChange,
    review: Element,
) -> Element {
    let graph = &history.graph;
    let previous = graph
        .nodes
        .get(&change.previous)
        .map(|node| node.text.as_str())
        .unwrap_or("表示できません");
    let replacement = change.replacement.as_deref().map(|id| {
        graph
            .nodes
            .get(id)
            .map(|node| node.text.as_str())
            .unwrap_or("表示できません")
    });
    let state = match change.state {
        ChangeState::Pending => "確認待ち",
        ChangeState::Approved => "承認済み",
        ChangeState::Rejected => "却下済み",
        ChangeState::Revoked => "承認取消済み",
        _ => "確認できません",
    };
    rsx! {
        details { key: "{change.id}", "data-change-state": change.state.as_str(),
            summary { "{state} · {previous}" }
            h4 { "変更前" }
            p { "{previous}" }
            {evidence_view(evidence(graph, &change.previous))}
            if let (Some(id), Some(text)) = (change.replacement.as_deref(), replacement) {
                h4 { "変更後" }
                p { "{text}" }
                {evidence_view(evidence(graph, id))}
            } else {
                p { "撤回" }
            }
            h4 { "変更を示す発話" }
            {evidence_view(graph.utterances.get(&change.evidence).into_iter().collect())}
            {review}
        }
    }
}

#[component]
fn AddChange(
    session_id: SessionId,
    generation: u64,
    rev: u64,
    nodes: Vec<(String, NodeKind, String)>,
    mut utterances: Vec<(String, i64, String)>,
) -> Element {
    let mut previous = use_signal(String::new);
    let mut replacement = use_signal(String::new);
    let mut ground = use_signal(String::new);
    let mut message = use_signal(|| None::<&'static str>);
    let mut busy = use_signal(|| false);
    let consumer = use_context::<Arc<AppState>>().graph_consumer.clone();
    utterances.sort_by_key(|(_, row, _)| *row);
    let previous_kind = nodes
        .iter()
        .find(|(id, _, _)| id == &previous())
        .map(|(_, kind, _)| *kind);
    if nodes.is_empty() || utterances.is_empty() {
        return rsx! {};
    }
    rsx! {
        details {
            summary { "変更を記録" }
            label { "変更前"
                select {
                    "data-change-field": "previous",
                    style: "display: block; width: 100%; max-width: 32rem;",
                    value: previous(),
                    onchange: move |event| {
                        previous.set(event.value());
                        replacement.set(String::new());
                    },
                    option { value: "", "決定・宿題を選択" }
                    for (id, _, text) in &nodes {
                        option { value: id.clone(), "{text}" }
                    }
                }
            }
            label { "変更後"
                select {
                    "data-change-field": "replacement",
                    style: "display: block; width: 100%; max-width: 32rem;",
                    value: replacement(),
                    onchange: move |event| replacement.set(event.value()),
                    option { value: "", "選択" }
                    option { value: "withdraw", "撤回" }
                    for (id, kind, text) in &nodes {
                        if Some(*kind) == previous_kind && *id != previous() {
                            option { value: id.clone(), "{text}" }
                        }
                    }
                }
            }
            label { "変更を示す発話"
                select {
                    "data-change-field": "evidence",
                    style: "display: block; width: 100%; max-width: 32rem;",
                    value: ground(),
                    onchange: move |event| ground.set(event.value()),
                    option { value: "", "発話を選択" }
                    for (id, _, text) in &utterances {
                        option { value: id.clone(), "{text}" }
                    }
                }
            }
            button {
                disabled: busy() || previous_kind.is_none() || replacement().is_empty() || ground().is_empty(),
                onclick: move |_| {
                    let consumer = consumer.clone();
                    let replacement_id = replacement();
                    let operation = DecisionHistoryOperation::Add {
                        previous: previous(),
                        replacement: (replacement_id != "withdraw").then_some(replacement_id),
                        evidence: ground(),
                    };
                    busy.set(true);
                    async move {
                        let result = consumer.update_decision_history(session_id, generation, rev, operation).await;
                        message.set(result.err().map(error_message));
                        if result.is_ok() {
                            previous.set(String::new());
                            replacement.set(String::new());
                            ground.set(String::new());
                        }
                        busy.set(false);
                    }
                },
                "確認候補を作成"
            }
            if let Some(message) = message() {
                p { role: "status", "{message}" }
            }
        }
    }
}

#[component]
fn ReviewChange(
    session_id: SessionId,
    generation: u64,
    rev: u64,
    id: u64,
    state: ChangeState,
) -> Element {
    let mut confirmed = use_signal(|| false);
    let mut message = use_signal(|| None::<&'static str>);
    let mut busy = use_signal(|| false);
    let consumer = use_context::<Arc<AppState>>().graph_consumer.clone();
    let submit = move |action| {
        let consumer = consumer.clone();
        busy.set(true);
        spawn(async move {
            let operation = DecisionHistoryOperation::Review { id, action };
            let result = consumer
                .update_decision_history(session_id, generation, rev, operation)
                .await;
            message.set(result.err().map(error_message));
            busy.set(false);
        });
    };
    let mut approve = submit.clone();
    let mut other = submit;
    rsx! {
        match state {
            ChangeState::Pending => rsx! {
                label {
                    input { r#type: "checkbox", checked: confirmed(), onchange: move |event| confirmed.set(event.checked()) }
                    "変更前の判断、変更後の判断、変更を示す発話が、同じ顧客・成果物などを指すことを確認した"
                }
                button { disabled: busy() || !confirmed(),
                    onclick: move |_| approve(DecisionChangeAction::Approve { same_subject: confirmed() }),
                    "承認"
                }
                button { disabled: busy(), onclick: move |_| other(DecisionChangeAction::Reject), "却下" }
            },
            ChangeState::Approved => rsx! {
                button { disabled: busy(), onclick: move |_| other(DecisionChangeAction::Revoke), "承認を取り消す" }
            },
            ChangeState::Rejected | ChangeState::Revoked => rsx! {
                button { disabled: busy(), onclick: move |_| other(DecisionChangeAction::Reopen), "再確認する" }
            },
            _ => rsx! {},
        }
        if let Some(message) = message() {
            p { role: "status", "{message}" }
        }
    }
}

fn error_message(error: &str) -> &'static str {
    match error {
        "review_stale" => "グラフが更新されました。根拠を確認し直してください。",
        "review_unavailable" => "現在は変更を確認できません。",
        "invalid_change" => {
            "関連する変更や根拠を確認してください。承認を取り消すときは後続の変更が先です。"
        }
        _ => "変更を保存できませんでした。",
    }
}

#[component]
pub(super) fn SummaryDecisionHistory(
    session_id: SessionId,
    summary: Option<(String, chrono::DateTime<chrono::Utc>)>,
) -> Element {
    let consumer = use_context::<Arc<AppState>>().graph_consumer.clone();
    let mut snapshot = use_signal(|| None::<DecisionHistory>);
    let mut load_failed = use_signal(|| false);
    use_future(move || {
        let consumer = consumer.clone();
        async move {
            let mut loaded = None;
            loop {
                let status = consumer.status(session_id);
                let current = (status.graph_generation, status.rev);
                if loaded != Some(current) {
                    if loaded.is_some_and(|(generation, _)| generation != current.0) {
                        snapshot.set(None);
                    }
                    match consumer.load_decision_history(session_id, false).await {
                        Ok((history, _)) => {
                            snapshot.set(Some(history));
                            load_failed.set(false);
                            loaded = Some(current);
                        }
                        Err(_) => {
                            snapshot.set(None);
                            load_failed.set(true);
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    });
    let history = snapshot.read();
    let has_changes = history
        .as_ref()
        .is_some_and(|history| !history.changes.is_empty());
    rsx! {
        if let Some(history) = history.as_ref() {
            {changes_view(history, session_id, 0, false)}
        }
        if let Some((text, generated_at)) = summary {
            if has_changes || load_failed() {
                details {
                    summary { "生成要約 · ", {generated_at.format("%Y-%m-%d %H:%M UTC").to_string()} }
                    if load_failed() {
                        p { role: "status", "決定・宿題の変更履歴を読み込めませんでした" }
                    } else {
                        p { class: "hint", "変更確認の結果は要約本文に反映されません" }
                    }
                    div { class: "summary-text", "{text}" }
                }
            } else if history.is_some() {
                div { class: "summary-text", "{text}" }
            }
        }
    }
}
