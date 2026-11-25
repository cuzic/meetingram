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
    /// The line to the parent. Only stances set it.
    #[serde(skip_serializing_if = "Option::is_none")]
    edge: Option<BranchEdge>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tags: Vec<Tag>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    children: Vec<Branch>,
}

#[derive(Serialize)]
struct BranchEdge {
    tone: &'static str,
    line: &'static str,
}

#[derive(Serialize)]
struct Tag {
    text: &'static str,
    tone: &'static str,
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
        edge: None,
        tags: vec![],
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
                edge: None,
                tags: vec![],
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
        } else if node.status == Status::Retracted {
            "撤回済み"
        } else if derived.unanswered.contains(&node.nid) {
            "未回答"
        } else {
            "検討中"
        }
    });
    let state = match node.kind {
        NodeKind::Issue => question.map(|text| {
            let tone = match text {
                "未回答" => "danger",
                "決定済み" => "accent",
                _ => "muted",
            };
            Tag { text, tone }
        }),
        NodeKind::Action => Some(Tag {
            text: match node.status {
                Status::Open => "未解決",
                Status::Resolved => "解決済み",
                Status::Decided => "決定済み",
                Status::Retracted => "撤回済み",
                _ => node.status.as_str(),
            },
            tone: "muted",
        }),
        _ => None,
    };
    // A low-confidence item is faded so that it is not emphasised, whatever its
    // kind (a doubtful decision must not stand out).
    let doubtful = node.review == Review::Auto && node.confidence < threshold;
    let tone = if doubtful {
        Some("muted")
    } else {
        match node.kind {
            NodeKind::Decision => Some("solid"),
            NodeKind::Action => Some("accent"),
            NodeKind::Issue if question == Some("未回答") => Some("danger"),
            _ => None,
        }
    };
    let (stance, edge) = match relation {
        Some(EdgeKind::Supports) => (
            Some(Tag {
                text: "賛成",
                tone: "accent",
            }),
            Some(BranchEdge {
                tone: "accent",
                line: "solid",
            }),
        ),
        Some(EdgeKind::ObjectsTo) => (
            Some(Tag {
                text: "反対",
                tone: "danger",
            }),
            Some(BranchEdge {
                tone: "danger",
                line: "dashed",
            }),
        ),
        _ => (None, None),
    };
    // At most two tags: the kind, then the question state or the stance.
    let tags = [
        Some(Tag {
            text: kind,
            tone: "muted",
        }),
        state.or(stance),
    ]
    .into_iter()
    .flatten()
    .collect();
    let sub = if reference { "参照" } else { "" }.to_string();
    let label = node
        .text
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    Branch {
        label,
        sub,
        tone,
        edge,
        tags,
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
                    "path" => &[
                        "d",
                        "fill",
                        "stroke",
                        "stroke-width",
                        "stroke-dasharray",
                        "stroke-linecap",
                    ][..],
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
                            "stroke-linecap" => attr.value() == "round",
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
            "<path stroke-linecap=\"square\"/>",
            "<path stroke-dasharray=\"url(#x)\"/>",
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
        assert!(json.contains("検討事項") && json.contains("\"tone\":\"muted\""));
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

    fn project_json(graph: &Graph) -> serde_json::Value {
        serde_json::to_value(project(graph, None, 0.6, String::new()).unwrap()).unwrap()
    }

    /// The tags of a node as "text:tone" strings.
    fn tags(json: &serde_json::Value, id: &str) -> Vec<String> {
        find(&json["root"], &format!("text {id}"))["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|tag| {
                format!(
                    "{}:{}",
                    tag["text"].as_str().unwrap(),
                    tag["tone"].as_str().unwrap()
                )
            })
            .collect()
    }

    fn tone<'a>(json: &'a serde_json::Value, id: &str) -> Option<&'a str> {
        find(&json["root"], &format!("text {id}"))["tone"].as_str()
    }

    #[test]
    fn tones_tags_and_edges_follow_kind_and_edges() {
        let json = project_json(&tones_graph());
        let node = |id: &str| find(&json["root"], &format!("text {id}")).clone();
        // A decision is solid; its tags carry no state.
        assert_eq!(tone(&json, "dec"), Some("solid"));
        assert_eq!(tags(&json, "dec"), ["決定:muted"]);
        // The stored status is "open", yet a live decision makes it decided.
        assert_eq!(tags(&json, "done"), ["問い:muted", "決定済み:accent"]);
        assert_eq!(tone(&json, "done"), None);
        // An unanswered question is danger.
        assert_eq!(tone(&json, "open"), Some("danger"));
        assert_eq!(tags(&json, "open"), ["問い:muted", "未回答:danger"]);
        // Stances: a tag and a line, no box tone, no state.
        assert_eq!(tone(&json, "pro"), None);
        assert_eq!(tone(&json, "con"), None);
        assert_eq!(tags(&json, "pro"), ["理由:muted", "賛成:accent"]);
        assert_eq!(tags(&json, "con"), ["理由:muted", "反対:danger"]);
        assert_eq!(
            node("pro")["edge"],
            serde_json::json!({"tone": "accent", "line": "solid"})
        );
        assert_eq!(
            node("con")["edge"],
            serde_json::json!({"tone": "danger", "line": "dashed"})
        );
        // Other nodes get no edge.
        for id in ["pos", "dec", "act", "done"] {
            assert!(node(id)["edge"].is_null(), "{id}");
        }
        // Positions are plain; homework is accented with its state tag.
        assert_eq!(tone(&json, "pos"), None);
        assert_eq!(tone(&json, "act"), Some("accent"));
        assert_eq!(tags(&json, "act"), ["宿題:muted", "未解決:muted"]);
        // The sub line is only for references and the root caption.
        assert_eq!(node("pos")["sub"], serde_json::Value::Null);
    }

    #[test]
    fn low_confidence_fades_the_box_whatever_its_kind() {
        let mut graph = tones_graph();
        for id in ["dec", "act", "open", "pro"] {
            graph.nodes.get_mut(id).unwrap().confidence = 0.1;
        }
        let json = project_json(&graph);
        // Faded, not emphasised; no review tag.
        for id in ["dec", "act", "open", "pro"] {
            assert_eq!(tone(&json, id), Some("muted"), "{id}");
        }
        assert_eq!(tags(&json, "open"), ["問い:muted", "未回答:danger"]);
        assert_eq!(tags(&json, "pro"), ["理由:muted", "賛成:accent"]);
        // The stance line does not depend on confidence.
        assert_eq!(
            find(&json["root"], "text pro")["edge"],
            serde_json::json!({"tone": "accent", "line": "solid"})
        );
        // A reviewed (approved) item is confident whatever its score.
        graph.nodes.get_mut("dec").unwrap().review = Review::Approved;
        assert_eq!(tone(&project_json(&graph), "dec"), Some("solid"));
    }

    #[test]
    fn answered_question_without_decision_is_under_discussion() {
        let mut graph = tones_graph();
        graph.edges.remove("dec-done");
        let json = project_json(&graph);
        assert_eq!(tags(&json, "done"), ["問い:muted", "検討中:muted"]);
        assert_eq!(tone(&json, "done"), None);
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
        let json = project_json(&graph);
        assert_eq!(tags(&json, "q"), ["問い:muted", "決定済み:accent"]);
        assert_eq!(tone(&json, "q"), None);
    }

    #[test]
    fn retracted_question_is_not_danger() {
        // m1: retracted comes before unanswered.
        let mut graph = Graph::default();
        graph
            .nodes
            .insert("q".into(), node("q", NodeKind::Issue, Status::Retracted));
        let json = project_json(&graph);
        assert_eq!(tags(&json, "q"), ["問い:muted", "撤回済み:muted"]);
        assert_eq!(tone(&json, "q"), None);
    }

    #[test]
    fn a_rejected_decision_does_not_decide_the_question() {
        let mut graph = tones_graph();
        graph.nodes.get_mut("dec").unwrap().review = Review::Rejected;
        let json = project_json(&graph);
        assert_eq!(tags(&json, "done"), ["問い:muted", "検討中:muted"]);
    }

    #[test]
    fn a_rejected_resolves_edge_does_not_decide_the_question() {
        // m2
        let mut graph = tones_graph();
        graph.edges.get_mut("dec-done").unwrap().review = Review::Rejected;
        let json = project_json(&graph);
        assert_eq!(tags(&json, "done"), ["問い:muted", "検討中:muted"]);
    }

    #[test]
    fn unattached_items_alone_can_exceed_the_limit() {
        // m2: the group header plus 99 unattached items exceed the budget of 99.
        let mut graph = Graph::default();
        for i in 0..99 {
            let id = format!("n{i:03}");
            graph
                .nodes
                .insert(id.clone(), node(&id, NodeKind::Action, Status::Open));
        }
        assert!(matches!(
            project(&graph, None, 0.6, String::new()),
            Err(ProjectionError::Limit)
        ));
        graph.nodes.remove("n098");
        assert!(project(&graph, None, 0.6, String::new()).is_ok());
    }
}
