//! Plan the candidates and run the calls.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
    thread,
};

use anyhow::Result;
use discussion_graph::{
    build_conflict_cloud_default, conflict_candidates, validate_cloud, CloudInputError,
    CloudPrompt, ConflictCandidate, Graph,
};
use serde::{Deserialize, Serialize};

use crate::{backend::Backend, data};

/// Round 2 measures sonnet only.
pub const MODELS: [&str; 1] = ["sonnet"];
/// The candidate cap still assumes two models, so round 2 uses the same 22 candidates as round 1.
pub const CAP_MODELS: usize = 2;
pub const RUNS: u32 = 3;
pub const MAX_CALLS: usize = 150;

pub struct Item {
    pub meeting: String,
    pub candidate: ConflictCandidate,
    pub prompt: CloudPrompt,
}

pub struct TooLarge {
    pub meeting: String,
    pub subject: String,
}

pub struct Plan {
    pub graphs: Vec<(String, Graph)>,
    /// Candidate counts per meeting before any cap.
    pub counts: Vec<(String, usize)>,
    pub cap: Option<usize>,
    pub items: Vec<Item>,
    pub too_large: Vec<TooLarge>,
}

pub fn make_plan(data_dir: &Path) -> Result<Plan> {
    let mut graphs = Vec::new();
    let mut found = Vec::new();
    for name in data::MEETINGS {
        let graph = data::load(data_dir, name)?;
        let candidates = conflict_candidates(&graph);
        found.push((name.to_owned(), candidates));
        graphs.push((name.to_owned(), graph));
    }
    let per_cap = MAX_CALLS / (RUNS as usize * CAP_MODELS);
    let total: usize = found.iter().map(|(_, c)| c.len()).sum();
    let cap = (total > per_cap).then(|| {
        (0..=total)
            .rev()
            .find(|n| found.iter().map(|(_, c)| c.len().min(*n)).sum::<usize>() <= per_cap)
            .unwrap_or(0)
    });
    let counts = found.iter().map(|(m, c)| (m.clone(), c.len())).collect();
    let mut items = Vec::new();
    let mut too_large = Vec::new();
    for ((name, candidates), (_, graph)) in found.into_iter().zip(&graphs) {
        for candidate in candidates.into_iter().take(cap.unwrap_or(usize::MAX)) {
            let [d, d2] = &candidate.default_pair;
            match build_conflict_cloud_default(graph, &candidate.issue, [d, d2]) {
                Ok(prompt) => items.push(Item {
                    meeting: name.clone(),
                    candidate,
                    prompt,
                }),
                Err(CloudInputError::TooLarge) => too_large.push(TooLarge {
                    meeting: name.clone(),
                    subject: candidate.subject(),
                }),
                Err(e) => anyhow::bail!("{name} {}: {e}", candidate.subject()),
            }
        }
    }
    Ok(Plan {
        graphs,
        counts,
        cap,
        items,
        too_large,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attempt {
    pub elapsed_ms: u64,
    pub ok: bool,
    pub error: Option<String>,
    pub timeout: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Call {
    pub meeting: String,
    pub subject: String,
    pub model: String,
    pub run: u32,
    /// Sum over attempts.
    pub elapsed_ms: u64,
    pub input_chars: usize,
    pub stage: String,
    pub parse_ok: bool,
    pub error: Option<String>,
    pub dropped: usize,
    pub downgraded: usize,
    pub timeout: bool,
    pub retried: bool,
    pub attempts: Vec<Attempt>,
}

pub fn safe(subject: &str) -> String {
    subject.replace(':', "_")
}

pub fn call_dir(out: &Path, model: &str, meeting: &str, subject: &str) -> PathBuf {
    out.join(model).join(meeting).join(safe(subject))
}

pub fn do_call(
    backend: &dyn Backend,
    out: &Path,
    item: &Item,
    model: &str,
    run: u32,
) -> Result<Call> {
    let subject = item.candidate.subject();
    let dir = call_dir(out, model, &item.meeting, &subject);
    fs::create_dir_all(&dir)?;
    let record_path = dir.join(format!("run{run}.call.json"));
    if let Ok(text) = fs::read_to_string(&record_path) {
        return Ok(serde_json::from_str(&text)?);
    }
    let prompt = &item.prompt;
    let mut user = prompt.user.clone();
    let mut attempts = Vec::new();
    let mut verdict = None;
    for attempt in 0..2 {
        let outcome = backend.complete(model, prompt, &user);
        let mut rec = Attempt {
            elapsed_ms: outcome.elapsed_ms,
            ok: false,
            error: None,
            timeout: outcome.timeout,
        };
        let suffix = if attempt == 0 { "raw.txt" } else { "raw2.txt" };
        match outcome.text {
            Ok(text) => {
                fs::write(dir.join(format!("run{run}.{suffix}")), &text)?;
                match validate_cloud(&text, prompt) {
                    Ok(valid) => {
                        rec.ok = true;
                        verdict = Some(valid);
                    }
                    Err(e) => rec.error = Some(format!("{e:?}")),
                }
            }
            Err(e) => rec.error = Some(e),
        }
        let stop = rec.ok || rec.timeout;
        let note = rec.error.clone();
        attempts.push(rec);
        if stop {
            break;
        }
        user = format!(
            "{}\n\n（前回の応答は読めませんでした: {}。JSON オブジェクト 1 つだけを返してください。）",
            prompt.user,
            note.unwrap_or_default()
        );
    }
    let last = attempts.last().unwrap();
    let (dropped, downgraded) = verdict
        .as_ref()
        .map_or((0, 0), |v| (v.dropped, v.downgraded));
    if let Some(valid) = &verdict {
        fs::write(
            dir.join(format!("run{run}.json")),
            serde_json::to_string_pretty(&valid.diagram)?,
        )?;
    }
    let call = Call {
        meeting: item.meeting.clone(),
        subject,
        model: model.to_owned(),
        run,
        elapsed_ms: attempts.iter().map(|a| a.elapsed_ms).sum(),
        input_chars: prompt.system.chars().count() + prompt.user.chars().count(),
        stage: format!("{:?}", prompt.stage),
        parse_ok: last.ok,
        error: last.error.clone(),
        dropped,
        downgraded,
        timeout: attempts.iter().any(|a| a.timeout),
        retried: attempts.len() > 1,
        attempts,
    };
    fs::write(&record_path, serde_json::to_string_pretty(&call)?)?;
    Ok(call)
}

/// Run every (item, model, run) with at most `parallel` workers. Existing
/// per-call records are reused, so an interrupted run resumes.
pub fn run_all(
    backend: &dyn Backend,
    out: &Path,
    plan: &Plan,
    parallel: usize,
) -> Result<Vec<Call>> {
    let mut jobs = Vec::new();
    for model in MODELS {
        for item in &plan.items {
            for run in 1..=RUNS {
                jobs.push((item, model, run));
            }
        }
    }
    let next = AtomicUsize::new(0);
    let done = Mutex::new(Vec::new());
    thread::scope(|scope| {
        for _ in 0..parallel.max(1) {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, Ordering::SeqCst);
                let Some((item, model, run)) = jobs.get(index) else {
                    break;
                };
                match do_call(backend, out, item, model, *run) {
                    Ok(call) => {
                        eprintln!(
                            "[{}/{}] {} {} {} run{} {}ms ok={}",
                            index + 1,
                            jobs.len(),
                            call.model,
                            call.meeting,
                            call.subject,
                            call.run,
                            call.elapsed_ms,
                            call.parse_ok
                        );
                        done.lock().unwrap().push(call);
                    }
                    Err(e) => eprintln!("call failed: {e}"),
                }
            });
        }
    });
    let mut calls = done.into_inner().unwrap();
    calls.sort_by(|a, b| {
        (&a.model, &a.meeting, &a.subject, a.run).cmp(&(&b.model, &b.meeting, &b.subject, b.run))
    });
    Ok(calls)
}
