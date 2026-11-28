//! Write calls.jsonl, SUMMARY.md and REVIEW.md from the per-call files.

use std::{fmt::Write as _, fs, path::Path};

use anyhow::Result;
use discussion_graph::{Basis, CloudDiagram, CloudElement, Graph};

use crate::run::{call_dir, safe, Call, Plan, MODELS, RUNS};

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1]
}

fn basis(b: Basis) -> &'static str {
    match b {
        Basis::Stated => "stated",
        Basis::Inferred => "inferred",
    }
}

pub fn write_all(out: &Path, plan: &Plan, calls: &[Call]) -> Result<()> {
    for model in MODELS {
        let mut lines = String::new();
        for call in calls.iter().filter(|c| c.model == model) {
            lines.push_str(&serde_json::to_string(call)?);
            lines.push('\n');
        }
        fs::create_dir_all(out.join(model))?;
        fs::write(out.join(model).join("calls.jsonl"), lines)?;
    }
    for item in &plan.items {
        let dir = out.join("prompts").join(&item.meeting);
        fs::create_dir_all(&dir)?;
        fs::write(
            dir.join(format!("{}.txt", safe(&item.candidate.subject()))),
            &item.prompt.user,
        )?;
    }
    fs::write(out.join("SUMMARY.md"), summary(plan, calls))?;
    fs::write(out.join("REVIEW.md"), review(out, plan))?;
    Ok(())
}

pub fn summary(plan: &Plan, calls: &[Call]) -> String {
    let mut s = String::from("# Conflict cloud spike: summary\n\n");
    s.push_str(
        "Input: hand-made gold graphs (synthetic), not post flush+consolidate graphs, to isolate the cloud \
         prompt from extraction noise. Each candidate uses its default position pair.\n\n",
    );
    s.push_str("## Candidates per meeting (before any cap)\n\n| meeting | candidates | used |\n| --- | --- | --- |\n");
    for (meeting, count) in &plan.counts {
        let used = plan.items.iter().filter(|i| &i.meeting == meeting).count();
        let _ = writeln!(s, "| {meeting} | {count} | {used} |");
    }
    let total: usize = plan.counts.iter().map(|(_, c)| c).sum();
    let _ = writeln!(s, "\nTotal candidates: {total}.");
    match plan.cap {
        Some(n) => {
            let _ = writeln!(
                s,
                "Cap applied: at most {n} candidates per meeting (first {n} in candidate order) so that \
                 candidates x {RUNS} runs x {} models <= {}.",
                MODELS.len(),
                crate::run::MAX_CALLS
            );
        }
        None => s.push_str("No cap applied.\n"),
    }
    let _ = writeln!(s, "\nTooLarge (no call made): {}", plan.too_large.len());
    for t in &plan.too_large {
        let _ = writeln!(s, "- {} {}", t.meeting, t.subject);
    }
    let _ = writeln!(
        s,
        "\nTotal calls (candidate x run x model): {}\n",
        calls.len()
    );
    s.push_str("## Per model\n\n");
    s.push_str(
        "Elapsed percentiles are over finished attempts (timeouts excluded); `per call` sums a call's attempts.\n\n",
    );
    s.push_str(
        "| model | calls | attempts | p50 s | p95 s | per-call p50 s | per-call p95 s | input chars min-max | \
         parse failures (final) | retried | timeouts | dropped | downgraded |\n\
         | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n",
    );
    for model in MODELS {
        let mine: Vec<&Call> = calls.iter().filter(|c| c.model == model).collect();
        let mut att: Vec<u64> = mine
            .iter()
            .flat_map(|c| {
                c.attempts
                    .iter()
                    .filter(|a| !a.timeout)
                    .map(|a| a.elapsed_ms)
            })
            .collect();
        att.sort_unstable();
        let mut per: Vec<u64> = mine
            .iter()
            .filter(|c| !c.timeout)
            .map(|c| c.elapsed_ms)
            .collect();
        per.sort_unstable();
        let min = mine.iter().map(|c| c.input_chars).min().unwrap_or(0);
        let max = mine.iter().map(|c| c.input_chars).max().unwrap_or(0);
        let sec = |ms: u64| format!("{:.1}", ms as f64 / 1000.0);
        let _ = writeln!(
            s,
            "| {model} | {} | {} | {} | {} | {} | {} | {min}-{max} | {} | {} | {} | {} | {} |",
            mine.len(),
            mine.iter().map(|c| c.attempts.len()).sum::<usize>(),
            sec(percentile(&att, 0.5)),
            sec(percentile(&att, 0.95)),
            sec(percentile(&per, 0.5)),
            sec(percentile(&per, 0.95)),
            mine.iter().filter(|c| !c.parse_ok).count(),
            mine.iter().filter(|c| c.retried).count(),
            mine.iter().filter(|c| c.timeout).count(),
            mine.iter().map(|c| c.dropped).sum::<usize>(),
            mine.iter().map(|c| c.downgraded).sum::<usize>(),
        );
    }
    s.push_str("\n## Input stages\n\n");
    for stage in [
        "Full",
        "OneUtterancePerNode",
        "ShortUtterances",
        "NoOtherIssues",
        "NoSecondLevelReasons",
    ] {
        let n = plan
            .items
            .iter()
            .filter(|i| format!("{:?}", i.prompt.stage) == stage)
            .count();
        let _ = writeln!(s, "- {stage}: {n} candidates");
    }
    let failed: Vec<&Call> = calls.iter().filter(|c| !c.parse_ok || c.retried).collect();
    if !failed.is_empty() {
        s.push_str("\n## Failures and retries\n\n");
        for c in failed {
            let _ = writeln!(
                s,
                "- {} {} {} run{}: final ok={} errors={:?}",
                c.model,
                c.meeting,
                c.subject,
                c.run,
                c.parse_ok,
                c.attempts
                    .iter()
                    .map(|a| a.error.clone())
                    .collect::<Vec<_>>()
            );
        }
    }
    s
}

fn one_line(text: &str) -> String {
    text.replace('\n', " ")
}

fn element(s: &mut String, graph: &Graph, label: &str, e: &CloudElement) {
    let _ = writeln!(
        s,
        "  - **{label}** [{}] {}",
        basis(e.basis),
        one_line(&e.text)
    );
    grounds(s, graph, "    ", e);
}

fn grounds(s: &mut String, graph: &Graph, indent: &str, e: &CloudElement) {
    if e.basis == Basis::Stated {
        for uid in &e.grounds {
            match graph.utterances.get(uid) {
                Some(u) => {
                    let _ = writeln!(
                        s,
                        "{indent}- ground {uid} ({}): {}",
                        u.speaker_label,
                        one_line(&u.text)
                    );
                }
                None => {
                    let _ = writeln!(s, "{indent}- ground {uid}: (missing)");
                }
            }
        }
    }
}

pub fn review(out: &Path, plan: &Plan) -> String {
    let mut s = String::from(
        "# Conflict cloud spike: review sheet\n\n\
         Synthetic meetings only. Judge (1) A serves both B and C, (2) B and C are wants not actions, \
         (3) assumptions are falsifiable beliefs, (4) every `stated` element's ground utterances really say it, \
         (5) A is the same gist across the 3 runs. D and D2 are copied from the positions by the app.\n\n",
    );
    for item in &plan.items {
        let graph = &plan
            .graphs
            .iter()
            .find(|(m, _)| m == &item.meeting)
            .unwrap()
            .1;
        let subject = item.candidate.subject();
        let _ = writeln!(s, "## {} / {}\n", item.meeting, subject);
        let issue = &graph.nodes[&item.candidate.issue];
        let _ = writeln!(
            s,
            "- Issue ({}): {}\n- Input stage: {:?}; decided: {}; objection: {}",
            issue.nid,
            issue.text,
            item.prompt.stage,
            item.candidate.decided,
            item.candidate.has_objection
        );
        for (slot, anchor) in ["D", "D2"].iter().zip(&item.prompt.anchors) {
            let _ = writeln!(s, "- {slot} = position {}: {}", anchor.nid, anchor.text);
        }
        s.push('\n');
        for model in MODELS {
            for run in 1..=RUNS {
                let dir = call_dir(out, model, &item.meeting, &subject);
                let call: Option<Call> =
                    fs::read_to_string(dir.join(format!("run{run}.call.json")))
                        .ok()
                        .and_then(|t| serde_json::from_str(&t).ok());
                let diagram: Option<CloudDiagram> =
                    fs::read_to_string(dir.join(format!("run{run}.json")))
                        .ok()
                        .and_then(|t| serde_json::from_str(&t).ok());
                let meta = call.as_ref().map_or("no record".to_owned(), |c| {
                    format!(
                        "{} ms, dropped {}, downgraded {}{}",
                        c.elapsed_ms,
                        c.dropped,
                        c.downgraded,
                        if c.retried { ", retried" } else { "" }
                    )
                });
                let _ = writeln!(s, "- **{model} run{run}** ({meta})");
                let Some(d) = diagram else {
                    let _ = writeln!(s, "  - (no valid cloud: {:?})", call.and_then(|c| c.error));
                    continue;
                };
                element(&mut s, graph, "A", &d.a);
                element(&mut s, graph, "B", &d.b);
                element(&mut s, graph, "C", &d.c);
                let _ = writeln!(s, "  - **D** {}", one_line(&d.d.text));
                let _ = writeln!(s, "  - **D2** {}", one_line(&d.d2.text));
                for a in &d.assumptions {
                    let _ = writeln!(
                        s,
                        "  - assumption on `{}` [{}]: {}",
                        format!("{:?}", a.on).to_lowercase(),
                        basis(a.basis),
                        one_line(&a.text)
                    );
                    if a.basis == Basis::Stated {
                        for uid in &a.grounds {
                            let text = graph
                                .utterances
                                .get(uid)
                                .map_or("(missing)".to_owned(), |u| {
                                    format!("({}): {}", u.speaker_label, one_line(&u.text))
                                });
                            let _ = writeln!(s, "    - ground {uid} {text}");
                        }
                    }
                    if let Some(inj) = &a.injection {
                        let _ = writeln!(
                            s,
                            "    - injection [{}]: {}",
                            basis(inj.basis),
                            one_line(&inj.text)
                        );
                        grounds(&mut s, graph, "      ", inj);
                    }
                }
            }
        }
        s.push('\n');
    }
    s
}
