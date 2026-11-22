use std::{sync::Arc, time::Duration};

use dioxus::prelude::*;
use discussion_graph::{
    build_outline, EdgeKind, Graph, Node, NodeKind, Outline, OutlineNode, Status, Utterance,
};
use recorder_domain::SessionId;

use crate::app_state::AppState;

enum OutlineRow<'a> {
    Unattached,
    Node {
        node: &'a Node,
        depth: usize,
        dimmed: bool,
        relation: Option<EdgeKind>,
    },
}

fn outline_rows<'a>(outline: &Outline<'a>, threshold: f64) -> Vec<OutlineRow<'a>> {
    fn append<'a>(
        rows: &mut Vec<OutlineRow<'a>>,
        branch: &OutlineNode<'a>,
        depth: usize,
        threshold: f64,
    ) {
        rows.push(OutlineRow::Node {
            node: branch.node,
            depth,
            dimmed: branch.node.confidence < threshold,
            relation: branch.relation,
        });
        for child in &branch.children {
            append(rows, child, depth + 1, threshold);
        }
    }

    let mut rows = Vec::new();
    for agenda in &outline.agendas {
        append(&mut rows, agenda, 0, threshold);
    }
    if !outline.unattached.is_empty() {
        rows.push(OutlineRow::Unattached);
        rows.extend(outline.unattached.iter().map(|node| OutlineRow::Node {
            node,
            depth: 0,
            dimmed: node.confidence < threshold,
            relation: None,
        }));
    }
    rows
}

fn evidence<'a>(graph: &'a Graph, nid: &str) -> Vec<&'a Utterance> {
    let mut utterances: Vec<_> = graph
        .grounds
        .iter()
        .filter(|ground| ground.target == nid)
        .filter_map(|ground| graph.utterances.get(&ground.uid))
        .collect();
    utterances.sort_by_key(|utterance| utterance.row_id);
    utterances.dedup_by_key(|utterance| utterance.row_id);
    utterances
}

fn kind_label(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Issue => "問い",
        NodeKind::Position => "案",
        NodeKind::Argument => "理由",
        NodeKind::Decision => "決定",
        NodeKind::Action => "宿題",
        _ => kind.as_str(),
    }
}

fn status_label(status: Status) -> &'static str {
    match status {
        Status::Open => "未解決",
        Status::Resolved => "解決済み",
        Status::Decided => "決定済み",
        Status::Retracted => "撤回済み",
        _ => status.as_str(),
    }
}

fn should_load(loaded: Option<(u64, u64)>, current: (u64, u64)) -> bool {
    loaded != Some(current)
}

#[component]
pub(super) fn GraphOutline(session_id: SessionId) -> Element {
    let state = use_context::<Arc<AppState>>();
    let consumer = state.graph_consumer.clone();
    let mut snapshot = use_signal(|| None::<Result<Graph, &'static str>>);
    let mut threshold = use_signal(|| consumer.review_threshold());
    let mut selected = use_signal(|| None::<String>);
    use_future(move || {
        let consumer = consumer.clone();
        async move {
            let mut loaded = None;
            loop {
                let status = consumer.status(session_id);
                let current = (status.rev, status.graph_generation);
                if should_load(loaded, current) {
                    let graph = consumer.load_graph(session_id).await;
                    if loaded.is_none_or(|(_, generation)| generation != current.1) {
                        selected.set(None);
                    }
                    snapshot.set(Some(graph));
                    loaded = Some(current);
                }
                let latest_threshold = consumer.review_threshold();
                if *threshold.peek() != latest_threshold {
                    threshold.set(latest_threshold);
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    });

    let snapshot = snapshot.read();
    let graph = match snapshot.as_ref() {
        None => return rsx! { p { class: "hint", "議論グラフを読み込んでいます" } },
        Some(Err(_)) => return rsx! { p { class: "hint", "議論グラフを読み込めませんでした" } },
        Some(Ok(graph)) => graph,
    };
    let rows = outline_rows(&build_outline(graph), threshold());
    let selected_id = selected();
    let selected_node = selected_id
        .as_deref()
        .and_then(|nid| graph.nodes.get(nid))
        .filter(|node| node.is_live());
    let utterances = selected_node.map(|node| evidence(graph, &node.nid));

    rsx! {
        section { class: "panel", role: "tabpanel", aria_label: "アウトライン",
            if rows.is_empty() {
                p { class: "hint", "表示できる議論グラフはありません" }
            }
            for (index, row) in rows.into_iter().enumerate() {
                match row {
                    OutlineRow::Unattached => rsx! { h3 { key: "{index}", "未整理" } },
                    OutlineRow::Node { node, depth, dimmed, relation } => {
                        let id = node.nid.clone();
                        let pressed = selected_id.as_ref() == Some(&id);
                        let indent = depth * 20;
                        let opacity = if dimmed { 0.6 } else { 1.0 };
                        let kind = kind_label(node.kind);
                        let status = status_label(node.status);
                        let stance = match relation {
                            Some(EdgeKind::Supports) => " · ＋賛成",
                            Some(EdgeKind::ObjectsTo) => " · −反対",
                            _ => "",
                        };
                        rsx! {
                            div { key: "{index}", style: "padding-inline-start: {indent}px;",
                                button {
                                    style: "opacity: {opacity}; text-align: start;",
                                    aria_pressed: pressed,
                                    onclick: move |_| selected.set(Some(id.clone())),
                                    "{kind}{stance} · {status} · {node.text}"
                                }
                            }
                        }
                    }
                }
            }
            if let Some(utterances) = utterances {
                section { aria_label: "根拠",
                    h3 { "根拠" }
                    if utterances.is_empty() {
                        p { class: "hint", "表示できる発話はありません" }
                    }
                    for utterance in utterances {
                        div { key: "{utterance.uid}",
                            p { class: "hint",
                                "{utterance.speaker_label}"
                                if let Some(ms) = utterance.start_ms.or(utterance.end_ms) {
                                    " · {super::format_elapsed(ms)}"
                                }
                            }
                            p { "{utterance.text}" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub(super) fn HistoryOutline(initial_session: Option<SessionId>) -> Element {
    let state = use_context::<Arc<AppState>>();
    let sessions = use_hook(move || {
        state
            .store
            .list_sessions()
            .map(|sessions| {
                sessions
                    .into_iter()
                    .map(|session| {
                        let started = session.started_at.with_timezone(&chrono::Local);
                        (
                            session.session_id,
                            started.format("%Y-%m-%d %H:%M:%S").to_string(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .map_err(|_| ())
    });
    let mut selected_session = use_signal(|| {
        sessions.as_ref().ok().and_then(|sessions| {
            initial_session
                .filter(|id| sessions.iter().any(|(session_id, _)| session_id == id))
                .or_else(|| sessions.first().map(|(session_id, _)| *session_id))
        })
    });
    let Ok(sessions) = sessions else {
        return rsx! { p { class: "hint", "セッション一覧を読み込めませんでした" } };
    };
    let Some(session_id) = selected_session() else {
        return rsx! { p { class: "hint", "記録されたセッションがありません" } };
    };

    rsx! {
        section { class: "history-container",
            label {
                "アウトラインを表示するセッション"
                select {
                    value: "{session_id}",
                    onchange: move |event| selected_session.set(event.value().parse().ok()),
                    for (id, started) in sessions {
                        option { key: "{id}", value: "{id}", "{started}" }
                    }
                }
            }
            GraphOutline { key: "{session_id}", session_id }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use discussion_graph::{Edge, Review};

    #[test]
    fn outline_lists_subquestions_first_and_unattached_last() {
        let mut graph = Graph::default();
        for (id, kind, status, review, confidence) in [
            ("n1", NodeKind::Issue, Status::Open, Review::Auto, 0.6),
            ("n2", NodeKind::Position, Status::Open, Review::Auto, 0.5),
            ("n3", NodeKind::Issue, Status::Open, Review::Auto, 0.9),
            ("n4", NodeKind::Argument, Status::Open, Review::Auto, 0.9),
            ("n5", NodeKind::Decision, Status::Merged, Review::Auto, 0.9),
            ("n6", NodeKind::Action, Status::Open, Review::Rejected, 0.9),
            ("n7", NodeKind::Action, Status::Open, Review::Auto, 0.9),
        ] {
            graph.nodes.insert(
                id.into(),
                Node {
                    nid: id.into(),
                    text: id.into(),
                    kind,
                    status,
                    review,
                    confidence,
                    merged_into: None,
                    created_rev: 1,
                    updated_rev: 1,
                },
            );
        }
        for (id, src, dst, kind) in [
            ("e1", "n2", "n1", EdgeKind::RespondsTo),
            ("e2", "n3", "n1", EdgeKind::SubIssueOf),
            ("e3", "n4", "n2", EdgeKind::ObjectsTo),
            ("e4", "n5", "n1", EdgeKind::Resolves),
        ] {
            graph.edges.insert(
                id.into(),
                Edge {
                    eid: id.into(),
                    src: src.into(),
                    dst: dst.into(),
                    kind,
                    confidence: 1.0,
                    review: Review::Auto,
                    created_rev: 1,
                },
            );
        }
        let actual: Vec<_> = outline_rows(&build_outline(&graph), 0.6)
            .into_iter()
            .map(|row| match row {
                OutlineRow::Unattached => ("未整理", 0, false, None),
                OutlineRow::Node {
                    node,
                    depth,
                    dimmed,
                    relation,
                } => (node.nid.as_str(), depth, dimmed, relation),
            })
            .collect();
        assert_eq!(
            actual,
            [
                ("n1", 0, false, None),
                ("n3", 1, false, Some(EdgeKind::SubIssueOf)),
                ("n2", 1, true, Some(EdgeKind::RespondsTo)),
                ("n4", 2, false, Some(EdgeKind::ObjectsTo)),
                ("未整理", 0, false, None),
                ("n7", 0, false, None),
            ]
        );
    }

    #[test]
    fn reloads_only_when_revision_changes_or_a_graph_is_rebuilt() {
        assert!(should_load(None, (0, 0)));
        assert!(!should_load(Some((0, 0)), (0, 0)));
        assert!(!should_load(Some((2, 0)), (2, 0)));
        assert!(should_load(Some((2, 0)), (3, 0)));
        assert!(should_load(Some((2, 0)), (2, 1)));
    }
}
