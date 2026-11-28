use std::{fs, path::PathBuf};

use anyhow::{bail, Result};
use conflict_cloud_spike::{
    backend::{Backend, ClaudeCli, Fake},
    report,
    run::{make_plan, run_all, Call, MODELS, RUNS},
};

fn main() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let data = root.join("data");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone());
    let out = flag("--out").map_or(root.join("results"), PathBuf::from);
    let plan = make_plan(&data)?;
    match args.first().map(String::as_str) {
        Some("candidates") => {
            for (meeting, count) in &plan.counts {
                println!("{meeting}: {count}");
            }
            let total: usize = plan.counts.iter().map(|(_, c)| c).sum();
            println!(
                "total {total}; cap {:?}; calls {}; too large {}",
                plan.cap,
                plan.items.len() * RUNS as usize * MODELS.len(),
                plan.too_large.len()
            );
        }
        Some("run") => {
            let backend: Box<dyn Backend> = match flag("--backend").as_deref() {
                Some("claude") => {
                    let workdir = PathBuf::from(flag("--workdir").unwrap_or_default());
                    if !workdir.is_dir() {
                        bail!("--workdir must be an existing directory outside the repo");
                    }
                    Box::new(ClaudeCli { workdir })
                }
                Some("fake") => Box::new(Fake { fail_first: false }),
                _ => bail!("--backend claude|fake"),
            };
            let parallel = flag("--parallel").and_then(|v| v.parse().ok()).unwrap_or(3);
            fs::create_dir_all(&out)?;
            let calls: Vec<Call> = run_all(backend.as_ref(), &out, &plan, parallel)?;
            report::write_all(&out, &plan, &calls)?;
            println!("{} calls; wrote {}", calls.len(), out.display());
        }
        _ => bail!("usage: candidates | run --backend claude|fake [--out DIR] [--workdir DIR] [--parallel N]"),
    }
    Ok(())
}
