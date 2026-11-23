use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use credential_store::{CredentialStore, FallbackCredentialStore};
use summarize::{BuildSummarizerError, Summarizer, SummaryProvider};

use crate::app_settings::{AppSettings, HerdrBillingVerification};
use crate::graph_consumer::{build_backend, Backend, DisabledReason};

pub(crate) struct SummaryBackend {
    pub model: String,
    pub provider_model: String,
    pub summarizer: Arc<dyn Summarizer>,
}

pub(crate) type BackendFuture<'a, T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;

pub(crate) trait LlmFactory: Send + Sync {
    fn graph(&self, settings: AppSettings) -> BackendFuture<'_, Backend, DisabledReason>;
    fn summary(
        &self,
        settings: AppSettings,
    ) -> BackendFuture<'_, SummaryBackend, BuildSummarizerError>;

    fn inherited_provider_matches(
        &self,
        _provider: SummaryProvider,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        Box::pin(async { false })
    }

    fn graph_settings(&self, _settings: &mut AppSettings) {}

    fn herdr_is_registered(&self, _provider: SummaryProvider, _settings: &AppSettings) -> bool {
        false
    }
}

pub(crate) struct ProductionLlm {
    pub credentials: Arc<FallbackCredentialStore>,
    pub cli_temp_base: PathBuf,
    pub herdr: HerdrConnections,
}

#[derive(Clone)]
pub(crate) struct HerdrConnection {
    pub mode: HerdrBillingVerification,
    pub summarizer: Arc<dyn Summarizer>,
}

#[derive(Clone, Default)]
pub(crate) struct HerdrConnections {
    pub claude: Option<HerdrConnection>,
    pub codex: Option<HerdrConnection>,
}

pub(crate) fn herdr_verification(
    provider: SummaryProvider,
    settings: &AppSettings,
) -> Option<HerdrBillingVerification> {
    match provider {
        SummaryProvider::ClaudeHerdr => Some(settings.herdr_claude_billing_verification),
        SummaryProvider::CodexHerdr => Some(settings.herdr_codex_billing_verification),
        _ => None,
    }
}

impl HerdrConnections {
    fn selected(
        &self,
        provider: SummaryProvider,
        settings: &AppSettings,
    ) -> Option<&HerdrConnection> {
        let mode = herdr_verification(provider, settings)?;
        let connection = match provider {
            SummaryProvider::ClaudeHerdr => &self.claude,
            SummaryProvider::CodexHerdr => &self.codex,
            _ => return None,
        };
        connection
            .as_ref()
            .filter(|connection| connection.mode == mode)
    }
}

pub(crate) fn build_selected(
    provider: SummaryProvider,
    credentials: Arc<impl CredentialStore + Send + Sync + 'static>,
    settings: AppSettings,
    cli_temp_base: PathBuf,
    herdr: &HerdrConnections,
) -> Result<Arc<dyn Summarizer>, BuildSummarizerError> {
    if provider.herdr_backend().is_some() {
        return herdr
            .selected(provider, &settings)
            .map(|connection| connection.summarizer.clone())
            .ok_or(BuildSummarizerError::HerdrNotConfigured);
    }
    summarize::build_summarizer(
        provider.key(),
        credentials,
        settings.ollama_base_url,
        Some(cli_temp_base),
    )
    .map(Arc::from)
}

pub(crate) fn selected_summary_provider(credentials: &impl CredentialStore) -> SummaryProvider {
    credentials
        .load(
            summarize::CREDENTIAL_SERVICE,
            summarize::SELECTED_PROVIDER_ACCOUNT,
        )
        .ok()
        .map(|key| SummaryProvider::from_key(&key))
        .unwrap_or(SummaryProvider::Claude)
}

impl LlmFactory for ProductionLlm {
    fn herdr_is_registered(&self, provider: SummaryProvider, settings: &AppSettings) -> bool {
        self.herdr.selected(provider, settings).is_some()
    }

    fn inherited_provider_matches(
        &self,
        provider: SummaryProvider,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        let credentials = self.credentials.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                selected_summary_provider(credentials.as_ref()) == provider
            })
            .await
            .unwrap_or(false)
        })
    }

    fn graph(&self, settings: AppSettings) -> BackendFuture<'_, Backend, DisabledReason> {
        Box::pin(async move {
            let credentials = self.credentials.clone();
            let cli_temp_base = self.cli_temp_base.clone();
            let herdr = self.herdr.clone();
            let (provider, backend) = tokio::task::spawn_blocking(move || {
                build_backend(credentials, settings, cli_temp_base, &herdr)
            })
            .await
            .map_err(|_| DisabledReason::Backend)??;
            if let Some(cli) = provider.cli_backend() {
                if !cli.is_available().await {
                    return Err(DisabledReason::Backend);
                }
            }
            Ok(backend)
        })
    }

    fn summary(
        &self,
        settings: AppSettings,
    ) -> BackendFuture<'_, SummaryBackend, BuildSummarizerError> {
        Box::pin(async move {
            let credentials = self.credentials.clone();
            let cli_temp_base = self.cli_temp_base.clone();
            let herdr = self.herdr.clone();
            tokio::task::spawn_blocking(move || {
                let provider = selected_summary_provider(credentials.as_ref());
                let model = credentials
                    .load(
                        summarize::CREDENTIAL_SERVICE,
                        summarize::SELECTED_MODEL_ACCOUNT,
                    )
                    .unwrap_or_else(|_| provider.default_model().to_string());
                let summarizer =
                    build_selected(provider, credentials, settings, cli_temp_base, &herdr)?;
                Ok(SummaryBackend {
                    provider_model: format!("{}/{}", provider.key(), model),
                    model,
                    summarizer,
                })
            })
            .await
            .map_err(|_| BuildSummarizerError::Client)?
        })
    }
}

#[cfg(all(windows, feature = "e2e-scripted"))]
pub(crate) struct ScriptedLlm(pub &'static e2e_scripted::Scenario);

#[cfg(all(windows, feature = "e2e-scripted"))]
impl LlmFactory for ScriptedLlm {
    fn graph(&self, _settings: AppSettings) -> BackendFuture<'_, Backend, DisabledReason> {
        Box::pin(async move {
            Ok(Backend {
                herdr: None,
                model: "scripted".into(),
                summarizer: Arc::new(self.0.summarizer()),
            })
        })
    }

    fn summary(
        &self,
        _settings: AppSettings,
    ) -> BackendFuture<'_, SummaryBackend, BuildSummarizerError> {
        Box::pin(async move {
            Ok(SummaryBackend {
                model: "scripted".into(),
                provider_model: "scripted".into(),
                summarizer: Arc::new(self.0.summarizer()),
            })
        })
    }

    fn graph_settings(&self, settings: &mut AppSettings) {
        let overrides = self.0.settings();
        settings.graph_batch_utterances = overrides
            .graph_batch_utterances
            .or(settings.graph_batch_utterances);
        settings.graph_batch_seconds = overrides
            .graph_batch_seconds
            .or(settings.graph_batch_seconds);
        settings.graph_review_threshold = overrides
            .graph_review_threshold
            .or(settings.graph_review_threshold);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use summarize::{cli_backend::CliBackend, CliSummarizer};

    #[test]
    fn herdr_selection_reuses_the_connection_and_requires_matching_verification() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = Arc::new(credential_store::EncryptedFileStore::new(dir.path()).unwrap());
        let claude =
            Arc::new(CliSummarizer::new(CliBackend::ClaudeCode, None)) as Arc<dyn Summarizer>;
        let codex = Arc::new(CliSummarizer::new(CliBackend::Codex, None)) as Arc<dyn Summarizer>;
        let herdr = HerdrConnections {
            claude: Some(HerdrConnection {
                mode: HerdrBillingVerification::AdministratorVerified,
                summarizer: claude.clone(),
            }),
            codex: Some(HerdrConnection {
                mode: HerdrBillingVerification::Automatic,
                summarizer: codex.clone(),
            }),
        };
        let mut settings = AppSettings::default();
        let select = |provider, settings: &AppSettings| {
            build_selected(
                provider,
                credentials.clone(),
                settings.clone(),
                dir.path().into(),
                &herdr,
            )
        };
        assert!(matches!(
            select(SummaryProvider::ClaudeHerdr, &settings),
            Err(BuildSummarizerError::HerdrNotConfigured)
        ));
        assert!(Arc::ptr_eq(
            &select(SummaryProvider::CodexHerdr, &settings).unwrap(),
            &codex
        ));
        settings.herdr_claude_billing_verification =
            HerdrBillingVerification::AdministratorVerified;
        assert!(Arc::ptr_eq(
            &select(SummaryProvider::ClaudeHerdr, &settings).unwrap(),
            &claude
        ));
        assert!(Arc::ptr_eq(
            &select(SummaryProvider::ClaudeHerdr, &settings).unwrap(),
            &claude
        ));
    }
}
