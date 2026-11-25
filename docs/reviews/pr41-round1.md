# PR #41 レビュー（round 1）

対象: cuzic/meetingram feat/diagram-edge-tags @ c9d90d59e1420b0805cbb1314b7064866f2d177a

## 結論（先に）
converged: **no**。Major が 1 件あります。表示倍率の初期値を 100 にしたため、選択肢の表示と実際の倍率がずれ、「全体表示」を選べなくなっています。
それ以外は問題ありません。svg-diagram v0.10.0 への依存、edge と札の投影、要確認の箱の薄さ、凡例、許可リストの追加（path の `stroke-dasharray` と `stroke-linecap="round"`）、PR #38 の m1〜m3 の修正を確かめました。

## 前ラウンドの解消状況（pr38-round2 の Minor）
- m1: 解消。撤回された問いを「未回答」より先に判定します（`node_branch`）。
- m2: 解消。却下された resolves エッジのテストと、未整理の項目だけで上限を超えるテストがあります。
- m3: 解消。`busy` のときは「前の描画が終わるまでお待ちください」と出します（`discussion_diagram_view.rs`）。

## 確かめたこと
- CI（c9d90d5）: linux、windows、macos、e2e-windows が pass です。
- 手元: `cargo fmt --all --check`、`cargo test -p discussion-diagram`（12 passed）。
- 依存: tarball の URL と package-lock の integrity。dist を v0.10.0 タグのビルドと照らし合わせた記録は PR 本文にあります。
- renderer.ts の `sanitize`:
  - tone・edge・tags を許可値に絞ります。
  - 札は空を捨て、8 コードポイントまで、3 つまでです。
  - 根の edge は捨てます。
  - 凡例は固定の文字です。
- 変異:
  - 反対の線を実線にする → テストが落ちる。
  - 札の「状態」と「賛否」の順序を入れ替える → 落ちない。ただし、両者は種類で排他（問い・宿題か、理由か）なので、結果が変わらない同値の変異です。

## Findings
### M1（Major）: 表示倍率の選択肢が「全体表示」のまま、図は 100% で出る
- 根拠:
  - `discussion_diagram_view.rs` は `select { value: "{zoom_percent}", ... }` で選択値を指定し、option は子として描いています。初期値が 0 のときは、先頭の option（全体表示）と一致したので、ずれが見えませんでした。
  - 初期値を 100 にした c9d90d5 の e2e-windows の `step-7-history-discussion-map.png` では、図は 100% の大きさなのに、選択肢は「全体表示」と出ています。
  - 表示中の選択肢を選び直しても change は起きません。そのため利用者は、別の倍率を一度選ばないと全体表示に戻せません。
- 修正案:
  - `settings.rs:1367` と同じように、各 option に `selected: zoom_percent == 値` を付ける。
  - E2E の巡回で、図を開いた直後の選択肢の値が "100" であることを DOM で確かめる。

### m1（Minor、follow-up）: E2E の合成グラフでは、全項目が要確認（薄い箱）になる
- 根拠: 合成データの確信度は 0.8、しきい値は 0.9 です。確認の一覧の段が低確信度の項目に頼っているため、変えられません（PR 本文の Open question）。
  決定・宿題・未回答の色は、fixture と単体テストでだけ確かめています。
- 修正案: 確認の一覧の段で使う項目だけを低確信度に残し、他を上げる。巡回の段の変更を含むので、別の PR にする。

## 判定
converged: no（Blocker 0 / Major 1 / Minor 1 / nit 0）
