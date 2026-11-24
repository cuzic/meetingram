use discussion_graph::{
    build_outline, EdgeKind, Graph, Node, NodeKind, OutlineNode, Review, Status,
};
use serde::Serialize;

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
    let mut root = Branch {
        label: "議論の全体像".into(),
        sub: caption,
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
        root.children
            .push(project_branch(selected, 1, threshold, &mut remaining)?);
    } else {
        for entry in &outline.agendas {
            root.children
                .push(project_branch(entry, 1, threshold, &mut remaining)?);
        }
        if !outline.unattached.is_empty() {
            check_limit("未整理", 1, &mut remaining)?;
            let mut unattached = Branch {
                label: "未整理".into(),
                sub: String::new(),
                children: vec![],
            };
            for node in outline.unattached {
                check_limit(&node.text, 2, &mut remaining)?;
                unattached
                    .children
                    .push(node_branch(node, None, false, threshold));
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
    remaining: &mut usize,
) -> Result<Branch, ProjectionError> {
    check_limit(&entry.node.text, depth, remaining)?;
    let mut branch = node_branch(entry.node, entry.relation, entry.is_reference, threshold);
    if !entry.is_reference {
        for child in &entry.children {
            branch
                .children
                .push(project_branch(child, depth + 1, threshold, remaining)?);
        }
    }
    Ok(branch)
}

fn node_branch(node: &Node, relation: Option<EdgeKind>, reference: bool, threshold: f64) -> Branch {
    let kind = match node.kind {
        NodeKind::Issue => "問い",
        NodeKind::Position => "案",
        NodeKind::Argument => "理由",
        NodeKind::Decision => "決定",
        NodeKind::Action => "宿題",
        _ => node.kind.as_str(),
    };
    let status = match node.status {
        Status::Open => "未解決",
        Status::Resolved => "解決済み",
        Status::Decided => "決定済み",
        Status::Retracted => "撤回済み",
        _ => node.status.as_str(),
    };
    let stance = match relation {
        Some(EdgeKind::Supports) => Some("賛成"),
        Some(EdgeKind::ObjectsTo) => Some("反対"),
        _ => None,
    };
    let sub = [
        Some(kind),
        Some(status),
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
}
