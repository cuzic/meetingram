//! Admin verification for a dedicated Herdr core. A deployed approval must
//! describe an account whose additional billing is disabled by its administrator.
//! Deploy `claude.json` or `codex.json` under the app data directory's
//! `herdr-approvals` folder before startup. Keep approval files and the dedicated
//! CLI profiles under administrator control. The account and CLI configuration
//! must stay fixed for the lifetime of the approval; revoke the file before
//! changing either. A core restart requires a new approval. `expires_at` is
//! Unix time in seconds. The approval asserts billing and tool restrictions
//! applied by the administrator; the app checks this assertion, expiry, the
//! connected core identity, and subscription login before sending a prompt.

use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use summarize::cli_backend::{CliBackend, HerdrPreflight, HerdrSummarizer};

use crate::{
    app_settings::HerdrBillingVerification,
    llm_factory::{HerdrConnection, HerdrConnections},
};

#[derive(Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Approval {
    herdr_binary: PathBuf,
    provider_binary: PathBuf,
    session: String,
    core_id: String,
    core_pid: u32,
    expires_at: u64,
    additional_billing_disabled: bool,
    external_tools_disabled: bool,
}

impl Approval {
    fn read(path: &Path) -> Option<Self> {
        const MAX_BYTES: usize = 16 * 1024;
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .ok()?
            .take((MAX_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() > MAX_BYTES {
            return None;
        }
        serde_json::from_slice(&bytes).ok()
    }

    fn valid_at(&self, now: SystemTime) -> bool {
        self.additional_billing_disabled
            && self.external_tools_disabled
            && self.herdr_binary.is_absolute()
            && self.provider_binary.is_absolute()
            && !self.session.is_empty()
            && !self.core_id.is_empty()
            && self.core_pid != 0
            && now
                .duration_since(UNIX_EPOCH)
                .is_ok_and(|now| now.as_secs() < self.expires_at)
    }
}

struct AdminPreflight {
    path: PathBuf,
    approved: Approval,
}

#[async_trait::async_trait]
impl HerdrPreflight for AdminPreflight {
    async fn verify(&self, session: &str, core_id: &str, pid: u32) -> bool {
        if session != self.approved.session
            || core_id != self.approved.core_id
            || pid != self.approved.core_pid
        {
            return false;
        }
        let path = self.path.clone();
        let approved = self.approved.clone();
        tokio::task::spawn_blocking(move || {
            approved.valid_at(SystemTime::now())
                && Approval::read(&path).as_ref() == Some(&approved)
        })
        .await
        .unwrap_or(false)
    }
}

fn load_connection(
    path: PathBuf,
    temp_base: &Path,
    backend: CliBackend,
) -> Option<HerdrConnection> {
    let approved = Approval::read(&path)?;
    if !approved.valid_at(SystemTime::now()) {
        return None;
    }
    let summarizer = HerdrSummarizer::new(
        backend,
        approved.herdr_binary.clone(),
        approved.session.clone(),
        approved.provider_binary.clone(),
        temp_base.to_path_buf(),
        Arc::new(AdminPreflight { path, approved }),
    );
    Some(HerdrConnection {
        mode: HerdrBillingVerification::AdministratorVerified,
        summarizer: Arc::new(summarizer),
    })
}

pub(crate) fn load_connections(app_data_dir: &Path, temp_base: &Path) -> HerdrConnections {
    let approval_dir = app_data_dir.join("herdr-approvals");
    HerdrConnections {
        claude: load_connection(
            approval_dir.join("claude.json"),
            temp_base,
            CliBackend::ClaudeCode,
        ),
        codex: load_connection(
            approval_dir.join("codex.json"),
            temp_base,
            CliBackend::Codex,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn administrator_approval_is_bound_to_the_live_core_and_revocable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("approval.json");
        let data = serde_json::json!({
            "herdr_binary": dir.path().join("hc"),
            "provider_binary": dir.path().join("claude"),
            "session": "synthetic",
            "core_id": "synthetic-core",
            "core_pid": 42,
            "expires_at": u64::MAX,
            "additional_billing_disabled": true,
            "external_tools_disabled": true
        });
        std::fs::write(&path, data.to_string()).unwrap();
        let approved = Approval::read(&path).unwrap();
        let verifier = AdminPreflight {
            path: path.clone(),
            approved,
        };
        assert!(verifier.verify("synthetic", "synthetic-core", 42).await);
        assert!(!verifier.verify("synthetic", "synthetic-core", 43).await);
        assert!(!verifier.verify("synthetic", "other-core", 42).await);
        std::fs::remove_file(path).unwrap();
        assert!(!verifier.verify("synthetic", "synthetic-core", 42).await);
    }
}
