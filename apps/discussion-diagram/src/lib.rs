use discussion_graph::{
    build_outline, detect_gaps, EdgeKind, Graph, Node, NodeKind, OutlineNode, Review, Status,
};
use serde::Serialize;
use std::collections::BTreeSet;

pub const MAX_SVG_BYTES: usize = 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum ProjectionError {
    Empty,
    UnknownAgenda,
    Limit,
}

#[derive(Serialize)]
pub struct Diagram {
    version: u8,
    root: Branch,
}

#[derive(Serialize)]
struct Branch {
    label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    sub: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tone: Option<&'static str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    children: Vec<Branch>,
}

pub fn agendas(graph: &Graph) -> Vec<(&str, &str)> {
    build_outline(graph)
        .agendas
        .into_iter()
        .map(|entry| (entry.node.nid.as_str(), entry.node.text.as_str()))
        .collect()
}

pub fn project(
    graph: &Graph,
    agenda: Option<&str>,
    threshold: f64,
    caption: String,
) -> Result<Diagram, ProjectionError> {
    let outline = build_outline(graph);
    let derived = Derived::new(graph);
    let mut root = Branch {
        label: "議論の全体像".into(),
        sub: caption,
        tone: None,
        children: vec![],
    };
    let mut remaining = 99; // The root is also a displayed item.
    if let Some(id) = agenda {
        let selected = outline
            .agendas
            .iter()
            .find(|entry| entry.node.nid == id)
            .ok_or(ProjectionError::UnknownAgenda)?;
        root.label = "選択した議題".into();
        root.children.push(project_branch(
            selected,
            1,
            threshold,
            &derived,
            &mut remaining,
        )?);
    } else {
        for entry in &outline.agendas {
            root.children.push(project_branch(
                entry,
                1,
                threshold,
                &derived,
                &mut remaining,
            )?);
        }
        if !outline.unattached.is_empty() {
            check_limit("未整理", 1, &mut remaining)?;
            let mut unattached = Branch {
                label: "未整理".into(),
                sub: String::new(),
                tone: None,
                children: vec![],
            };
            for node in outline.unattached {
                check_limit(&node.text, 2, &mut remaining)?;
                unattached
                    .children
                    .push(node_branch(node, None, false, threshold, &derived));
            }
            root.children.push(unattached);
        }
    }
    if root.children.is_empty() {
        return Err(ProjectionError::Empty);
    }
    let diagram = Diagram { version: 1, root };
    if serde_json::to_vec(&diagram).map_or(true, |json| json.len() > 128 * 1024) {
        return Err(ProjectionError::Limit);
    }
    Ok(diagram)
}

fn check_limit(label: &str, depth: usize, remaining: &mut usize) -> Result<(), ProjectionError> {
    if depth >= 16 || label.chars().count() > 240 {
        return Err(ProjectionError::Limit);
    }
    *remaining = remaining.checked_sub(1).ok_or(ProjectionError::Limit)?;
    Ok(())
}

fn project_branch(
    entry: &OutlineNode<'_>,
    depth: usize,
    threshold: f64,
    derived: &Derived,
    remaining: &mut usize,
) -> Result<Branch, ProjectionError> {
    check_limit(&entry.node.text, depth, remaining)?;
    let mut branch = node_branch(
        entry.node,
        entry.relation,
        entry.is_reference,
        threshold,
        derived,
    );
    if !entry.is_reference {
        for child in &entry.children {
            branch.children.push(project_branch(
                child,
                depth + 1,
                threshold,
                derived,
                remaining,
            )?);
        }
    }
    Ok(branch)
}

/// What the edges say about each issue, independent of its stored status.
struct Derived {
    unanswered: BTreeSet<String>,
    decided: BTreeSet<String>,
}

impl Derived {
    fn new(graph: &Graph) -> Self {
        let live = |nid: &str| graph.nodes.get(nid).is_some_and(Node::is_live);
        let decided = graph
            .edges
            .values()
            .filter(|edge| {
                edge.kind == EdgeKind::Resolves
                    && edge.review != Review::Rejected
                    && live(&edge.src)
                    && live(&edge.dst)
                    && graph.nodes[&edge.src].kind == NodeKind::Decision
            })
            .map(|edge| edge.dst.clone())
            .collect();
        Self {
            unanswered: detect_gaps(graph, false)
                .unanswered_issues
                .into_iter()
                .collect(),
            decided,
        }
    }
}

fn node_branch(
    node: &Node,
    relation: Option<EdgeKind>,
    reference: bool,
    threshold: f64,
    derived: &Derived,
) -> Branch {
    let kind = match node.kind {
        NodeKind::Issue => "問い",
        NodeKind::Position => "案",
        NodeKind::Argument => "理由",
        NodeKind::Decision => "決定",
        NodeKind::Action => "宿題",
        _ => node.kind.as_str(),
    };
    // Only questions and homework carry a state. A question's state and tone
    // come from one judgment over its edges (a live decision, an answer), not
    // from the stored status.
    let question = (node.kind == NodeKind::Issue).then(|| {
        if derived.decided.contains(&node.nid) {
            "決定済み"
        } else if derived.unanswered.contains(&node.nid) {
            "未回答"
        } else if node.status == Status::Retracted {
            "撤回済み"
        } else {
            "検討中"
        }
    });
    let status = match node.kind {
        NodeKind::Issue => question,
        NodeKind::Action => Some(match node.status {
            Status::Open => "未解決",
            Status::Resolved => "解決済み",
            Status::Decided => "決定済み",
            Status::Retracted => "撤回済み",
            _ => node.status.as_str(),
        }),
        _ => None,
    };
    let tone = match node.kind {
        NodeKind::Decision => Some("solid"),
        NodeKind::Action => Some("accent"),
        NodeKind::Issue if question == Some("未回答") => Some("danger"),
        _ => None,
    };
    let stance = match relation {
        Some(EdgeKind::Supports) => Some("賛成"),
        Some(EdgeKind::ObjectsTo) => Some("反対"),
        _ => None,
    };
    let sub = [
        Some(kind),
        status,
        stance,
        reference.then_some("参照"),
        (node.review == Review::Auto && node.confidence < threshold).then_some("要確認"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let label = node
        .text
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    Branch {
        label,
        sub,
        tone,
        children: vec![],
    }
}

// Only the fixed emitter's two literal rules are accepted. Updating the pinned
// renderer requires checking the actual output against this policy.
const FONT_RULES: &str = concat!(
    "text { font-family: 'Hiragino Sans','Hiragino Kaku Gothic ProN','Yu Gothic','YuGothic',Meiryo,'Noto Sans JP',sans-serif; }\n",
    "    .emoji { font-family: 'Noto Color Emoji','Segoe UI Emoji',sans-serif; }"
);
const SVG_NS: &str = "http://www.w3.org/2000/svg";

pub fn valid_svg(svg: &str) -> bool {
    if svg.len() > MAX_SVG_BYTES || svg.contains("<?") || svg.contains("<!") {
        return false;
    }
    let Ok(doc) = roxmltree::Document::parse(svg) else {
        return false;
    };
    let root = doc.root_element();
    root.has_tag_name((SVG_NS, "svg"))
        && root
            .descendants()
            .filter(|node| node.is_element())
            .all(|node| {
                if node.tag_name().namespace() != Some(SVG_NS) {
                    return false;
                }
                let allowed = match node.tag_name().name() {
                    "svg" if node == root => {
                        &["width", "height", "viewBox", "role", "aria-labelledby"][..]
                    }
                    "title" | "desc" => &["id"][..],
                    "style"
                        if node.text().is_some_and(|css| css.trim() == FONT_RULES)
                            && node.children().all(|child| child.is_text()) =>
                    {
                        &[][..]
                    }
                    "rect" => &[
                        "x",
                        "y",
                        "width",
                        "height",
                        "rx",
                        "fill",
                        "stroke",
                        "stroke-width",
                        "stroke-dasharray",
                    ][..],
                    "path" => &["d", "fill", "stroke", "stroke-width"][..],
                    "text" => &["x", "y", "text-anchor", "font-size", "font-weight", "fill"][..],
                    "tspan" => &["class"][..],
                    _ => return false,
                };
                node.attributes().all(|attr| {
                    attr.namespace().is_none()
                        && allowed.contains(&attr.name())
                        && match attr.name() {
                            "fill" | "stroke" => {
                                attr.value() == "none"
                                    || attr.value().strip_prefix('#').is_some_and(|hex| {
                                        hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit())
                                    })
                            }
                            "class" => attr.value() == "emoji",
                            "stroke-dasharray" => {
                                let value = attr.value();
                                (1..=8).contains(&value.len())
                                    && value.split(',').count() == 2
                                    && value.split(',').all(|part| {
                                        !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())
                                    })
                            }
                            _ => true,
                        }
                })
            })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_graph_is_not_exported() {
        assert!(matches!(
            project(&Graph::default(), None, 0.6, String::new()),
            Err(ProjectionError::Empty)
        ));
    }

    #[test]
    fn accepts_pinned_renderer_output() {
        assert!(valid_svg(include_str!(
            "../tests/fixtures/rendered-map.svg"
        )));
    }

    #[test]
    fn rejects_active_svg_and_accepts_inert_content() {
        let wrap = |body: &str| format!("<svg xmlns=\"{SVG_NS}\">{body}</svg>");
        assert!(valid_svg(&wrap(&format!(
            "<style>{FONT_RULES}</style><text x=\"7\">判断 &amp; 根拠</text>"
        ))));
        for fragment in [
            "<script/>",
            "<foreignObject/>",
            "<text onclick=\"x()\"/>",
            "<rect fill=\"url(https://example.com)\"/>",
            "<style>text { fill: \\75rl(x) }</style>",
            "<image href=\"https://example.com\"/>",
            "<rect stroke-dasharray=\"url(#x)\"/>",
            "<rect stroke-dasharray=\"6,4,2\"/>",
        ] {
            assert!(!valid_svg(&wrap(fragment)), "{fragment}");
        }
    }

    #[test]
    fn projects_a_selected_agenda_and_excludes_rejected_nodes() {
        let mut graph = Graph::default();
        for (id, text, review) in [
            ("a", "検討事項", Review::Auto),
            ("b", "却下した項目", Review::Rejected),
        ] {
            graph.nodes.insert(
                id.into(),
                Node {
                    nid: id.into(),
                    kind: NodeKind::Issue,
                    text: text.into(),
                    status: Status::Open,
                    merged_into: None,
                    confidence: 0.4,
                    review,
                    created_rev: 1,
                    updated_rev: 1,
                },
            );
        }
        assert_eq!(agendas(&graph), vec![("a", "検討事項")]);
        let json = serde_json::to_string(&project(&graph, Some("a"), 0.6, "rev 1".into()).unwrap())
            .unwrap();
        assert!(json.contains("検討事項") && json.contains("要確認"));
        assert!(!json.contains("却下した項目"));
        graph.nodes.get_mut("a").unwrap().text = "長".repeat(241);
        assert!(matches!(
            project(&graph, None, 0.6, String::new()),
            Err(ProjectionError::Limit)
        ));
    }

    fn node(id: &str, kind: NodeKind, status: Status) -> Node {
        Node {
            nid: id.into(),
            kind,
            text: format!("text {id}"),
            status,
            merged_into: None,
            confidence: 1.0,
            review: Review::Auto,
            created_rev: 1,
            updated_rev: 1,
        }
    }

    fn edge(src: &str, dst: &str, kind: EdgeKind) -> discussion_graph::Edge {
        discussion_graph::Edge {
            eid: format!("{src}-{dst}"),
            src: src.into(),
            dst: dst.into(),
            kind,
            confidence: 1.0,
            review: Review::Auto,
            created_rev: 1,
        }
    }

    fn tones_graph() -> Graph {
        let mut graph = Graph::default();
        for (id, kind, status) in [
            ("open", NodeKind::Issue, Status::Open),
            ("done", NodeKind::Issue, Status::Open),
            ("pos", NodeKind::Position, Status::Open),
            ("pro", NodeKind::Argument, Status::Open),
            ("con", NodeKind::Argument, Status::Open),
            ("dec", NodeKind::Decision, Status::Decided),
            ("act", NodeKind::Action, Status::Open),
        ] {
            graph.nodes.insert(id.into(), node(id, kind, status));
        }
        for (src, dst, kind) in [
            ("pos", "done", EdgeKind::RespondsTo),
            ("pro", "pos", EdgeKind::Supports),
            ("con", "pos", EdgeKind::ObjectsTo),
            ("dec", "done", EdgeKind::Resolves),
            ("act", "dec", EdgeKind::Assigns),
        ] {
            graph
                .edges
                .insert(format!("{src}-{dst}"), edge(src, dst, kind));
        }
        graph
    }

    fn find<'a>(branch: &'a serde_json::Value, label: &str) -> &'a serde_json::Value {
        if branch["label"] == label {
            return branch;
        }
        branch["children"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|child| find(child, label))
            .find(|found| !found.is_null())
            .unwrap_or(&serde_json::Value::Null)
    }

    #[test]
    fn tones_and_states_follow_kind_and_edges() {
        let graph = tones_graph();
        let json =
            serde_json::to_value(project(&graph, None, 0.6, String::new()).unwrap()).unwrap();
        let root = &json["root"];
        let sub = |id: &str| {
            find(root, &format!("text {id}"))["sub"]
                .as_str()
                .unwrap_or("")
        };
        let tone = |id: &str| find(root, &format!("text {id}"))["tone"].as_str();
        // A decision is solid and carries no state.
        assert_eq!(tone("dec"), Some("solid"));
        assert_eq!(sub("dec"), "決定");
        // The stored status is "open", yet a live decision makes it decided.
        assert_eq!(sub("done"), "問い · 決定済み");
        assert_eq!(tone("done"), None);
        // An unanswered question is danger.
        assert_eq!(tone("open"), Some("danger"));
        assert_eq!(sub("open"), "問い · 未回答");
        // Stances are told apart by their label, not a tone, and show no state.
        assert_eq!(tone("pro"), None);
        assert_eq!(tone("con"), None);
        assert_eq!(sub("pro"), "理由 · 賛成");
        assert_eq!(sub("con"), "理由 · 反対");
        // Positions are plain; homework is accented with its state.
        assert_eq!(tone("pos"), None);
        assert_eq!(tone("act"), Some("accent"));
        assert_eq!(sub("act"), "宿題 · 未解決");
    }

    #[test]
    fn answered_question_without_decision_is_under_discussion() {
        let mut graph = tones_graph();
        graph.edges.remove("dec-done");
        let json =
            serde_json::to_value(project(&graph, None, 0.6, String::new()).unwrap()).unwrap();
        let done = find(&json["root"], "text done");
        assert_eq!(done["sub"], "問い · 検討中");
        assert!(done["tone"].is_null());
    }

    #[test]
    fn decided_question_without_answer_is_not_danger() {
        // A confirmation question decided at once: a decision, no responds_to.
        let mut graph = Graph::default();
        graph
            .nodes
            .insert("q".into(), node("q", NodeKind::Issue, Status::Open));
        graph
            .nodes
            .insert("d".into(), node("d", NodeKind::Decision, Status::Decided));
        graph
            .edges
            .insert("d-q".into(), edge("d", "q", EdgeKind::Resolves));
        let json =
            serde_json::to_value(project(&graph, None, 0.6, String::new()).unwrap()).unwrap();
        let q = find(&json["root"], "text q");
        assert_eq!(q["sub"], "問い · 決定済み");
        assert!(q["tone"].is_null());
    }

    #[test]
    fn a_rejected_decision_does_not_decide_the_question() {
        let mut graph = tones_graph();
        graph.nodes.get_mut("dec").unwrap().review = Review::Rejected;
        let json =
            serde_json::to_value(project(&graph, None, 0.6, String::new()).unwrap()).unwrap();
        assert_eq!(find(&json["root"], "text done")["sub"], "問い");
    }
}
