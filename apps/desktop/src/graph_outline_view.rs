use std::{sync::Arc, time::Duration};

use dioxus::prelude::*;
use discussion_graph::{
    build_outline, EdgeKind, Graph, Node, NodeKind, Outline, OutlineNode, Review, Status, Utterance,
};
use recorder_domain::SessionId;

use crate::app_state::AppState;

use super::graph_review_view::{candidates, review_view};

enum OutlineRow<'a> {
    Unattached,
    Node {
        node: &'a Node,
        depth: usize,
        dimmed: bool,
        relation: Option<EdgeKind>,
        is_reference: bool,
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
            dimmed: branch.node.review == Review::Auto && branch.node.confidence < threshold,
            relation: branch.relation,
            is_reference: branch.is_reference,
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
            dimmed: node.review == Review::Auto && node.confidence < threshold,
            relation: None,
            is_reference: false,
        }));
    }
    rows
}

pub(super) fn evidence<'a>(graph: &'a Graph, nid: &str) -> Vec<&'a Utterance> {
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

pub(super) fn kind_label(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Issue => "問い",
        NodeKind::Position => "案",
        NodeKind::Argument => "理由",
        NodeKind::Decision => "決定",
        NodeKind::Action => "宿題",
        _ => kind.as_str(),
    }
}

fn reference_label(is_reference: bool) -> &'static str {
    if is_reference {
        "↑ 上に出たもの · "
    } else {
        ""
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

async fn load_changed_graph<T>(
    loaded: &mut Option<(u64, u64, bool)>,
    current: (u64, u64, bool),
    load: impl std::future::Future<Output = Result<T, &'static str>>,
) -> Option<Result<T, &'static str>> {
    if *loaded == Some(current) {
        return None;
    }
    let result = load.await;
    if result.is_ok() {
        *loaded = Some(current);
    }
    Some(result)
}

fn next_retry_delay(delay: Duration) -> Duration {
    (delay * 2).clamp(Duration::from_secs(1), Duration::from_secs(5))
}

fn reset_on_generation_change(
    loaded: Option<(u64, u64, bool)>,
    generation: u64,
    clear: impl FnOnce(),
) -> bool {
    if loaded.is_some_and(|(_, previous, _)| previous != generation) {
        clear();
        return true;
    }
    false
}

#[component]
pub(super) fn GraphOutline(session_id: SessionId, #[props(default)] history: bool) -> Element {
    let state = use_context::<Arc<AppState>>();
    let consumer = state.graph_consumer.clone();
    let mut snapshot = use_signal(|| None);
    let mut review_enabled = use_signal(|| false);
    let stale_review = use_signal(|| false);
    let mut load_failed = use_signal(|| false);
    let mut threshold = use_signal(|| consumer.review_threshold());
    let mut selected = use_signal(|| None::<String>);
    use_future(move || {
        let consumer = consumer.clone();
        async move {
            let mut loaded = None;
            let mut retry_delay = Duration::ZERO;
            let mut retry_at = tokio::time::Instant::now();
            loop {
                let status = consumer.status(session_id);
                let enabled = history && status.review_allowed;
                // Review eligibility can change while the graph revision stays the same.
                let current = (status.rev, status.graph_generation, enabled);
                if reset_on_generation_change(loaded, current.1, || {
                    selected.set(None);
                    snapshot.set(None);
                }) {
                    loaded = None;
                    retry_at = tokio::time::Instant::now();
                    retry_delay = Duration::ZERO;
                }
                if *review_enabled.peek() != enabled {
                    review_enabled.set(enabled);
                }
                if tokio::time::Instant::now() >= retry_at {
                    match load_changed_graph(
                        &mut loaded,
                        current,
                        consumer.load_outline(session_id, history),
                    )
                    .await
                    {
                        Some(Ok((graph, review_available))) => {
                            let rows = if history {
                                candidates(&graph)
                            } else {
                                Vec::new()
                            };
                            snapshot.set(Some((graph, rows, current.1, review_available)));
                            load_failed.set(false);
                            retry_delay = Duration::ZERO;
                        }
                        Some(Err(_)) => {
                            if !*load_failed.peek() {
                                load_failed.set(true);
                            }
                            retry_delay = next_retry_delay(retry_delay);
                            retry_at = tokio::time::Instant::now() + retry_delay;
                        }
                        None => {}
                    }
                }
                let current_threshold = consumer.review_threshold();
                if *threshold.peek() != current_threshold {
                    threshold.set(current_threshold);
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    });

    let snapshot = snapshot.read();
    let error = load_failed().then(|| {
        rsx! {
            p { class: "hint", role: "status", "議論グラフを読み込めませんでした" }
        }
    });
    let Some((graph, candidates, generation, review_available)) = snapshot.as_ref() else {
        return error
            .unwrap_or_else(|| rsx! { p { class: "hint", "議論グラフを読み込んでいます" } });
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
            if let Some(error) = error {
                {error}
            }
            if rows.is_empty() {
                p { class: "hint", "表示できる議論グラフはありません" }
            }
            for (index, row) in rows.into_iter().enumerate() {
                match row {
                    OutlineRow::Unattached => rsx! { h3 { key: "{index}", "未整理" } },
                    OutlineRow::Node { node, depth, dimmed, relation, is_reference } => {
                        let id = node.nid.clone();
                        let pressed = selected_id.as_ref() == Some(&id);
                        let indent = depth * 20;
                        let opacity = if dimmed { 0.6 } else { 1.0 };
                        let kind = kind_label(node.kind);
                        let reference = reference_label(is_reference);
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
                                    "data-node-kind": if node.kind == discussion_graph::NodeKind::Issue { "issue" } else { "other" },
                                    onclick: move |_| selected.set(Some(id.clone())),
                                    "{reference}{kind}{stance} · {status} · {node.text}"
                                }
                            }
                        }
                    }
                }
            }
            if let Some(utterances) = utterances {
                {evidence_view(utterances)}
            }
            if history && stale_review() {
                p { class: "hint", role: "status", "一覧が更新されました" }
            }
            if history && *review_available && review_enabled() {
                {review_view(session_id, *generation, graph, candidates, threshold(), !load_failed(), stale_review)}
            }
        }
    }
}

pub(super) fn evidence_view(utterances: Vec<&Utterance>) -> Element {
    rsx! {
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
                    id: "history-outline-session",
                    value: "{session_id}",
                    onchange: move |event| selected_session.set(event.value().parse().ok()),
                    for (id, started) in sessions {
                        option { key: "{id}", value: "{id}", "{started}" }
                    }
                }
            }
            { history_outline_contents(session_id, rsx! { GraphOutline { session_id, history: true } }) }
        }
    }
}

fn history_outline_contents(session_id: SessionId, outline: Element) -> Element {
    // The key must be on the root of its own template for Dioxus to remount the outline.
    rsx! { Fragment { key: "{session_id}", {outline} } }
}

#[cfg(test)]
mod tests {
    use super::*;
    use discussion_graph::{Edge, Ground, Review};
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    #[test]
    fn switching_history_sessions_remounts_the_outline() {
        #[derive(Clone)]
        struct History {
            session: Rc<Cell<SessionId>>,
            mounts: Rc<RefCell<Vec<SessionId>>>,
        }

        #[component]
        fn Probe(session_id: SessionId) -> Element {
            let history = use_context::<History>();
            use_hook(move || history.mounts.borrow_mut().push(session_id));
            rsx! { "outline" }
        }

        fn app() -> Element {
            let session_id = use_context::<History>().session.get();
            rsx! {
                section {
                    label { "session picker" }
                    { history_outline_contents(session_id, rsx! { Probe { session_id } }) }
                }
            }
        }

        let first = SessionId::new();
        let second = SessionId::new();
        let history = History {
            session: Rc::new(Cell::new(first)),
            mounts: Rc::default(),
        };
        let mut dom = VirtualDom::new(app).with_root_context(history.clone());
        dom.rebuild_in_place();
        history.session.set(second);
        dom.mark_dirty(ScopeId::APP);
        dom.render_immediate_to_vec();

        assert_eq!(*history.mounts.borrow(), [first, second]);
    }

    #[test]
    fn evidence_is_filtered_sorted_and_deduplicated() {
        let mut graph = Graph::default();
        for (uid, row_id) in [("u1", 1), ("u2", 2), ("u3", 3)] {
            graph.utterances.insert(
                uid.into(),
                Utterance {
                    uid: uid.into(),
                    row_id,
                    track: None,
                    speaker_label: "Synthetic speaker".into(),
                    start_ms: None,
                    end_ms: None,
                    text: "Synthetic turn".into(),
                    is_retranscribed: false,
                },
            );
        }
        for (target, uid) in [
            ("n1", "u2"),
            ("n2", "u3"),
            ("n1", "u1"),
            ("n1", "u2"),
            ("n1", "missing"),
        ] {
            graph.grounds.push(Ground {
                target: target.into(),
                uid: uid.into(),
                act: None,
            });
        }
        let ids: Vec<_> = evidence(&graph, "n1")
            .into_iter()
            .map(|row| row.uid.as_str())
            .collect();
        assert_eq!(ids, ["u1", "u2"]);
    }

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
            ("n7", NodeKind::Action, Status::Open, Review::Approved, 0.5),
            (
                "n8",
                NodeKind::Decision,
                Status::Decided,
                Review::Approved,
                0.5,
            ),
            ("n9", NodeKind::Action, Status::Open, Review::Auto, 0.9),
            ("n10", NodeKind::Issue, Status::Open, Review::Auto, 0.9),
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
            ("e5", "n8", "n1", EdgeKind::Resolves),
            ("e6", "n9", "n1", EdgeKind::Assigns),
            ("e7", "n2", "n10", EdgeKind::RespondsTo),
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
                OutlineRow::Unattached => ("未整理", 0, false, None, ""),
                OutlineRow::Node {
                    node,
                    depth,
                    dimmed,
                    relation,
                    is_reference,
                } => (
                    node.nid.as_str(),
                    depth,
                    dimmed,
                    relation,
                    reference_label(is_reference),
                ),
            })
            .collect();
        assert_eq!(
            actual,
            [
                ("n1", 0, false, None, ""),
                ("n3", 1, false, Some(EdgeKind::SubIssueOf), ""),
                ("n2", 1, true, Some(EdgeKind::RespondsTo), ""),
                ("n4", 2, false, Some(EdgeKind::ObjectsTo), ""),
                ("n8", 1, false, Some(EdgeKind::Resolves), ""),
                ("n9", 1, false, Some(EdgeKind::Assigns), ""),
                ("n10", 0, false, None, ""),
                (
                    "n2",
                    1,
                    true,
                    Some(EdgeKind::RespondsTo),
                    "↑ 上に出たもの · "
                ),
                ("未整理", 0, false, None, ""),
                ("n7", 0, false, None, ""),
            ]
        );
    }

    #[test]
    fn outline_read_failures_back_off_to_five_seconds() {
        let mut delay = Duration::ZERO;
        for seconds in [1, 2, 4, 5, 5] {
            delay = next_retry_delay(delay);
            assert_eq!(delay, Duration::from_secs(seconds));
        }
    }

    #[tokio::test]
    async fn failed_read_after_rebuild_clears_the_old_outline_and_selection() {
        let mut loaded = Some((3, 0, true));
        let mut snapshot = Some(Graph::default());
        let mut selected = Some("n1");
        assert!(!reset_on_generation_change(loaded, 0, || {
            panic!("the current generation must retain its outline")
        }));
        if reset_on_generation_change(loaded, 1, || {
            snapshot = None;
            selected = None;
        }) {
            loaded = None;
        }
        let update =
            load_changed_graph::<Graph>(&mut loaded, (3, 1, false), async { Err("db") }).await;
        assert_eq!(update, Some(Err("db")));
        assert!(snapshot.is_none());
        assert!(selected.is_none());
    }

    #[tokio::test]
    async fn polls_reload_on_revision_generation_or_review_changes_and_retry_failed_reads() {
        let mut loaded = None;
        let reads = Cell::new(0);
        let load = async |rev, success| {
            reads.set(reads.get() + 1);
            if success {
                Ok(Graph {
                    rev,
                    ..Graph::default()
                })
            } else {
                Err("db")
            }
        };

        for (current, success, expected_reads) in [
            ((0, 0, true), true, 1),
            ((0, 0, true), true, 1),
            ((1, 0, true), false, 2),
            ((1, 0, true), true, 3),
            ((1, 0, true), true, 3),
            ((1, 1, true), true, 4),
            ((1, 1, false), true, 5),
            ((1, 1, false), true, 5),
            ((1, 1, true), true, 6),
        ] {
            let before = reads.get();
            let update = load_changed_graph(&mut loaded, current, load(current.0, success)).await;
            assert_eq!(reads.get(), expected_reads);
            if before == expected_reads {
                assert!(update.is_none());
            } else if success {
                assert_eq!(update.unwrap().unwrap().rev, current.0);
                assert_eq!(loaded, Some(current));
            } else {
                assert_eq!(update.unwrap(), Err("db"));
                assert_eq!(loaded, Some((0, 0, true)));
            }
        }
    }
}
