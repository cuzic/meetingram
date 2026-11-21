# PR #15 レビュー（round 7）

対象: cuzic/meetingram feat/graph-consumer @ 5f60869249a403c5d57019fe3e707a282a9d49b0

## 結論（先に）
converged: yes（Blocker 0 / Major 0）。round 6 の M1（整理で 1 回の再試行を外していた）、m1（流し切りが失敗で止まった後の印のテスト）、n1（`pending_count` の注記）は、変異で落ちるテストにより解消を確かめた。今回の差分（`0ba6cb6..5f60869`）に新しい Blocker・Major は無い。

## 前ラウンドの解消状況
- M1: 解消。整理だけ再試行しない分岐を消し、整理も増分と同じく 1 回だけ再試行する。整理の入力の上限は `INPUT_LIMIT`（再試行の注記の分を空けた値）になった。新しいテスト `consolidation_retries_each_unreadable_stage_once` は、段 1・段 2 のそれぞれで、読めない応答の後に同じ 120 秒で注記付きの再試行が 1 回だけあり、段 1 の結果が段 2 の入力に入ることを確かめる。`invalid_issue_response_…` は、段 1 が 2 回続けて読めないと段 2 を呼ばない、に直った。一時 crate（コミットしない）での変異: 整理だけ再試行しない分岐を戻すと 2 件落ちる。再試行そのものを消すと 3 件落ちる。
- m1: 解消。`failed_flush_leaves_consolidation_pending_without_a_new_patch` は、整理が済んだ後に流し切りが 3 回失敗し、その後に新しいパッチなしの Continue で整理が 1 回呼ばれることを確かめる。停止の直前で印を入れる変異で落ちる。
- n1: 解消。`pending_count` に「書きのトランザクションで取り込んでから数える。録音中のセッションには `status().pending` を使う」と注記がある。

## 確認したこと
- 差分: 製品のコードの変更は、上の分岐の削除、整理の上限 2 か所の `INPUT_LIMIT` への変更、doc コメントだけ。録音の経路、ロック、DB の呼び方は変わらない。
- CI: 4 ジョブ pass（Linux、Windows・macOS の build、e2e-windows）。Windows・macOS のログで `graph_consumer::tests` の 26 件が全部 `ok`。
- Linux の一時 crate で 32 件全部 pass。

## Findings
なし。

## 判定
converged: yes（Blocker 0 / Major 0 / Minor 0 / nit 0）
