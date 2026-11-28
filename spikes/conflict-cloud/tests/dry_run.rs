use std::{fs, path::PathBuf};

use conflict_cloud_spike::{
    backend::Fake,
    report,
    run::{make_plan, run_all, MODELS, RUNS},
};

fn plan() -> conflict_cloud_spike::run::Plan {
    make_plan(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data")).unwrap()
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cc-spike-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn plan_respects_call_budget() {
    let plan = plan();
    assert!(!plan.items.is_empty());
    assert!(plan.items.len() * RUNS as usize * MODELS.len() <= 150);
}

#[test]
fn fake_backend_pipeline_writes_all_artifacts() {
    let plan = plan();
    let out = tmp("fake");
    let calls = run_all(&Fake { fail_first: false }, &out, &plan, 3).unwrap();
    assert_eq!(calls.len(), plan.items.len() * RUNS as usize * MODELS.len());
    assert!(calls.iter().all(|c| c.parse_ok && !c.retried && !c.timeout));
    // Canned response has one unknown ground and one bad `on`: both are dropped.
    assert!(calls.iter().all(|c| c.dropped >= 2 && c.downgraded == 1));
    report::write_all(&out, &plan, &calls).unwrap();
    for model in MODELS {
        let lines = fs::read_to_string(out.join(model).join("calls.jsonl")).unwrap();
        assert_eq!(lines.lines().count(), calls.len() / MODELS.len());
    }
    let review = fs::read_to_string(out.join("REVIEW.md")).unwrap();
    assert!(review.contains("**A** [inferred]") && review.contains("- ground u"));
    let summary = fs::read_to_string(out.join("SUMMARY.md")).unwrap();
    assert!(summary.contains("| sonnet |"));
    let first = &plan.items[0];
    let dir = out
        .join("sonnet")
        .join(&first.meeting)
        .join(first.candidate.subject().replace(':', "_"));
    assert!(dir.join("run1.raw.txt").exists() && dir.join("run1.json").exists());
    let _ = fs::remove_dir_all(out);
}

#[test]
fn unparsable_first_answer_is_retried_once() {
    let plan = plan();
    let out = tmp("retry");
    let calls = run_all(&Fake { fail_first: true }, &out, &plan, 1).unwrap();
    assert!(calls
        .iter()
        .all(|c| c.parse_ok && c.retried && c.attempts.len() == 2));
    assert!(calls
        .iter()
        .all(|c| c.attempts[0].error.as_deref() == Some("NotJson")));
    let _ = fs::remove_dir_all(out);
}
