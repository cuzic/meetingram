use std::sync::Arc;

use dioxus::prelude::*;
use discussion_graph::{
    store::{ReviewDecision, ReviewTarget},
    EdgeKind, Graph, Node, Review,
};
use recorder_domain::SessionId;

use crate::app_state::AppState;

use super::graph_outline_view::{evidence, evidence_view, kind_label};

#[derive(PartialEq)]
pub(super) struct Candidate {
    id: String,
    confidence: f64,
    is_node: bool,
}

impl Candidate {
    fn target(&self) -> ReviewTarget<'_> {
        if self.is_node {
            ReviewTarget::Node(&self.id)
        } else {
            ReviewTarget::Edge(&self.id)
        }
    }
}

pub(super) fn candidates(graph: &Graph) -> Vec<Candidate> {
    let nodes = graph
        .nodes
        .values()
        .filter(|node| node.is_live() && node.review == Review::Auto);
    let edges = graph.edges.values().filter(|edge| {
        edge.review == Review::Auto
            && graph.nodes.get(&edge.src).is_some_and(Node::is_live)
            && graph.nodes.get(&edge.dst).is_some_and(Node::is_live)
    });
    nodes
        .map(|node| Candidate {
            id: node.nid.clone(),
            confidence: node.confidence,
            is_node: true,
        })
        .chain(edges.map(|edge| Candidate {
            id: edge.eid.clone(),
            confidence: edge.confidence,
            is_node: false,
        }))
        .collect()
}

fn visible_candidates(
    candidates: &[Candidate],
    threshold: f64,
    ready: bool,
) -> impl Iterator<Item = &Candidate> {
    candidates
        .iter()
        .filter(move |row| ready && row.confidence < threshold)
}

fn edge_kind_label(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::RespondsTo => "問いへの案",
        EdgeKind::Supports => "＋賛成",
        EdgeKind::ObjectsTo => "−反対",
        EdgeKind::Resolves => "決定",
        EdgeKind::Assigns => "宿題",
        EdgeKind::SubIssueOf => "細かな問い",
        _ => "関連",
    }
}

pub(super) fn review_view(
    session_id: SessionId,
    generation: u64,
    graph: &Graph,
    candidates: &[Candidate],
    threshold: f64,
    enabled: bool,
    stale_review: Signal<bool>,
) -> Element {
    rsx! {
        ReviewPanel {
            session_id,
            generation,
            rev: graph.rev,
            enabled,
            stale_review,
            rows: visible_candidates(candidates, threshold, true).map(|row| {
                let (key, label) = match row.target() {
                    ReviewTarget::Node(id) => {
                        let node = &graph.nodes[id];
                        (format!("node-{id}"), format!("{} · {}", kind_label(node.kind), node.text))
                    }
                    ReviewTarget::Edge(id) => {
                        let edge = &graph.edges[id];
                        (format!("edge-{id}"), format!("{} → {} · {}", graph.nodes[&edge.src].text,
                            graph.nodes[&edge.dst].text, edge_kind_label(edge.kind)))
                    }
                };
                ReviewRow { key, label, id: row.id.clone(), is_node: row.is_node,
                    grounds: evidence_view(evidence(graph, &row.id)) }
            }).collect(),
        }
    }
}

#[derive(Clone, PartialEq)]
struct ReviewRow {
    key: String,
    label: String,
    id: String,
    is_node: bool,
    grounds: Element,
}

#[component]
fn ReviewPanel(
    session_id: SessionId,
    generation: u64,
    rev: u64,
    enabled: bool,
    mut stale_review: Signal<bool>,
    rows: Vec<ReviewRow>,
) -> Element {
    let consumer = use_context::<Arc<AppState>>().graph_consumer.clone();
    let mut saving = use_signal(|| false);
    let mut failed = use_signal(|| false);
    let mut submitted = use_signal(|| None);
    let revision = (generation, rev);
    let save = move |(id, is_node, decision): (String, bool, ReviewDecision)| {
        let status = consumer.status(session_id);
        if !enabled
            || !status.review_allowed
            || *saving.peek()
            || *submitted.peek() == Some(revision)
        {
            return;
        }
        saving.set(true);
        failed.set(false);
        stale_review.set(false);
        let consumer = consumer.clone();
        let request = tokio::spawn(async move {
            let target = if is_node {
                ReviewTarget::Node(&id)
            } else {
                ReviewTarget::Edge(&id)
            };
            consumer
                .set_review(session_id, generation, rev, target, decision)
                .await
        });
        spawn(async move {
            match request.await {
                Ok(Ok(())) => submitted.set(Some(revision)),
                Ok(Err("review_stale")) => {
                    submitted.set(Some(revision));
                    stale_review.set(true);
                }
                _ => failed.set(true),
            }
            saving.set(false);
        });
    };

    let disabled = !enabled || saving() || submitted() == Some(revision);
    rsx! {
        section { aria_label: "確認",
            h3 { "確認" }
            if failed() {
                p { class: "hint", role: "status", "確認結果を保存できませんでした" }
            }
            if rows.is_empty() {
                p { class: "hint", "確認する項目はありません" }
            }
            for row in rows {
                {
                    let approve = (row.id.clone(), row.is_node, ReviewDecision::Approve);
                    let reject = (row.id, row.is_node, ReviewDecision::Reject);
                    let mut approve_save = save.clone();
                    let mut reject_save = save.clone();
                    rsx! {
                        div { key: "{row.key}",
                            details {
                                summary { "{row.label}" }
                                {row.grounds}
                            }
                            button { disabled, onclick: move |_| approve_save(approve.clone()), "承認" }
                            button { disabled, onclick: move |_| reject_save(reject.clone()), "却下" }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use discussion_graph::{Edge, EdgeKind, NodeKind, Status};

    #[test]
    fn candidates_require_live_automatic_items_strictly_below_threshold() {
        let mut graph = Graph::default();
        for (id, status, review, confidence) in [
            ("low", Status::Open, Review::Auto, 0.5),
            ("equal", Status::Open, Review::Auto, 0.6),
            ("high", Status::Open, Review::Auto, 0.9),
            ("merged", Status::Merged, Review::Auto, 0.1),
            ("approved", Status::Open, Review::Approved, 0.1),
            ("rejected", Status::Open, Review::Rejected, 0.1),
        ] {
            graph.nodes.insert(
                id.into(),
                Node {
                    nid: id.into(),
                    kind: NodeKind::Issue,
                    text: "Synthetic issue".into(),
                    status,
                    review,
                    confidence,
                    merged_into: None,
                    created_rev: 1,
                    updated_rev: 1,
                },
            );
        }
        for (id, src, review, confidence) in [
            ("low", "low", Review::Auto, 0.5),
            ("equal", "equal", Review::Auto, 0.6),
            ("high", "high", Review::Auto, 0.9),
            ("dead-end", "rejected", Review::Auto, 0.1),
            ("approved", "low", Review::Approved, 0.1),
            ("rejected", "low", Review::Rejected, 0.1),
        ] {
            graph.edges.insert(
                id.into(),
                Edge {
                    eid: id.into(),
                    src: src.into(),
                    dst: "high".into(),
                    kind: EdgeKind::SubIssueOf,
                    confidence,
                    review,
                    created_rev: 1,
                },
            );
        }
        let rows = candidates(&graph);
        assert_eq!(
            visible_candidates(&rows, 0.6, true)
                .map(Candidate::target)
                .collect::<Vec<_>>(),
            [ReviewTarget::Node("low"), ReviewTarget::Edge("low")]
        );
        assert_eq!(visible_candidates(&rows, 0.6, false).count(), 0);
    }
}
