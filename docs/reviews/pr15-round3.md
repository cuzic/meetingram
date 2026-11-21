# PR #15 レビュー（round 3）

対象: cuzic/meetingram feat/graph-consumer @ bdbcf9bd16d161fe2d58148f092935070f39dc79

## 結論（先に）
converged: no（Major 1）。round 2 の M1（アプリが 3 本目の接続でグラフの表を読む）は解消した。
ただし新しい判定「その回の流し切りが抽出のパッチを保存したときだけ整理する」は、会議の後の流し切りにも当たる。会議中の抽出が追いついていて、停止の時点で未抽出が 0 件だと、会議の後の整理が 1 回も走らない（M1）。ADR §3 は、`stopped` の後の流し切りに続けて、整理を必ず 1 回行うと決めている。この判定は round 2 の修正案 (b) で私が書いたものそのままで、会議中に保存したパッチを数えていなかった。修正案の誤り。

## 前ラウンドの解消状況
- M1（round 2）: 解消。`apps/` と `Cargo.toml` に `rusqlite` も `graph_patches` も無く、`Cargo.lock` の desktop の依存からも `rusqlite` が消えた。整理するかどうかは、メモリ上の `saved_flush_patch` と、生きているノードの有無（`load_graph` は `GraphStore` 経由）で決める。判定（`graph_consumer.rs:679`）を外すと `continue_only_consolidates_after_a_new_extraction_patch` が落ちる。

## 確認したこと
- CI: 3 ジョブ pass。Windows・macOS のログで `graph_consumer::tests` の 18 件が全部 `ok`。
- Linux: round 1・2 と同じ一時 crate（コミットしない）で全件 pass。
- ほかの変異（作り直しで `flush(false)` にする、`saved_flush_patch` を流し切りの初めに戻さない、ソースを問わず立てる）はどのテストも落とさない。ただし、どれも今の呼び出し方では挙動が変わらない（作り直しは消した後に必ず抽出のパッチができる。1 つの `Run` で `flush` を 2 回呼ばない。フラグは整理の前に読む）。指摘にしない。
- 今回の差分（`b1f8823..bdbcf9b`）は `flush` の判定と、`Run` のフィールド 1 つだけ。録音の経路、ロック、DB の呼び方は変わらない。

## Findings
### M1（Major）: 会議中の抽出が追いついていると、会議の後の整理が走らない（ADR §3 に反する）
- 根拠: 会議中の実行者は停止の後に `run.flush(false)` を呼ぶ（`graph_consumer.rs:339`）。`flush` は初めに `saved_flush_patch = false` に戻し、フラグは `source == Flush` のパッチでしか立たないので、会議中の `live` のパッチは数えない。未抽出が 0 件なら、流し切りは何も保存せず、`if !rebuild && !self.saved_flush_patch { return; }`（`:679`）で整理を飛ばす。プローブ（`graph_batch_utterances = 1`、1 行を会議中に抽出させてノードを作り、`pending == 0` を待ってから停止）では、停止の後の呼び出しは 0 回、イベントは `[Live]` だけだった。最後の発話から 45 秒以上たってから停止すれば、未抽出は 0 件になる（よくある）。ADR §3「会議の後（流し切り→整理）: `session.{sid}.stopped` の後に 1 回だけ行う … 3. 整理を 1 回行う」、つまり重複の `merge` の主な機会が、黙って失われる。既存のテストは、どれも停止の時点で未抽出がある場合しか見ていない。
- 修正案: 会議中の実行者の停止後の流し切りは、作り直しと同じく必ず整理する（`flush(true)`。流し切りが失敗で止まったら整理しないのと、生きているノードが 0 件なら飛ばすのはそのまま）。判定を「保存したパッチがあれば」にするのは「続きを抽出」だけにする。停止の時点で未抽出 0 件・生きているノードがある会議で、停止の後に整理（120 秒）が 1 回呼ばれるテストを足す。

## 判定
converged: no（Blocker 0 / Major 1 / Minor 0 / nit 0）
