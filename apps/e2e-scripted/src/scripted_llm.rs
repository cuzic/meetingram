use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use discussion_graph::{EdgeKind, NodeKind, Prompt};
use serde::Deserialize;
use serde_json::{json, Value};
use summarize::{CompletionRequest, SummarizeOptions, Summarizer, SummarizerError, TranscriptTurn};

use crate::Utterance;

#[derive(Deserialize)]
struct GoldNode {
    id: String,
    kind: NodeKind,
    text: String,
    grounds: Vec<i64>,
    confidence: Option<f64>,
}

#[derive(Deserialize)]
struct GoldEdge {
    src: String,
    dst: String,
    kind: EdgeKind,
}

#[derive(Deserialize)]
struct GoldGraph {
    nodes: Vec<GoldNode>,
    edges: Vec<GoldEdge>,
}

pub(crate) struct Script {
    graph: GoldGraph,
    summary: String,
}

impl Script {
    pub(crate) fn load(directory: &Path) -> Result<Self, &'static str> {
        let graph = std::fs::read_to_string(directory.join("graph.json"))
            .map_err(|_| "cannot read graph.json")?;
        let graph: GoldGraph = serde_json::from_str(&graph).map_err(|_| "invalid graph.json")?;
        if graph.nodes.iter().any(|node| node.grounds.is_empty()) {
            return Err("gold nodes must have grounds");
        }
        let summary = std::fs::read_to_string(directory.join("summary.md"))
            .map_err(|_| "cannot read summary.md")?;
        Ok(Self { graph, summary })
    }
}

#[derive(Default)]
struct Matches {
    assigned: HashMap<String, i64>,
    consumed: HashSet<i64>,
}

pub struct ScriptedSummarizer {
    script: Arc<Script>,
    rows: Arc<[Utterance]>,
    latency: Duration,
    matches: Mutex<Matches>,
}

impl ScriptedSummarizer {
    pub(crate) fn new(script: Arc<Script>, rows: Arc<[Utterance]>, latency_ms: u64) -> Self {
        Self {
            script,
            rows,
            latency: Duration::from_millis(latency_ms),
            matches: Mutex::default(),
        }
    }

    fn increment(&self, user: &str) -> String {
        let Some(prompt) = Prompt::parse_incremental_user(user) else {
            tracing::info!(
                kind = "consolidation",
                matched = 0,
                operations = 0,
                "scripted LLM"
            );
            return json!({"ops": []}).to_string();
        };
        let mut matches = self.matches.lock().unwrap();
        let mut visible = HashMap::new();
        let mut new = HashMap::new();
        for (utterance, is_new) in prompt
            .reference
            .iter()
            .map(|u| (u, false))
            .chain(prompt.new.iter().map(|u| (u, true)))
        {
            let row_id = matches.assigned.get(&utterance.uid).copied().or_else(|| {
                let row = self
                    .rows
                    .iter()
                    .filter(|row| {
                        row.track.label() == utterance.speaker_label
                            && row.text == utterance.text
                            && !matches.consumed.contains(&row.row_id)
                    })
                    .min_by_key(|row| row.row_id)?;
                matches.assigned.insert(utterance.uid.clone(), row.row_id);
                matches.consumed.insert(row.row_id);
                Some(row.row_id)
            });
            if let Some(row_id) = row_id {
                visible.insert(row_id, &utterance.uid);
                if is_new {
                    new.insert(row_id, &utterance.uid);
                }
            }
        }
        drop(matches);

        let mut endpoints = HashMap::new();
        let mut ops = Vec::<Value>::new();
        for node in &self.script.graph.nodes {
            if let Some(existing) = prompt.context.iter().find(|entry| entry.text == node.text) {
                endpoints.insert(node.id.as_str(), (existing.nid.as_str(), None));
            } else if let Some(first_uid) = new.get(&node.grounds[0]) {
                let grounds: Vec<_> = node
                    .grounds
                    .iter()
                    .filter_map(|row| visible.get(row))
                    .map(|uid| json!({"uid": uid}))
                    .collect();
                ops.push(json!({
                    "op": "add_node", "tmp": node.id, "kind": node.kind,
                    "text": node.text, "grounds": grounds, "confidence": node.confidence.unwrap_or(0.8),
                }));
                endpoints.insert(node.id.as_str(), (node.id.as_str(), Some(*first_uid)));
            }
        }
        for edge in &self.script.graph.edges {
            if let (Some(&(src, src_uid)), Some(&(dst, dst_uid))) = (
                endpoints.get(edge.src.as_str()),
                endpoints.get(edge.dst.as_str()),
            ) {
                if let Some(uid) = src_uid.or(dst_uid) {
                    ops.push(json!({
                        "op": "add_edge", "src": src, "dst": dst, "kind": edge.kind,
                        "grounds": [{"uid": uid}], "confidence": 0.8,
                    }));
                }
            }
        }
        tracing::info!(
            kind = "incremental",
            matched = new.len(),
            operations = ops.len(),
            "scripted LLM"
        );
        json!({ "ops": ops }).to_string()
    }
}

#[async_trait::async_trait]
impl Summarizer for ScriptedSummarizer {
    async fn summarize(
        &self,
        _turns: &[TranscriptTurn],
        _options: &SummarizeOptions,
    ) -> Result<String, SummarizerError> {
        tokio::time::sleep(self.latency).await;
        tracing::info!(
            kind = "summary",
            matched = 0,
            operations = 0,
            "scripted LLM"
        );
        Ok(self.script.summary.clone())
    }

    async fn complete(&self, request: &CompletionRequest) -> Result<String, SummarizerError> {
        tokio::time::sleep(self.latency).await;
        Ok(self.increment(&request.user))
    }
}
