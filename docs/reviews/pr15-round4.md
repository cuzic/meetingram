# PR #15 レビュー（round 4）

対象: cuzic/meetingram feat/graph-consumer @ 88f9d91fac323a44822ab4f2b9e7a3f203e91fc8

## 結論（先に）
converged: no（Major 1）。round 3 の M1 は解消した（停止の時点で未抽出 0 件でも、停止の後に整理が 120 秒で 1 回だけ走る）。
ただし、整理するかどうかの判定が入ったことで、受け入れ条件「失敗で止めた流し切りでは整理しない」のテストが効かなくなった。流し切りが 3 回の失敗で止まったときに整理する変異を入れても、全テストが通る（M1）。挙動は正しい。

## 前ラウンドの解消状況
- M1（round 3）: 解消。round 3 のプローブ（`graph_batch_utterances = 1`、1 行を会議中に抽出し、`pending == 0` で停止）をこの head で走らせると、停止の後の呼び出しは `[120s]` の 1 回、イベントは `[Live, Consolidate]`。PR の `stopped_consolidates_when_live_extraction_has_caught_up` も同じことを確かめる。変異: 停止後を `flush(false)` に戻すとこのテストが落ち、Continue を `flush(true)` にすると `continue_only_consolidates_after_a_new_extraction_patch` が落ちる。再文字起こしは `increment` だけで `flush` を通らないので、整理しない（変更なし）。

## 確認したこと
- CI: 3 ジョブ pass。Windows・macOS のログで `graph_consumer::tests` の 19 件が全部 `ok`。
- Linux: これまでと同じ一時 crate（コミットしない）で 26 件全部 pass（プローブを含む）。
- 今回の差分（`bdbcf9b..88f9d91`）は、停止後の `flush(true)`、引数の名前、テスト 1 件だけ。録音の経路、ロック、DB の呼び方は変わらない。

## Findings
### M1（Major）: 「失敗で止めた流し切りでは整理しない」を守るテストが無くなった
- 根拠: `graph_consumer.rs:672-673` の `if failures >= FAILURE_LIMIT { return; }` を `break;`（止めた後も整理へ進む）にしても、全テストが通る（round 1 では `failed_flush_is_bounded_and_skips_consolidation` と `three_live_failures_…` が落ちていた）。理由は 2 つ。`failed_flush_is_bounded_and_skips_consolidation`（`:1375`）は Continue で、パッチを 1 件も保存しないので、`break` でも `:679` の判定で整理を飛ばす。生きているノードも 0 件。round 2 からは `three_live_failures_…` の流し切りが成功する。今は整理を必ず行う経路（停止後と作り直し）で、流し切りが失敗で止まる場合をどのテストも通らない。ADR タスク 5 の受け入れ条件と ADR §3（「失敗で止めたときは 3 の整理を行わない」）に当たる。
- 修正案: 会議中の抽出で生きているノードを 1 つ作り、停止の時点で未抽出を残す。その後の流し切りで 3 回失敗するダミーを使い、停止の後に 120 秒の呼び出しが無く、`Consolidate` のイベントも出ないことを確かめるテストを足す。`break` の変異で落ちることを PR 本文に書く。

## 判定
converged: no（Blocker 0 / Major 1 / Minor 0 / nit 0）
