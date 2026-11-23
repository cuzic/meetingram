use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use credential_store::{CredentialStore, FallbackCredentialStore};
use summarize::{BuildSummarizerError, Summarizer, SummaryProvider};

use crate::app_settings::AppSettings;
use crate::graph_consumer::{build_backend, Backend, DisabledReason};

pub(crate) struct SummaryBackend {
    pub model: String,
    pub provider_model: String,
    pub summarizer: Box<dyn Summarizer>,
}

pub(crate) type BackendFuture<'a, T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;

pub(crate) trait LlmFactory: Send + Sync {
    fn graph(&self, settings: AppSettings) -> BackendFuture<'_, Backend, DisabledReason>;
    fn summary(
        &self,
        settings: AppSettings,
    ) -> BackendFuture<'_, SummaryBackend, BuildSummarizerError>;

    fn graph_settings(&self, _settings: &mut AppSettings) {}
}

pub(crate) struct ProductionLlm {
    pub credentials: Arc<FallbackCredentialStore>,
    pub cli_temp_base: PathBuf,
}

impl LlmFactory for ProductionLlm {
    fn graph(&self, settings: AppSettings) -> BackendFuture<'_, Backend, DisabledReason> {
        Box::pin(async move {
            let credentials = self.credentials.clone();
            let cli_temp_base = self.cli_temp_base.clone();
            let (provider, backend) = tokio::task::spawn_blocking(move || {
                build_backend(credentials, settings, cli_temp_base)
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
            tokio::task::spawn_blocking(move || {
                let provider = credentials
                    .load(
                        summarize::CREDENTIAL_SERVICE,
                        summarize::SELECTED_PROVIDER_ACCOUNT,
                    )
                    .ok()
                    .map(|key| SummaryProvider::from_key(&key))
                    .unwrap_or(SummaryProvider::Claude);
                let model = credentials
                    .load(
                        summarize::CREDENTIAL_SERVICE,
                        summarize::SELECTED_MODEL_ACCOUNT,
                    )
                    .unwrap_or_else(|_| provider.default_model().to_string());
                let summarizer = summarize::build_summarizer(
                    provider.key(),
                    credentials,
                    settings.ollama_base_url,
                    Some(cli_temp_base),
                )?;
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
                model: "scripted".into(),
                summarizer: Box::new(self.0.summarizer()),
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
                summarizer: Box::new(self.0.summarizer()),
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
