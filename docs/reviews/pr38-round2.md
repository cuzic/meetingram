# PR #38 レビュー（round 2）

対象: cuzic/meetingram feat/svg-discussion-map @ d21c323120c50053f065bddb25a2fabbd50add94

## 結論（先に）
converged: **yes**。PR 全体（ADR 0004、投影、SVG の検査、WebView2 のアダプター、表示と保存、E2E）を読み直しました。
Blocker は 0 件です。Major は M1 の 1 件で、ADR 0004 が Proposed のまま実装されていたことです。owner が 2026-11-25 に図種と配布方式を承認したので、このコミットで Accepted にして解消しました。
残りは Minor 3 件と nit 1 件で、follow-up に回せます。

## 前ラウンドの解消状況（pr38-round1）
- M-a・M-b・M1: 解消のまま。d21c323 は main の取り込みだけで、差分は `Cargo.toml`、`Cargo.lock`、`scripts/ci/windows-e2e-scenario.ps1` の main 側の変更です。
- m1（撤回された議題が未回答の赤になる）: 未対応。下の m1 に引き継ぎます。
- n1（e2e-windows の不安定さ）: 未対応。PR の CI（run 45606966731）は 1 回で通りました。

## 確かめたこと
- CI（d21c323、pull_request）: linux、windows、macos、e2e-windows がすべて pass です。
  linux では renderer.js を Node 22.18.0 で再生成し、差分が無いことを検査しています（`npm ci --ignore-scripts`）。
- 手元: `cargo fmt --all --check` と `cargo test -p discussion-diagram`（8 passed）が通りました。
- 録音を止めないか:
  - グラフの読み取り（`GraphConsumer::load_graph`）は、グラフ専用の接続（`db()`、`spawn_blocking`）を通ります。録音の SessionStore の Mutex は取りません。
  - 投影と SVG の検査も `spawn_blocking` で動きます。
  - 描画は 15 秒で打ち切ります。失敗は画面の文言になるだけで、録音には触れません。
- 個人データ:
  - JS は例外を握って分類コードだけを返し、console には出しません。
  - Rust 側のエラー文は固定の文言だけです。
  - ラベルはログに出ません。E2E の入力は合成データです。
  - 新しい外部送信はありません。描画はオフラインで動き、rfd の保存ダイアログはローカルのファイルに書くだけです。
- SVG の検査（`valid_svg`）: 次を確かめました。
  - 拒否するもの: DTD・処理命令、svg 名前空間の外の要素、許可リストに無い要素・属性、`url(...)` を含む色。
  - `style` は固定の 2 規則と完全に一致するものだけを通します。
  - `stroke-dasharray` は「数字,数字」だけを許します。
  - プレビューは `<img>` の data URL なので、仮に何かがすり抜けても script は動きません。
- 古い結果が混ざらないか:
  - 図の部品は `key: "{session_id}"` で作り直されます。
  - 描画の前後と完了の時点で、世代（graph_generation）と再構築中かどうかを照らし合わせています。
  - 描画したリビジョンより新しい版があれば「更新あり」を出します。
- 2 つの repo: meetingram-core は v0.14.0 のタグ参照だけで、ブランチ参照はありません。
- 変異:
  - `stroke-dasharray` の値の検査を外す → `rejects_active_svg_and_accepts_inert_content` が落ちる。
  - 却下された resolves エッジを決定扱いにする → 落ちない（m2）。
  - 未整理の項目の上限検査を外す → 落ちない（m2）。

## Findings
### M1（Major、このコミットで解消）: ADR 0004 が Proposed のまま実装されていた
- 根拠: AGENTS.md では、実装は Accepted の ADR に沿って行います。ADR 0004 の owner 決定表では、「表示の起点と最初の図種」と「初回の配布方式」が「提案」のままでした。
- 対応: owner が 2026-11-25 に両方を承認しました。Status を Accepted にし、決定表と改訂履歴（r5）を更新しました。

### m1（Minor）: 撤回された議題に案が無いと「未回答」の赤で出る
- 根拠: `apps/discussion-diagram/src/lib.rs` の `node_branch` は、「撤回済み」より先に「未回答」を判定します。
  core v0.14.0 の `detect_gaps` は、議題を status に関係なく responds_to の有無で数えます。
- 修正案: `node_branch` で撤回を先に判定し、撤回済みの問いは danger にしない。そのケースのテストを足す。

### m2（Minor）: テストが拾わない変異が 2 つある
- 根拠: (1) `Derived::new` の `edge.review != Review::Rejected` を外しても、テストは落ちません。既存のテストが確かめているのは、却下された決定ノードの側だけです。
  (2) `project` の未整理の項目の `check_limit` を外しても、テストは落ちません。
- 修正案: 却下された resolves エッジのテストと、未整理の項目だけで上限を超えるテストを足す。

### m3（Minor）: 15 秒の打ち切りの後も JS の描画は動き続け、その間は次の描画が失敗する
- 根拠: `discussion_diagram_view.rs:68` は Rust 側の future を捨てるだけです。`renderer.ts` の `active` は、JS の描画が終わるまで true のままです。
  その間の描画要求は `busy` になり、画面には「議論を図解できませんでした」と出ます。部品を閉じたとき（セッションの切り替え）も同じです。
- 修正案: `busy` を専用の文言（「前の描画が終わるまでお待ちください」）にする。直すのは、実機の計測で 15 秒に近づく図が出てからでよい。

### n1（nit）: README の説明と画面の文言の確認
- 根拠: README は、「議論を図解」を開いたあと、ボタンが「図を更新」に変わるとしています。画面も描画後は「図を更新」に変わるので、矛盾はありません。
  ただし、Windows だけの機能であることを README の 1 文目で示すと読み違いが減ります。
- 修正案: 任意。

## 判定
converged: yes（Blocker 0 / Major 0（M1 は解消済み）/ Minor 3 / nit 1）
