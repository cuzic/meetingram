use crate::graph_consumer::{DisabledReason, GraphStatus};

pub(super) struct GraphView {
    pub label: String,
    pub can_continue: bool,
    pub can_rebuild: bool,
}

pub(super) fn disabled_message(reason: DisabledReason) -> &'static str {
    match reason {
        DisabledReason::Platform => "議論グラフは Windows で利用できます",
        DisabledReason::Disabled => "議論グラフの設定が無効です",
        DisabledReason::Provider => "要約が Codex CLI に設定されています",
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

impl GraphView {
    pub(super) fn new(status: &GraphStatus, recording: bool, enabled: bool) -> Self {
        let pending = format!("未抽出 {} 件", status.pending);
        let label = if status.rebuilding {
            "作り直し中".to_string()
        } else if let Some(reason) = status.disabled_reason {
            disabled_message(reason).to_string()
        } else if status.live_halted {
            format!("会議中に停止 · {pending}")
        } else if status.running {
            "抽出中".to_string()
        } else {
            pending
        };
        let can_rebuild = enabled && !recording && !status.rebuilding;
        Self {
            label,
            can_continue: can_rebuild && !status.running && status.pending > 0,
            can_rebuild,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_view_never_refreshes_pending_count() {
        let status = GraphStatus {
            pending: 4,
            ..GraphStatus::default()
        };
        assert!(!should_refresh_pending(false, false, &status, true));
        assert_eq!(GraphView::new(&status, true, true).label, "未抽出 4 件");
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
        assert_eq!(GraphView::new(&status, true, true).label, "抽出中");
        status.live_halted = true;
        status.pending = 4;
        assert_eq!(
            GraphView::new(&status, true, true).label,
            "会議中に停止 · 未抽出 4 件"
        );
        status.running = false;
        status.live_halted = false;
        assert_eq!(GraphView::new(&status, false, true).label, "未抽出 4 件");
        status.rebuilding = true;
        assert_eq!(GraphView::new(&status, false, true).label, "作り直し中");
        status.rebuilding = false;
        status.pending = 0;
        assert_eq!(GraphView::new(&status, false, true).label, "未抽出 0 件");
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
                DisabledReason::Provider,
                "要約が Codex CLI に設定されています",
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
            assert_eq!(GraphView::new(&status, false, true).label, message);
        }
    }

    #[test]
    fn recording_and_rebuild_disable_both_actions() {
        let mut status = GraphStatus {
            pending: 5,
            ..GraphStatus::default()
        };
        let idle = GraphView::new(&status, false, true);
        assert!(idle.can_continue && idle.can_rebuild);
        let recording = GraphView::new(&status, true, true);
        assert!(!recording.can_continue && !recording.can_rebuild);
        status.rebuilding = true;
        let rebuilding = GraphView::new(&status, false, true);
        assert!(!rebuilding.can_continue && !rebuilding.can_rebuild);
    }

    #[test]
    fn settings_and_running_jobs_gate_actions() {
        let mut status = GraphStatus {
            pending: 1,
            ..GraphStatus::default()
        };
        let disabled = GraphView::new(&status, false, false);
        assert!(!disabled.can_continue && !disabled.can_rebuild);
        status.running = true;
        let running = GraphView::new(&status, false, true);
        assert!(!running.can_continue && running.can_rebuild);
        let empty = GraphView::new(&GraphStatus::default(), false, true);
        assert!(!empty.can_continue && empty.can_rebuild);
    }
}
