use std::{path::Path, sync::Arc};

use crate::{app_settings::AppSettings, app_state::AppState, graph_consumer::DisabledReason};
use dioxus::prelude::*;

use super::graph_view::disabled_message;

struct GraphSettingsDraft {
    enabled: bool,
    model: String,
    utterances: String,
    seconds: String,
    threshold: String,
}

impl From<&AppSettings> for GraphSettingsDraft {
    fn from(settings: &AppSettings) -> Self {
        Self {
            enabled: settings.graph_enabled.unwrap_or(true),
            model: settings.graph_model.clone().unwrap_or_default(),
            utterances: settings.graph_batch_utterances.unwrap_or(6).to_string(),
            seconds: settings.graph_batch_seconds.unwrap_or(45).to_string(),
            threshold: settings.graph_review_threshold.unwrap_or(0.6).to_string(),
        }
    }
}

impl GraphSettingsDraft {
    fn save(&self, settings: &mut AppSettings, directory: &Path) -> Result<(), &'static str> {
        let utterances = self
            .utterances
            .parse::<usize>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or("窓の件数は 1 以上の整数を入力してください")?;
        let seconds = self
            .seconds
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or("窓の秒数は 1 以上の整数を入力してください")?;
        let threshold = self
            .threshold
            .parse::<f64>()
            .ok()
            .filter(|value| (0.0..=1.0).contains(value))
            .ok_or("確認のしきい値は 0 から 1 の範囲で入力してください")?;

        let mut updated = settings.clone();
        updated.graph_enabled = Some(self.enabled);
        updated.graph_model = Some(self.model.trim().to_string());
        updated.graph_batch_utterances = Some(utterances);
        updated.graph_batch_seconds = Some(seconds);
        updated.graph_review_threshold = Some(threshold);
        updated
            .save(directory)
            .map_err(|_| "議論グラフの設定を保存できませんでした")?;
        *settings = updated;
        Ok(())
    }
}

#[component]
pub(crate) fn GraphSettings(disabled_reason: Option<DisabledReason>) -> Element {
    let state = use_context::<Arc<AppState>>();
    let mut draft = use_signal(|| GraphSettingsDraft::from(&*state.app_settings.lock().unwrap()));
    let mut message = use_signal(|| None::<&'static str>);
    let disabled = disabled_reason.map(disabled_message);

    rsx! {
        section { class: "settings-section",
            h2 { "議論グラフ" }
            if let Some(reason) = disabled {
                p { class: "hint", "セッションの起動結果 · {reason}" }
            }
            label {
                input {
                    r#type: "checkbox",
                    checked: draft.read().enabled,
                    onchange: move |event| draft.write().enabled = event.checked(),
                }
                "議論グラフを有効にする"
            }
            label {
                "グラフ用のモデル（空欄なら要約と同じ）"
                input {
                    r#type: "text",
                    value: "{draft.read().model}",
                    oninput: move |event| draft.write().model = event.value(),
                }
            }
            label {
                "抽出の窓（件数）"
                input {
                    r#type: "number",
                    min: "1",
                    value: "{draft.read().utterances}",
                    oninput: move |event| draft.write().utterances = event.value(),
                }
            }
            label {
                "抽出の窓（秒）"
                input {
                    r#type: "number",
                    min: "1",
                    value: "{draft.read().seconds}",
                    oninput: move |event| draft.write().seconds = event.value(),
                }
            }
            label {
                "確認のしきい値"
                input {
                    r#type: "number",
                    min: "0",
                    max: "1",
                    step: "any",
                    value: "{draft.read().threshold}",
                    oninput: move |event| draft.write().threshold = event.value(),
                }
            }
            button {
                class: "primary",
                onclick: move |_| {
                    let mut settings = state.app_settings.lock().unwrap();
                    let result = draft.read().save(&mut settings, &state.app_data_dir);
                    message.set(Some(match result {
                        Ok(()) => "議論グラフの設定を保存しました",
                        Err(error) => error,
                    }));
                },
                "この設定を保存"
            }
            if let Some(text) = message() {
                p { class: "status-badge", "{text}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_graph_fields_and_preserves_other_settings() {
        let directory = tempfile::tempdir().unwrap();
        let mut settings = AppSettings {
            hint_enabled: Some(true),
            ..AppSettings::default()
        };
        let defaults = GraphSettingsDraft::from(&settings);
        assert!(defaults.enabled);
        assert!(defaults.model.is_empty());
        assert_eq!(defaults.utterances, "6");
        assert_eq!(defaults.seconds, "45");
        assert_eq!(defaults.threshold, "0.6");

        let draft = GraphSettingsDraft {
            enabled: false,
            model: " sonnet ".into(),
            utterances: "12".into(),
            seconds: "90".into(),
            threshold: "0.75".into(),
        };
        draft.save(&mut settings, directory.path()).unwrap();
        let loaded = AppSettings::load(directory.path());
        assert_eq!(loaded.graph_enabled, Some(false));
        assert_eq!(loaded.graph_model.as_deref(), Some("sonnet"));
        assert_eq!(loaded.graph_batch_utterances, Some(12));
        assert_eq!(loaded.graph_batch_seconds, Some(90));
        assert_eq!(loaded.graph_review_threshold, Some(0.75));
        assert_eq!(loaded.hint_enabled, Some(true));
        assert_eq!(settings.graph_enabled, loaded.graph_enabled);
    }

    #[test]
    fn invalid_input_and_failed_save_leave_current_settings_intact() {
        let directory = tempfile::tempdir().unwrap();
        let mut settings = AppSettings::default();
        let mut draft = GraphSettingsDraft::from(&settings);
        draft.enabled = false;
        draft.threshold = "NaN".into();
        assert!(draft.save(&mut settings, directory.path()).is_err());
        assert!(!AppSettings::path(directory.path()).exists());
        draft.threshold = "0.6".into();
        assert!(draft
            .save(&mut settings, &directory.path().join("missing"))
            .is_err());
        assert_eq!(settings.graph_enabled, None);
    }
}
