# PR #27 レビュー（round 1）

対象: meetingram feat/graph-outline @ 7d64b0480de7b333db94cafbfc2303949f6a9c69

## 結論（先に）
converged: no。履歴の「アウトライン」でセッションを切り替えても、前のセッションのグラフを出し続ける（M1）。
`key` がテンプレートの根に無いため効かず、`use_future` が最初のセッション ID のまま読み続ける。実機確認の手順 4 はこれを確かめるが、Dioxus 0.7.10 の最小プローブで再現した。
ライブ側の `rev` による読み直し、フォローアップ 4 件（pr15 r8 m1、r9 m1、pr21 r2 m1・m2）は仕様どおりで、変異も捉える。Minor 3 件と nit 2 件は follow-up でよい。

## 前ラウンドの解消状況
- round 1 のため無し。フォローアップの確認:
  - pr15-round8 m1 解消: `disabling_between_consolidation_stages_leaves_consolidation_pending` は、段 1 の呼び出し中に無効にし、段 2 が呼ばれないこと（呼び出し 2 回）と、有効に戻した後の新しいパッチ無しの「続きを抽出」で整理が走ることを確かめる。`"disabled"` で整理済みの印を入れる変異では、再開時の更新が空になり落ちる。
  - pr15-round9 m1 解消: `only_issue_consolidation_can_replace_a_saved_parent` は、流し切りの `remove_edge` が `rejected = 1`（`events[1]`）になることを見る。流し切りに段 1 の文脈を付ける変異では 0 になり落ちる。
  - pr21-round2 m1 解消: `settings_timing_message` は、無効の即時反映、有効に戻しても録音中は再開しない、会議後の「続きを抽出」で再開、モデルと窓の適用時点を書き分けている。テストも各文を見る。
  - pr21-round2 m2 一部解消: 押下時の `GraphStatus` を控え、`pending`・`disabled_reason`・`rev` のどれかが変われば印を下ろす。成功（`rev` が進む）は `running` を見逃しても下ろす。同じ理由での失敗は残る（m3）。

## 確認したこと
- 範囲: 変更は `graph_consumer.rs`・`graph_outline_view.rs`（新規）・`graph_view.rs`・`ui.rs`・`graph_settings_view.rs`・`graph_status_view.rs` だけ（Issue #26 と追加の仕様の範囲）。承認・却下や図は入っていない。ログ出力の追加は無い（`tracing` / `println` の追加 0 件）。発話の本文は画面にだけ出る。
- CI: `7d64b04` で linux・windows・macos が pass。Windows ジョブのログで、新しい 8 テスト（`outline_lists_subquestions_first_and_unattached_last`、`reloads_only_when_revision_changes_or_a_graph_is_rebuilt`、`status_revision_tracks_each_saved_patch_and_survives_executor_release`、`disabling_between_consolidation_stages_leaves_consolidation_pending` ほか）がすべて `ok`（99 passed）。e2e-windows は pending のまま。
- 手元（Linux）: `cargo fmt --all --check` pass。`desktop` は gdk-3.0 が無く Linux ではビルドできないので、UI はコードを読んで確かめ、Dioxus の差分の挙動は下のプローブで確かめた。
- `rev` の読み直し: `apply` の DB クロージャの中で、保存に成功したパッチの `graph.rev` を `revisions` に書く（`graph_consumer.rs:818`）。コアの `apply_patch_with_context` は保存のたびに `before.rev + 1`（core v0.7.0 `store/mod.rs:191`）。作り直しは `delete_session` と同じクロージャの中で `rev = 0`・世代 +1。グラフの書き込みはこの 2 か所だけ（`apply_patch` / `delete_session` の呼び出しを grep）。ポーリングは毎回 `status()`（`sessions` のロックだけ）を読み、`(rev, 世代)` が変わったときだけ `load_graph` を呼ぶ。
- ロック: 新しく「グラフのストア → `sessions`」の順でロックを取る。逆順（`sessions` を持ったままストアを取る）箇所は無い（ストアのロックは `db()` の `spawn_blocking` の中だけで、`sessions` のガードは `!Send` なので `.await` をまたげない）。デッドロックは無い。
- 接続: `load_graph` はランナーと同じ `Mutex<Option<GraphStore>>` を `spawn_blocking` で取るので、同時に使われることは無い。UI スレッドでは DB を待たない（`HistoryOutline` の `list_sessions` は既存の `GraphHistory` と同じ同期読みで、新しい種類のものではない）。
- 入れ子と循環: コアの `build_outline` は経路の集合で循環を断つ（`argument` 同士の `supports` / `objects_to` の循環でも止まる）。`sub_issue_of` だけの循環は議題にならず「未整理」に並ぶ。
- 変異（コードを読んで確認）: しきい値の比較を `>` / `<=` にすると、`n1`（0.6、薄くしない）と `n2`（0.5、薄くする）の期待が崩れて落ちる。`should_load` を常に true にすると落ちる。

## Findings
### M1（Major）: 履歴のアウトラインでセッションを切り替えても、前のセッションのグラフと根拠を出し続ける
- 根拠: `graph_outline_view.rs:236` の `GraphOutline { key: "{session_id}", session_id }` は `section` の子で、テンプレートの根ではない。dioxus-rsx 0.7.10 は `key` をテンプレートの最初の根（`template_body.rs:279` `implicit_key`）からしか取らず、子のコンポーネントの差分（dioxus-core `diff/component.rs:117` `diff_vcomponent`）は `render_fn` が同じなら props を差し替えて同じスコープを再実行する。`use_future`（dioxus-hooks `use_future.rs`）はタスクを最初の 1 回だけ作るので、`:99-117` のループは最初の `session_id` を読み続け、`snapshot` と `selected` も前のセッションのまま残る。結果、選択欄はセッション B を示しているのに、A の議題と A の発話の本文を出す。
  最小プローブ（Dioxus 0.7.10、`VirtualDom`）で、同じ形（`section { label {…} Child { key: "{sid}", sid } }`）は sid を 1 → 2 に変えても `use_hook` の値が 1 のまま、`if true { Child { key: "{sid}", sid } }`（根）は 2 になった。ライブ側（`ui.rs:915`、`rsx!` の根）と `GraphHistory` の `GraphStatusRow`（`if let` の根）は効いている。
- 修正案: `GraphOutline` を根に置く（例: `{ rsx! { GraphOutline { key: "{session_id}", session_id } } }` か `if let Some(session_id) = … { GraphOutline { key… } }`）か、`session_id` を `ReadOnlySignal` にして `use_future` を読み直させる。切り替えで行と根拠が変わることをテストか少なくとも PR 本文の手順で確かめる。

### m1（Minor）: 読み込みに失敗すると、次に `rev` が変わるまで読み直さない
- 根拠: `graph_outline_view.rs:107-112`。失敗しても `loaded = Some(current)` にするので、履歴のセッション（`rev` が動かない）では、タブを開き直すまで「議論グラフを読み込めませんでした」のまま。ライブでも、前に読めていた木を失敗で置き換えて消す。
- 修正案: 失敗のときは `loaded` を進めない（次のポーリングで読み直す）か、前の `Ok` を残して失敗の印だけを添える。

### m2（Minor）: 作り直しの世代と、ポーリングの配線にテストが無い
- 根拠: `graph_consumer.rs:469-471` の `*generation += 1`（または `*rev = 0`）を消しても落ちるテストが無い（`graph_generation` を見るテストは 0 件。`should_load` の単体テストは純粋関数だけを見る）。`GraphOutline` のループが `should_load` を通らず毎回読む変異も、どのテストも落とさない（仕様の変異「`rev` を見ずに毎回読む」は `should_load` を壊した場合だけ捉える）。`evidence()`（並び・重複除去・`target` での絞り込み）にもテストが無い。`outline_rows` の試料には、生きている decision・action が issue の下に付く形が無い（`n5` は merged）。
- 修正案: 作り直しの後に `status().graph_generation` が増え `rev` が 0 に戻るテスト、`evidence()` のテスト、試料に issue の下の decision と action を足す。

### m3（Minor）: 「続きを抽出」が前回と同じ理由ですぐ失敗すると、まだ最大 30 秒押せない
- 根拠: `graph_view.rs:31-42`。前回 `Backend` で止まっていたセッションで押し、`load_backend` がまた `Backend` で返ると、`running` は立たず、理由・`rev` は変わらない。履歴のセッションは実行の後に `prune_idle` で外れるので、consumer の `pending` は押す前も後も 0 で、件数の比較もこの場合には効かない（PR 本文の「未抽出件数の変化を検知」は、実行中か録音中にしか当たらない）。pr21-round2 m2 の根拠に挙げた「前回と同じ理由で再び失敗した場合」が残っている。30 秒で戻るので Minor。
- 修正案: consumer に実行（または起動失敗）の回数を持たせ、押下前と比べる。

### n1（nit）: しきい値は表示を開いたときに 1 回だけ読む
- 根拠: `graph_outline_view.rs:97` の `use_hook(|| consumer.review_threshold())`。設定で変えても、タブを開き直すまで薄さが変わらない。

### n2（nit）: 共有の子は木の中で重複し、`argument` の菱形が重なると行数が指数的に増えうる（コア）
- 根拠: core v0.7.0 `build_outline` は経路の集合だけで循環を断ち、訪問済みを覚えない。描画のたびに（ノードを選ぶたびにも）組み直す。v1 の会議の大きさでは問題にならないので、コアの follow-up に回してよい。

## 判定
converged: no（Blocker 0 / Major 1 / Minor 3 / nit 2）
