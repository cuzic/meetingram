# PR #15 レビュー（round 9）

対象: cuzic/meetingram feat/graph-consumer @ 98b3b9dfb245700ddc907b922e411e808698e611

## 結論（先に）
converged: yes（Blocker 0 / Major 0）。段 1 の文脈（`PatchContext::IssueConsolidation`）を付けて保存するのは、整理の段 1 の応答だけ。読めなかった応答の再試行の後も同じ文脈で保存する（プローブで確かめた）。段 2・増分・流し切りは `apply_patch` のままで、`source` も今までどおり。ログに足したのは `issue_stage` の列挙値とセッション ID だけで、本文は出ない。main の取り込みは文書だけで、v0.5.0 の app-service の変更への追従（`LiveTranscriptionOptions`）は挙動を変えない。Minor 1 件（増分に文脈を付けても落ちるテストが無い）。

## 前ラウンドの解消状況
round 8 で converged。round 8 の m1（整理の段の間で無効にしたときに印が残ることのテストが無い）と n1 は、今回の範囲の外で、まだ残っている（follow-up）。今回は PR #15 の最新の「## [codex] タスク仕様」（v0.6.0 への追従）を見た。

## 確認したこと
- 範囲: ff8a754 の後の変更は `Cargo.toml`・`Cargo.lock`・`graph_consumer.rs`・`recording.rs` だけ（仕様の触ってよい範囲）。08576a0 は main（e8dec58）の取り込みで、差分は `docs/adr`・`docs/reviews` だけ。main の ADR 0001 r13・ADR 0002 はそのまま入っている。
- 依存: 15 crate すべて tag `v0.6.0`。`Cargo.lock` の 21 か所すべて `d604771`（tag の commit と一致）。`rhai` は `=1.27.0` のままでコアとそろう。
- 文脈の付け方（`graph_consumer.rs`）:
  - 段 1: `:713-718` で `PatchSource::Consolidate` と `PatchContext::IssueConsolidation`。保存は `:782-791` の `apply_patch_with_context`。
  - 段 2: `:730` で `PatchContext::Other`。増分・流し切り・再文字起こし: `:655` で `PatchContext::Other`。どちらも `:793` の `apply_patch` を通る（コアでは `apply_patch_with_context(.., Other)` と同じ）。
  - 再試行: 文脈は `apply` の引数で、`complete` の 1 回目と再試行のどちらの応答も同じ保存の呼び出しに行く。プローブ（下）で確かめた。
- ログ: `:851-853` は `session_id` と `?issue_stage`（`FullAnnotations` などの列挙値）だけ。`settings` のロックの後、LLM の呼び出しの直前で、再試行のときも出る。プローブでログを全部取り、本文（発話・ノード）が出ないことを確かめた。§8 の「出してよいもの」に入る。
- `recording.rs:143-144`: `LiveTranscriptionOptions::default().with_silence_gate(..)`。v0.6.0 の `live_transcription.rs` は、`stt_override` が `None` のとき、v0.4.0 と同じく credential store から選んだ provider を両方のトラックに使い、同じ `target_sample_rate_hz` にする。挙動は変わらない。macOS とその他の OS の経路はこの関数を呼ばない。
- プローブ（一時 crate。コミットしない）:
  - 段 1 の 1 回目を `invalid JSON`、再試行を `remove_edge`＋付け直しの `add_edge` にする: 親は `n2` に変わる（再試行でも文脈が付く）。
  - 段 1 を `remove_edge` だけにする: 親が外れ、`GraphUpdated` は `added 0 / merged 0 / rejected 0`、`rev` は進む（n1）。
- 変異（一時 crate）:
  - 段 1 の文脈を `Other` にする: `only_issue_consolidation_can_replace_a_saved_parent` が落ちる（仕様の受け入れ条件どおり）
  - 段 2 に段 1 の文脈を付ける: 同じテストが落ちる
  - 増分（`:655`）に段 1 の文脈を付ける: 全部通る（m1）
  - `issue_stage` のログを消す: 全部通る（n2）
- CI: head 98b3b9d で 4 ジョブ pass（e2e-windows を含む）。Windows・macOS のログで新しいテストが `ok`。Linux の一時 crate で 35 件 pass、Clippy `-D warnings` pass。`cargo test -p desktop-cli` 5 件 pass。`cargo fmt --all --check` pass。

## Findings
### m1（Minor）: 増分・流し切りに段 1 の文脈を付けても落ちるテストが無い
- 根拠: `graph_consumer.rs:655` の `PatchContext::Other` を `PatchContext::IssueConsolidation` に変えても、全テストが通る。新しいテストは段 2 の `remove_edge` が落ちることだけを見る。この変異が入ると、増分のプロンプト（v0.6.0 で「付けた親が後で合わなくなったら整理の段 1 で外します」と書く）に応じた LLM が増分で返す `remove_edge` が保存され、ADR §2 の表の `remove_edge` 行（段 1 以外では落とす）に反する。今の実装は正しい。
- 修正案: 会議中か流し切りの応答に `remove_edge` を入れ、親が残り `rejected` が 1 になることを確かめるテストを足す。

### n1（nit）: 段 1 が `remove_edge` だけのとき、`GraphUpdated` の件数がすべて 0
- 根拠: プローブで `(Consolidate, added 0, merged 0, rejected 0)`。`:798-807` は `AddNode`/`AddEdge` と `Merge` だけを数える。ADR §4 のペイロード（`{rev, source, added, merged, rejected}`）に外した件数の欄は無く、今は購読者もいない。後で購読者を足すときは、件数ではなく `rev` で読み直す前提を書いておくとよい。

### n2（nit）: `issue_stage` のログにテストが無い
- 根拠: `:851-853` を消しても全テストが通る。本文が出ないことは既存の `sends_model_override_and_keeps_transcript_out_of_logs` と同じ形のプローブで確かめた。仕様の受け入れ条件には無いので nit。

## 判定
converged: yes（Blocker 0 / Major 0 / Minor 1 / nit 2）
