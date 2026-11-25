# ADR 0006 / 0007 レビュー（round 1）

対象: cuzic/meetingram docs/adr-0006-toc-diagrams（PR #42）@ 752503e9fe94862cb17511fd76f08647f6a5337b
（`docs/adr/0006-conflict-cloud.md`、`docs/adr/0007-current-reality-tree.md`）

## 結論（先に）
converged: no。owner の決定（推測も出して印を付ける／ボタンを押したときだけ呼ぶ／対立解消図が先）、
送り先と codex CLI の扱い、ログ、ロックの取り方、録音から独立して動くことは ADR 0001・0004 と合っている。
ただし、作り直しと保存の競合（照合と書き込みが原子的でない）、「〔推測〕」を足すと svg-diagram の 120 文字の上限を超えること、
`stated` が nid だけで成り立ち推測の印を機械的に保てないこと、CRT の検証の順序が決まっておらず推測の UDE が残りうることの 4 点は、
実装の前に ADR で決める必要がある。CI（Linux・Windows・macOS）は緑（ADR だけの変更）。

## 前ラウンドの解消状況
初回のため無し。

## 確かめた事実
- svg-diagram v0.10.0 の `cloud`: `a`・`b`・`c`・`d`・`d2` は必須で各 `maxLength: 120`、`assumptions` は 12 件まで、`on` は 5 値、
  `injection` は 120 文字、`conflictLabel` は 20 文字、箱ごとの tone は無い（ADR 0006 の背景どおり）。
- `causeTree`: ノード 30・リンク 45、`role` に `ude`・`effect`・`root`、ノードの `tone`、リンクの `line`、AND は `from` に 2〜5、
  `coreDriver`・`coreDriverLabel`（40 文字）、`tree: "crt"`、`dir: "up"`（ADR 0007 の背景どおり）。
- `renderer.ts` は `version !== 1` を `invalid_input` にし、`mindmap` だけを描く。`valid_svg`（`apps/discussion-diagram/src/lib.rs:317`）の要素は
  `svg`・`title`・`desc`・`style`・`rect`・`path`・`text`・`tspan` だけ（ADR 0006 の背景どおり）。
- 世代: `GraphStatus::graph_generation`（`apps/desktop/src/graph_consumer.rs:64`）は実行時の値で、作り直しは
  `delete_session` と世代の加算を同じ `db()` のクロージャ（`GraphStore` の Mutex の中）で行う（`graph_consumer.rs:588-597`）。
- コア origin/main の `GraphStore::open` は `schema.sql` を `CREATE TABLE IF NOT EXISTS` で流すだけで、表を足すのに移行は要らない
  （`graph_consolidations` が前例）。`delete_session` は 6 表を 1 つの `BEGIN IMMEDIATE` で消す。`CompletionRequest` は `system`・`user`・`model`・`timeout`。
- `build_backend` は `provider.supports_graph()` で codex CLI を外し、`graph_model` を `resolve_model` で使う（ADR 0006 §3 の起動の条件と合う）。

## Findings

### M1（Major・ADR 0006 §3・§2）: 完了の照合と保存が原子的でなく、作り直しの後に古い分析が新しいグラフの図として残りうる
- 根拠: §3 は「呼び出しの前に控えた世代と完了の時点の値が違えば捨てる」とだけ書き、照合と `graph_analyses` への書き込みの位置関係を決めていない。
  作り直しは `delete_session` と世代の加算を 1 つの `db()` のクロージャで行う（`graph_consumer.rs:588-597`）。分析が「照合 → 別の `db()` で書き込み」と実装すると、
  その間に作り直しの削除が入り、削除の後に古い分析の行が書かれる。作り直しで `nid` は `n1` から振り直され、`rev` も 0 から進み直す
  （`GraphStatus` の注記「A rebuild can reuse revision numbers」）。このため、`subject`（`"{issue_nid}:{pos}:{pos}"`）が新しいグラフの別のノードに当たり、
  `based_on_rev` が今の `rev` 以上なら「グラフが進んでいます」も出ない。中身の違う図が、新しいものとして黙って表示される。
- 修正案: 「世代と再構築中かの照合と `graph_analyses` への書き込みは、同じ `db()` のクロージャの中で行う（`delete_session` と世代の加算と同じ Mutex の下）」と §3 に書き、
  タスク 3 の受け入れ条件に「照合の後・書き込みの前に作り直しが割り込む」テストを足す。

### M2（Major・ADR 0006 §5・§6）: 120 文字に切った推測の要素に「〔推測〕」を足すと、`cloud` の上限 120 を超えて描けない
- 根拠: §5 の検証は各要素を 120 文字で切る。§6 は推測の要素の文頭に「〔推測〕」（4 文字）を付ける。svg-diagram の `cloud` は `a`〜`d2`・前提の `text`・`injection` が
  `maxLength: 120`（`list_diagram_parts` で確認）。116 文字を超える推測の要素が 1 つあるだけで最大 124 文字になり、描画が失敗する（`render_failed` か `invalid_input`）。
  A と前提はほぼ推測なので、長めの応答で常に起こる。
- 修正案: 推測の要素は「〔推測〕」の分を引いた 116 文字で切る（または描画の投影で、印を付けた後に 120 文字で切る）と決め、120 文字ちょうどの推測の要素の fixture をタスク 3 の単体テストに入れる。

### M3（Major・ADR 0006 §1・I1、ADR 0007 §3）: `stated` が入力のどの nid 1 つでも成り立ち、推測の印（owner の Q1）を機械的に守れない
- 根拠: I1・§1 は「`stated` は入力の `uid` か `nid` を 1 つ以上根拠に持つ」。入力には題材の issue・2 つの position・argument の nid がすべて載るので、
  LLM が A や前提に題材の issue の nid を付けて `stated` と書けば、検証は必ず通る。ADR が「悪い点」で認めているのは「`uid` が入力にあれば通る」ことで、
  `uid` を 1 つも出さなくても nid だけで通ることは書いていない。ADR 0007 も同じ規則で、UDE を nid だけで `stated` にでき、「推測の困りごとを作らない」が効かない。
  さらに ADR 0007 §2 は上限を超えると根拠の発話を外すので、その回は nid だけが根拠になり、すべての `stated` がこの抜け道を通る。
- 修正案: `stated` には入力に載せた発話の `uid` を 1 つ以上必須にし、`nids` は関係するノードを示すだけにする。nid だけを許すなら、要素ごとに許すノードの種類を限る
  （例: B・C は D・D2 に `supports` する argument だけ。A・前提・打ち手は不可。CRT の UDE は argument か issue だけ）。
  ADR 0007 §2 で根拠を外した回の扱い（UDE が残らず「作れなかった」になる）もあわせて書く。

### M4（Major・ADR 0007 §3）: 検証の表の適用順が決まっておらず、根拠で格下げされた UDE が UDE のまま残りうる
- 根拠: 表は「事象（`ude` で `inferred` → `effect`）」が「根拠（残らなければ `inferred` に格下げ）」より上にある。上から順に実装すると、
  根拠の検証で `inferred` に落ちた UDE は `effect` に直されず、推測の UDE が描かれる（ADR 0007 の「推測の困りごとを作らない」と ADR 0006 の Q1 の印に反する）。
  同じく、30 件への削減・閉路の除去・45 本の上限・孤立の除去・「作れなかった」の判定の順序で結果が変わる（例: 30 件に削ってからでないと孤立が決まらない、閉路で落としたリンクで孤立が増える）が、順序が無いのでタスク 1 の受け入れ条件「検証の表の行ごと」のテストの期待値が一意に決まらない。
- 修正案: 適用順を明記する（例: 根拠の検証と格下げ → `role` の補正（推測の `ude` を `effect` に）→ 事象の重複・空・切り詰め → つながりの端点・AND・重複 → 閉路 → 30 件 → 45 本 → 孤立 → 「作れなかった」）。根拠で格下げされた UDE が `effect` になるテストを受け入れ条件に足す。

### m1（Minor・ADR 0006 §4）: 対立解消図の入力が、根拠を外しても 24,000 文字を超えるときの振る舞いが無い
- 根拠: §4 は「根拠を 1 ノード 1 件 → 外す。ノードの本文は削らない」で終わる。position 2 件と 2 段までの argument の本文だけで上限を超えたときに、
  呼ぶのか（ADR 0001 §2 の上限に反する）、切るのか、止めるのかが無い。ADR 0007 §2 は「呼ばずに範囲を選ぶよう出す」と決めている。
- 修正案: 「それでも超えるなら呼ばずに固定文を出す」（か argument を賛否の多い順に落とす）と決め、テストに入れる。

### m2（Minor・ADR 0006 §4・背景・S1）: 候補の定義が ADR 0001 §2 の「案が対立する問い」と違い、却下されたエッジの扱いも無い
- 根拠: 背景と S1 は「ADR 0001 §2 の定義（position 2 件以上、**または** `objects_to` を受けた position）」で数え上げると書くが、§4 の候補は position 2 件以上だけ。
  案 1 つに反対が付いた問い（「D をするか、しないか」）は候補にならない。どちらを採るかを揃える。また、`responds_to`・`supports`・`objects_to` を数えるとき、
  `review = rejected` のエッジを除くかが書かれていない（`lib.rs` の `a_rejected_resolves_edge_does_not_decide_the_question` が議論マップでの前例）。
  「`merged`・`rejected` でない」は状態とレビューを混ぜているので、`Node::is_live()` と生きたエッジで定義すると曖昧さが無い。
- 修正案: §4 か背景のどちらかに揃え、候補の条件を `is_live()` と却下されていないエッジで書く。

### m3（Minor・ADR 0006 §3、ADR 0007 §1）: 「題材がなくなりました」の条件が狭い
- 根拠: §3 は「題材の問いや案が却下・統合されて消えていたら」だけ。候補の条件で除く `retracted` の position は入っていない。
  ADR 0007 の `"issues:…"` の題材（議題が統合・却下されたとき）は扱いが書かれていない。
- 修正案: 「題材のノードが `is_live()` でない、または position が `retracted`」を共通の条件にし、CRT の議題を選んだ題材にも当てる。

### m4（Minor・ADR 0007 §3・§4）: 「入ってくるつながりが無い事象 = root」が、原因の無い UDE にも当たる
- 根拠: §3 の「孤立」は原因の見つからない UDE を残す。§3 の「根本原因はアプリが決める」は入るつながりの無い事象を `root` にするので、
  この UDE が根本原因として描かれ、根本原因の一覧にも出る。UDE を木に描くのか表だけに出すのかも書かれていない（描けば CRT の検査が警告を出しうる）。
- 修正案: `root` は `role = effect` の事象だけに付けると書き、原因の無い UDE は木に描かず表にだけ出すかを決める。

### m5（Minor・ADR 0007 §2）: 範囲の取り方の抜けと、議題 1 つでも超えたときの行き先
- 根拠: 議題を選んだときの範囲は「position に付く argument」までで、argument に付く argument（ADR 0006 §4 は 2 段までたどる）と、decision に `assigns` する action が入らない。
  議題を 1 つに絞っても根拠を外して上限を超える場合、「範囲を選ぶよう画面に出す」では先に進めない。
- 修正案: たどる段数を ADR 0006 と揃え、議題 1 つでも超えるときの固定文を決める。

### m6（Minor・ADR 0006 実装の分割）: spike の後のプロンプトの修正とタグの関係、spike の入力の置き場所
- 根拠: タスク 2 は「満たさなければ 1 のプロンプトを直す PR を先に出す」が、タスク 3 の依存は「1 のタグ」。プロンプトはコアの中にあるので、
  直した場合は新しいタグが要る。また spike の入力にする ADR 0002 の合成会議はアプリ repo の `apps/e2e-scripted/scenarios/` にあり、コアの spike ブランチからは見えない
  （コアにあるのは `crates/discussion-graph/testdata/` の S2 の会議だけ）。
- 修正案: タスク 3 の依存を「2 の結果を反映したコアのタグ」にし、spike にアプリの台本をどう持ち込むか（コアの testdata に合成のコピーを置く等）を書く。

### m7（Minor・ADR 0006 §6・未確認の前提）: 単純な `cloud` でも svg-diagram の描画検査が警告を出した
- 根拠: 合成の短い文言の `cloud`（前提 1 件と打ち手、`conflictLabel: "対立"`、幅 940）を svg-diagram の MCP で描くと、
  `text out of its box`（前提の番号「1」と `conflictLabel`）の警告が 2 件出た。ADR 0004 の扱いでは警告は「注意の印」を付けて出すので、
  このままではほとんどの対立解消図に注意の印が付く。MCP の版は固定の v0.10.0 と同じとは限らない。
- 修正案: タスク 3 の fixture の段で、固定版で警告が出ない幅と `conflictLabel` の有無を決め、未確認の前提に書き足す。

### n1（nit・ADR 0006 §6）: `version: 1` を受け続ける必要が無い
- 根拠: `renderer.js` は Rust と同じ実行ファイルに `include_str!` で入るので、版のずれた組み合わせは起きない。一度に 2 へ移してよい。

### n2（nit・ADR 0006 §4）: 「親の議題があればその本文」は、ほぼ空になる
- 根拠: ADR 0001 §2（r13）は案が対立する issue を必ず議題（親無し）にするので、候補の issue に親はまず無い。A の手がかりを別に考えるか、記述を外す。

### n3（nit・ADR 0006 §2・§5）: 題材の鍵は nid を昇順に並べるが、`d`・`d2` は入力の順
- 根拠: 案の組の選び方を変えて作り直すと、同じ鍵の行が `d` と `d2` を入れ替えて置き換わる。鍵の順を `d` の順にするか、`d` を昇順の側に固定する。

## 判定
converged: no（Blocker 0 / Major 4 / Minor 7 / nit 3）
