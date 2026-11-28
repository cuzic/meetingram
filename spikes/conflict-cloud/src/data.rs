//! Load synthetic meetings (utterances + hand-made gold graph) into a `Graph`.

use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{Context, Result};
use discussion_graph::{Edge, EdgeKind, Graph, Ground, Node, NodeKind, Review, Status, Utterance};
use serde::Deserialize;

pub const MEETINGS: [&str; 8] = [
    "m1",
    "m2",
    "m3",
    "budget",
    "design",
    "one-on-one",
    "long",
    "e2e-design",
];

#[derive(Deserialize)]
struct GoldNode {
    id: String,
    kind: NodeKind,
    text: String,
    #[serde(default)]
    status: Option<Status>,
    grounds: Vec<i64>,
    #[serde(default)]
    parent: Option<String>,
}

#[derive(Deserialize)]
struct GoldEdge {
    src: String,
    dst: String,
    kind: EdgeKind,
}

#[derive(Deserialize)]
struct Gold {
    nodes: Vec<GoldNode>,
    edges: Vec<GoldEdge>,
}

pub fn uid(row_id: i64) -> String {
    format!("u{row_id}")
}

pub fn load(data_dir: &Path, name: &str) -> Result<Graph> {
    let jsonl = fs::read_to_string(data_dir.join(format!("{name}.jsonl")))
        .with_context(|| format!("read utterances of {name}"))?;
    let mut utterances = BTreeMap::new();
    for line in jsonl.lines().filter(|line| !line.trim().is_empty()) {
        let mut value: serde_json::Value = serde_json::from_str(line)?;
        value
            .as_object_mut()
            .context("utterance must be an object")?
            .insert("uid".to_owned(), "".into());
        let mut utterance: Utterance = serde_json::from_value(value)?;
        utterance.uid = uid(utterance.row_id);
        utterances.insert(utterance.uid.clone(), utterance);
    }
    let gold: Gold = serde_json::from_str(&fs::read_to_string(
        data_dir.join(format!("{name}.gold.json")),
    )?)?;
    Ok(build_graph(utterances, gold))
}

fn build_graph(utterances: BTreeMap<String, Utterance>, gold: Gold) -> Graph {
    let mut graph = Graph {
        rev: 1,
        utterances,
        ..Graph::default()
    };
    let mut edges: Vec<(String, String, EdgeKind)> = Vec::new();
    for (index, node) in gold.nodes.into_iter().enumerate() {
        let rev = index as u64 + 1;
        for row in &node.grounds {
            graph.grounds.push(Ground {
                target: node.id.clone(),
                uid: uid(*row),
                act: None,
            });
        }
        if let Some(parent) = &node.parent {
            edges.push((node.id.clone(), parent.clone(), EdgeKind::SubIssueOf));
        }
        graph.nodes.insert(
            node.id.clone(),
            Node {
                nid: node.id,
                kind: node.kind,
                text: node.text,
                status: node.status.unwrap_or(Status::Open),
                merged_into: None,
                confidence: 1.0,
                review: Review::Auto,
                created_rev: rev,
                updated_rev: rev,
            },
        );
    }
    edges.extend(gold.edges.into_iter().map(|e| (e.src, e.dst, e.kind)));
    for (src, dst, kind) in edges {
        let eid = format!("{src}>{dst}:{kind}");
        graph.edges.insert(
            eid.clone(),
            Edge {
                eid,
                src,
                dst,
                kind,
                confidence: 1.0,
                review: Review::Auto,
                created_rev: 1,
            },
        );
    }
    graph
}
