//! LLM backends: the `claude` CLI and a canned fake for the dry run.

use std::{
    io::{Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use discussion_graph::CloudPrompt;

pub const TIMEOUT: Duration = Duration::from_secs(120);

pub struct Outcome {
    /// Response text, or a short error description.
    pub text: Result<String, String>,
    pub elapsed_ms: u64,
    pub timeout: bool,
}

pub trait Backend: Sync {
    /// `user` is the prompt's user text, possibly with a retry note appended.
    fn complete(&self, model: &str, prompt: &CloudPrompt, user: &str) -> Outcome;
}

pub struct ClaudeCli {
    pub workdir: PathBuf,
}

impl Backend for ClaudeCli {
    fn complete(&self, model: &str, prompt: &CloudPrompt, user: &str) -> Outcome {
        let start = Instant::now();
        let done = |text, timeout| Outcome {
            text,
            elapsed_ms: start.elapsed().as_millis() as u64,
            timeout,
        };
        let spawned = Command::new("claude")
            .args(["-p", "--model", model, "--output-format", "text"])
            .args([
                "--tools",
                "",
                "--no-session-persistence",
                "--disable-slash-commands",
            ])
            .args(["--system-prompt", &prompt.system])
            .current_dir(&self.workdir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(_) => return done(Err("spawn failed".into()), false),
        };
        let mut stdin = child.stdin.take().unwrap();
        let input = user.to_owned();
        let writer = thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        });
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let out = thread::spawn(move || {
            let mut text = String::new();
            let _ = stdout.read_to_string(&mut text);
            text
        });
        let err = thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        });
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if start.elapsed() >= TIMEOUT => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                Ok(None) => thread::sleep(Duration::from_millis(100)),
                Err(_) => break None,
            }
        };
        let _ = writer.join();
        let text = out.join().unwrap_or_default();
        let stderr_text = err.join().unwrap_or_default();
        match status {
            None => done(Err("timeout".into()), true),
            Some(status) if status.success() => done(Ok(text), false),
            Some(status) => {
                let head: String = stderr_text.chars().take(200).collect();
                done(Err(format!("exit {status}: {head}")), false)
            }
        }
    }
}

/// Canned backend: a valid cloud grounded in the prompt's own utterances.
/// With `fail_first`, the first attempt (no retry note) is not JSON.
pub struct Fake {
    pub fail_first: bool,
}

impl Backend for Fake {
    fn complete(&self, _model: &str, prompt: &CloudPrompt, user: &str) -> Outcome {
        let text = if self.fail_first && !user.contains("前回の応答") {
            "sorry, not json".to_owned()
        } else {
            let uid = prompt.allowed_uids.first().cloned().unwrap_or_default();
            let nid = prompt.allowed_nids.first().cloned().unwrap_or_default();
            serde_json::json!({
                "a": {"text": "共通の目的（合成）", "basis": "inferred", "nids": [nid]},
                "b": {"text": "要望 B（合成）", "basis": "stated", "grounds": [uid]},
                "c": {"text": "要望 C（合成）", "basis": "stated", "grounds": ["nope"]},
                "d": {"text": "ignored"},
                "d2": {"text": "ignored"},
                "assumptions": [
                    {"on": "bd", "text": "前提（合成）", "basis": "inferred",
                     "injection": {"text": "打ち手（合成）", "basis": "inferred"}},
                    {"on": "zz", "text": "bad on", "basis": "inferred"}
                ]
            })
            .to_string()
        };
        Outcome {
            text: Ok(text),
            elapsed_ms: 1,
            timeout: false,
        }
    }
}
