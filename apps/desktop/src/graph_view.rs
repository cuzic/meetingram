use std::time::Duration;

use crate::graph_consumer::{DisabledReason, GraphStatus};

pub(super) struct GraphView {
    pub label: String,
    pub can_continue: bool,
    pub can_rebuild: bool,
    pub action_reason: Option<&'static str>,
}

pub(super) fn disabled_message(reason: DisabledReason) -> &'static str {
    match reason {
        DisabledReason::Platform => "議論グラフは Windows で利用できます",
        DisabledReason::Disabled => "議論グラフの設定が無効です",
        DisabledReason::DisabledDuringRun => "設定で無効にしたため、議論グラフの処理を停止しました",
        DisabledReason::Provider => "要約が Codex CLI のため議論グラフは使えません。要約のプロバイダを Claude CLI か API に変えると使えます",
        DisabledReason::Backend => "要約の API キーまたは CLI の設定を確認してください",
    }
}

pub(super) fn should_refresh_pending(
    history: bool,
    recording: bool,
    status: &GraphStatus,
    needs_refresh: bool,
) -> bool {
    history && !recording && !status.running && !status.rebuilding && needs_refresh
}

pub(super) fn continue_is_pending(
    status: &GraphStatus,
    previous: &GraphStatus,
    elapsed: Duration,
) -> bool {
    !status.running
        && status.pending == previous.pending
        && status.disabled_reason == previous.disabled_reason
        // An idle consumer can report zero pending before and after a successful run.
        && status.rev == previous.rev
        && status.failure_count == previous.failure_count
        && elapsed < Duration::from_secs(30)
}

pub(super) fn should_read_capture_state(
    history: bool,
    has_capture_state: bool,
    was_busy: bool,
    busy: bool,
    refresh_requested: bool,
) -> bool {
    history && (!has_capture_state || (!busy && (was_busy || refresh_requested)))
}

#[derive(Default)]
pub(super) struct HistoryPending {
    pub count: Option<usize>,
    pub failed: bool,
}

impl HistoryPending {
    pub(super) fn update(&mut self, result: Result<usize, &'static str>) {
        self.failed = result.is_err();
        if let Ok(count) = result {
            self.count = Some(count);
        }
    }
}

pub(super) fn rebuild_confirmation(session_label: &str) -> String {
    format!("セッション（{session_label}）のグラフを作り直すと、承認・却下の結果と決定・宿題の変更履歴も消えます。")
}

impl GraphView {
    pub(super) fn new(
        status: &GraphStatus,
        recording: bool,
        enabled: bool,
        continue_pending: bool,
    ) -> Self {
        let pending = format!("未抽出 {} 件", status.pending);
        let label = if status.rebuilding {
            "作り直し中".to_string()
        } else if let Some(reason) = status.disabled_reason {
            disabled_message(reason).to_string()
        } else if status.live_halted {
            format!("会議中に停止 · {pending}")
        } else if status.running {
            format!("抽出中 · {pending}")
        } else {
            pending
        };
        let can_rebuild = enabled && !recording && !status.rebuilding && !continue_pending;
        Self {
            label,
            can_continue: can_rebuild && !status.running && status.pending > 0,
            can_rebuild,
            action_reason: (!enabled).then_some("設定で無効"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_view_never_refreshes_pending_count() {
        let status = GraphStatus {
            running: true,
            pending: 4,
            ..GraphStatus::default()
        };
        assert!(!should_refresh_pending(false, true, &status, true));
        assert!(!should_refresh_pending(
            false,
            false,
            &GraphStatus::default(),
            true
        ));
        assert_eq!(
            GraphView::new(&status, true, true, false).label,
            "抽出中 · 未抽出 4 件"
        );
    }

    #[test]
    fn pending_count_refresh_requires_idle_history_and_an_invalidation() {
        let mut status = GraphStatus::default();
        assert!(should_refresh_pending(true, false, &status, true));
        assert!(!should_refresh_pending(true, false, &status, false));
        assert!(!should_refresh_pending(true, true, &status, true));
        status.running = true;
        assert!(!should_refresh_pending(true, false, &status, true));
        status.running = false;
        status.rebuilding = true;
        assert!(!should_refresh_pending(true, false, &status, true));
    }

    #[test]
    fn describes_graph_states() {
        let mut status = GraphStatus {
            running: true,
            ..GraphStatus::default()
        };
        assert_eq!(
            GraphView::new(&status, true, true, false).label,
            "抽出中 · 未抽出 0 件"
        );
        status.live_halted = true;
        status.pending = 4;
        assert_eq!(
            GraphView::new(&status, true, true, false).label,
            "会議中に停止 · 未抽出 4 件"
        );
        status.running = false;
        status.live_halted = false;
        assert_eq!(
            GraphView::new(&status, false, true, false).label,
            "未抽出 4 件"
        );
        status.rebuilding = true;
        assert_eq!(
            GraphView::new(&status, false, true, false).label,
            "作り直し中"
        );
        status.rebuilding = false;
        status.pending = 0;
        assert_eq!(
            GraphView::new(&status, false, true, false).label,
            "未抽出 0 件"
        );
    }

    #[test]
    fn describes_each_startup_failure() {
        for (reason, message) in [
            (
                DisabledReason::Platform,
                "議論グラフは Windows で利用できます",
            ),
            (DisabledReason::Disabled, "議論グラフの設定が無効です"),
            (
                DisabledReason::DisabledDuringRun,
                "設定で無効にしたため、議論グラフの処理を停止しました",
            ),
            (
                DisabledReason::Provider,
                "要約が Codex CLI のため議論グラフは使えません。要約のプロバイダを Claude CLI か API に変えると使えます",
            ),
            (
                DisabledReason::Backend,
                "要約の API キーまたは CLI の設定を確認してください",
            ),
        ] {
            assert_eq!(disabled_message(reason), message);
            let status = GraphStatus {
                disabled_reason: Some(reason),
                ..GraphStatus::default()
            };
            assert_eq!(GraphView::new(&status, false, true, false).label, message);
        }
    }

    #[test]
    fn recording_and_rebuild_disable_both_actions() {
        let mut status = GraphStatus {
            pending: 5,
            ..GraphStatus::default()
        };
        let idle = GraphView::new(&status, false, true, false);
        assert!(idle.can_continue && idle.can_rebuild);
        let recording = GraphView::new(&status, true, true, false);
        assert!(!recording.can_continue && !recording.can_rebuild);
        status.rebuilding = true;
        let rebuilding = GraphView::new(&status, false, true, false);
        assert!(!rebuilding.can_continue && !rebuilding.can_rebuild);
    }

    #[test]
    fn settings_and_running_jobs_gate_actions() {
        let mut status = GraphStatus {
            pending: 1,
            ..GraphStatus::default()
        };
        let disabled = GraphView::new(&status, false, false, false);
        assert!(!disabled.can_continue && !disabled.can_rebuild);
        assert_eq!(disabled.action_reason, Some("設定で無効"));
        status.running = true;
        let running = GraphView::new(&status, false, true, false);
        assert!(!running.can_continue && running.can_rebuild);
        let empty = GraphView::new(&GraphStatus::default(), false, true, false);
        assert!(!empty.can_continue && empty.can_rebuild);
    }

    #[test]
    fn accepted_continue_disables_both_actions_until_it_starts_fails_or_times_out() {
        let mut status = GraphStatus {
            pending: 5,
            ..GraphStatus::default()
        };
        let previous = status.clone();
        let elapsed = Duration::from_secs(29);
        let pending = continue_is_pending(&status, &previous, elapsed);
        assert!(pending);
        let view = GraphView::new(&status, false, true, pending);
        assert!(!view.can_continue && !view.can_rebuild);
        assert!(!continue_is_pending(
            &status,
            &previous,
            Duration::from_secs(30)
        ));
        status.running = true;
        assert!(!continue_is_pending(&status, &previous, elapsed));
        status.running = false;
        status.disabled_reason = Some(DisabledReason::Backend);
        assert!(!continue_is_pending(&status, &previous, elapsed));
        assert!(continue_is_pending(&status, &status, elapsed));
    }

    #[test]
    fn continue_releases_when_the_previous_failure_reason_clears() {
        let previous = GraphStatus {
            disabled_reason: Some(DisabledReason::Backend),
            ..GraphStatus::default()
        };
        assert!(!continue_is_pending(
            &GraphStatus::default(),
            &previous,
            Duration::from_millis(250),
        ));
    }

    #[test]
    fn continue_releases_after_a_new_failure_with_the_same_reason() {
        let previous = GraphStatus {
            disabled_reason: Some(DisabledReason::Backend),
            failure_count: 1,
            ..GraphStatus::default()
        };
        let status = GraphStatus {
            failure_count: 2,
            ..previous.clone()
        };
        let elapsed = Duration::from_millis(250);
        assert!(continue_is_pending(&previous, &previous, elapsed));
        assert!(!continue_is_pending(&status, &previous, elapsed));
    }

    #[test]
    fn continue_releases_after_progress_between_polls() {
        let elapsed = Duration::from_millis(250);
        let previous = GraphStatus {
            pending: 5,
            ..GraphStatus::default()
        };
        let status = GraphStatus {
            pending: 3,
            ..previous.clone()
        };
        let pending = continue_is_pending(&status, &previous, elapsed);
        assert!(!pending);
        let view = GraphView::new(&status, false, true, pending);
        assert!(view.can_continue && view.can_rebuild);

        let previous = GraphStatus::default();
        let status = GraphStatus {
            rev: 1,
            ..previous.clone()
        };
        assert!(!continue_is_pending(&status, &previous, elapsed));
    }

    #[test]
    fn pending_count_failure_preserves_count_and_actions() {
        let mut pending = HistoryPending::default();
        pending.update(Ok(5));
        pending.update(Err("busy"));
        assert!(pending.failed);
        assert_eq!(pending.count, Some(5));
        let status = GraphStatus {
            pending: pending.count.unwrap(),
            ..GraphStatus::default()
        };
        let view = GraphView::new(&status, false, true, false);
        assert!(view.can_continue && view.can_rebuild);

        pending.update(Ok(0));
        assert!(!pending.failed);
        assert_eq!(pending.count, Some(0));
    }

    #[test]
    fn capture_state_is_only_read_on_history_entry_or_after_activity() {
        assert!(!should_read_capture_state(false, false, false, false, true));
        assert!(should_read_capture_state(true, false, false, false, false));
        assert!(!should_read_capture_state(true, true, false, false, false));
        assert!(!should_read_capture_state(true, true, true, true, true));
        assert!(should_read_capture_state(true, true, true, false, false));
        assert!(should_read_capture_state(true, true, false, false, true));
    }

    #[test]
    fn rebuild_confirmation_identifies_the_session() {
        assert_eq!(
            rebuild_confirmation("2026-11-21 12:30:00"),
            "セッション（2026-11-21 12:30:00）のグラフを作り直すと、承認・却下の結果と決定・宿題の変更履歴も消えます。"
        );
    }
}
