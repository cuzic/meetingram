# PR #21 レビュー（round 2）

対象: meetingram feat/graph-ui-settings @ fd45360a4bed65ed3e58b698d8243a84522b494c（新しく見たのは `b7b57f6..fd45360` のうち 0aa9978 と fd45360。`feat/graph-consumer` から入った 4144c84 は、M2 の consumer 側として挙動だけを確かめた）

## 結論（先に）
converged: yes。round 1 の Major 3 件、Minor 4 件、nit 1 件はすべて直っている。どの修正も、純粋関数のテストで変異を捉える。
新しく見つかったのは Minor 2 件で、どちらも表示の正確さに関するもの。Blocker と Major は無い。CI は 4 ジョブとも通過しており、Windows のジョブで新しいテストが全部走っている。

## 前ラウンドの解消状況
- M1 解消: 押下が受け付けられたら、その場で `continue_request` を立てる（`graph_status_view.rs` の onclick）。`GraphView::new(…, continue_pending)` は、このあいだ両方のボタンを無効にする。印は `continue_is_pending`（`graph_view.rs:31`）で下ろす。下ろす条件は、`running` を見た、前回と違う起動しなかった理由が付いた、30 秒たった、のいずれか。onclick 側も `continue_request.peek().is_none()` を確かめる。変異（`!continue_pending` を外す、30 秒の条件を外す、`!status.running` を外す、理由の比較を `is_some()` にする）は、4 つとも `accepted_continue_disables_both_actions_until_it_starts_fails_or_times_out` が落とした。
- M2 解消: consumer 側（4144c84）は、LLM を呼ぶ前に毎回 `graph_enabled` を読み直す。無効になっていれば `DisabledReason::DisabledDuringRun` を付け、会議中の抽出、流し切り、整理をやめる（`graph_consumer.rs:583` ほか）。Windows の CI で `disabling_during_recording_stops_live_calls_and_continue_can_resume` と `disabling_before_stop_blocks_flush_and_consolidation` が ok。UI 側は、いつ反映されるかの説明（`settings_timing_message`）と `DisabledDuringRun` の文言を足した。文言を変えた変異は `describes_each_startup_failure` が落とした。
- M3 解消: 表示は「抽出中 · 未抽出 N 件」になった。`live_view_never_refreshes_pending_count` は `running: true, recording: true` の状態で書き直されている。`"抽出中"` に戻す変異は 2 つのテストが落とした。
- m1 解消: `HistoryPending::update` は、失敗しても前回の件数を残し、失敗の印だけを立てる。ボタンは残り、エラーは小さく添えて出す。件数を消す変異は `pending_count_failure_preserves_count_and_actions` が落とした。
- m2 解消: 設定で無効なら `action_reason = "設定で無効"` をボタンの横に出す。変異は `settings_and_running_jobs_gate_actions` が落とした。
- m3 解消: `capture_state` を読むのは、開いたとき、忙しい状態から待機に戻ったとき、停止のイベント（`session_stopped_subject`）を受けた後、操作の後だけになった（`should_read_capture_state`）。毎回のポーリングで確かめるのは `state.current` だけ。2 つの変異は `capture_state_is_only_read_on_history_entry_or_after_activity` が落とした。
- m4 解消: Q6 の文言を仕様どおりに変えた。実装側の文字列だけを変えた変異は `describes_each_startup_failure` が落とした。
- n1 解消: 確認の文に、対象のセッションの日時を入れた（`rebuild_confirmation`）。変異はテストが落とした。

## Findings
### m1（Minor）:「有効・無効はすぐに反映されます」は、無効から有効に戻した場合には当てはまらない
- 根拠: `graph_settings_view.rs:64`。会議中に無効にすると、ライブのタスクは `Err("disabled")` で終わる（`graph_consumer.rs:583` の `return false`）。その後に有効へ戻しても、その会議の抽出は再開しない。再開できるのは、会議の後の「続きを抽出」から。録音を始めた時点で無効だった会議も、途中で有効にしても動かない。反対に、「モデルと窓の設定は次の録音から」という説明は会議中の抽出についてだけ正しい。「続きを抽出」と作り直しは、その時点の設定を読む。送信の範囲を広げる誤りではないので Minor。
- 修正案:「無効にすると、録音中の会議でもそれ以降は送信しません。有効に戻した場合と、モデルと窓の変更は、次の録音（または会議の後の『続きを抽出』『作り直す』）から使われます」のように書く。

### m2（Minor）: 実行が 250 ms 以内に始まって終わると、両方のボタンが最大 30 秒押せないままになる
- 根拠: `continue_is_pending` は、`running` を一度観測するか、前回と違う理由が付くまで true のまま。ポーリングとポーリングのあいだに実行が始まって終わると、`running` は観測されない。成功すると `mark_running` が理由を消すが、「前回と違う理由が付いた」にも当たらない。前回と同じ理由で再び失敗した場合も同じで、30 秒のタイムアウトまで待つことになる。30 秒たてば自然に戻るので Minor。
- 修正案: 押下の前の `status().pending` や `disabled_reason` を控えておき、どちらかが変わったら下ろす。または、consumer に実行の回数を持たせる（#15 の変更になるので、やるなら後で）。

## 判定
converged: yes（Blocker 0 / Major 0 / Minor 2 / nit 0）
