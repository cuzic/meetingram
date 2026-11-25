# PR #38 レビュー（round 1）

対象: cuzic/meetingram feat/svg-discussion-map @ 07335160d5ca3d2ac0cb319691c369367729dfc3
（9191c24 の図を見た指摘を 47c9f8b・17cdb72・0733516 で直したあとの head）

## 結論（先に）
converged: **yes（内容）**。Blocker・Major は残っていません。
ただしマージの前に、main（#40、core v0.14）との `Cargo.lock` の衝突を Codex が解消し、PR 自体の CI（linux・windows・macos・e2e-windows）が通る必要があります。
今の CI の結果は、衝突のため workflow_dispatch で head に対して実行したものです。

## 9191c24 で見た図の問題と、その解消
- M-a（種類の見分け）: 解消。決定は `solid`、宿題は `accent`、未回答の問いは `danger` の tone になりました（`apps/discussion-diagram/src/lib.rs` の `node_branch`）。
  賛成・反対の理由は、owner の決定（2026-11-25）で tone を付けず、文字の「賛成／反対」だけで示します。
  線・バッジでの区別は、svg-diagram の ADR 0081 を待ちます。
- M-b（矛盾したラベル）: 解消。状態は問いと宿題にだけ出します。問いの状態は辺から導きます。
  - 生きた決定が resolves で付いていれば「決定済み」
  - 案も決定も無ければ「未回答」
  - 案があって決定が無ければ「検討中」（owner の決定、2026-11-25）
- M1（47c9f8b で見つけた食い違い）: 解消。決定はあるが responds_to の無い問いが、「決定済み」なのに `danger` になっていました。
  0733516 では tone を状態から決めます。テスト `decided_question_without_answer_is_not_danger` で固定しています。
- 折り返し: 解消。`wrap: 'bunsetsu-soft'` を指定しました（v0.9.0 の既存機能）。
- 許可リスト: `rect` に `stroke-dasharray` だけを足し、値は「数字,数字」に限りました。
  fixture は tone と wrap を使った実出力で作り直しています。拒否のテストも残っています。
- 対象外にしたもの: 採用された案と決定のつながり、穴の強調（ADR 0001 §6 の残りの穴）。
  どちらも図の範囲を広げる話なので、ADR 0004 の次の版で扱います。

## 確かめたこと
- 差分: `lib.rs` の投影と許可リスト、`renderer.ts` の tone の許可値による絞り込み（`sanitize`）、ADR 0004 の追記 1 行。
- CI（0733516、workflow_dispatch）: App Build & Test は run 45604711411 で success、Windows E2E recording は run 45604715777 で success。
- E2E の `step-7-history-discussion-map.png`（47c9f8b の run 45601390078 で確認）: 決定は濃い青の塗りつぶし、宿題は薄い青。文節で折れ、「決定 · 未解決」は消えています。

## Minor・nit（follow-up でよい）
- m1: 撤回された議題（親の無い issue）で responds_to が無いものは、`detect_gaps` が未回答に数えるため、「未回答」と `danger` になります。
  `node_branch` の判定では「撤回済み」が「未回答」より後にあります。撤回を先に判定するか、`detect_gaps` 側で除くかを決めてください。
- n1: e2e-windows は、47c9f8b の 1 回目が step-13 の後の `Tour did not reach second_session` で失敗し、再実行で通りました。
  今回の変更とは無関係に見えますが、2 度目が出たら Issue にしてください。
