# PR #27 レビュー（round 2）

対象: meetingram feat/graph-outline @ 19d9d4bc4e55658be8caae31e60dc351db81e39f（新しく見たのは e4fed40..19d9d4b の 19d9d4b）

## 結論（先に）
converged: yes。round 1 の M1 は直っている。履歴のアウトラインは、セッションを切り替えると作り直され、同じセッションでの再描画では作り直されない。Dioxus 0.7.10 のプローブと、PR に足されたテストの両方で確かめた。
m1〜m3、n1 も直っている。新しく見つかったのは Minor 1 件（テストの穴）と nit 2 件。round 1 の n2（コアの `build_outline`）は範囲外として残る。Blocker と Major は無い。

## 前ラウンドの解消状況
- M1 解消: `history_outline_contents` が `rsx! { Fragment { key: "{session_id}", {outline} } }` を返す（`graph_outline_view.rs:270-273`）。`Fragment` がそのテンプレートの根なので、`key` が `VNode` に載る。`Element` の動的ノードはキー付きの断片として差分を取るので、キーが変わると子を外して作り直す。round 1 のプローブをこの形に変えて回した結果、sid 1 → 2 → 2（再描画）→ 3 で `use_hook` の値は 1 → 2 → 2 → 3、マウントの回数は 3 回（同じキーの再描画では作り直さない）。PR のテスト `switching_history_sessions_remounts_the_outline` も同じ形をそのまま使っており、マウントの記録が `[first, second]` になることを見る。Windows の CI で `ok`。
- m1 解消: 失敗したときは `loaded` を進めないので、次のポーリングで読み直す（`load_changed_graph`、`:88-101`）。前に読めた木は `snapshot` に残し、その上に「議論グラフを読み込めませんでした」を `role: "status"` で添える（`:142-163`）。成功すると印を下ろす。`polls_reload_on_revision_or_generation_changes_and_retry_failed_reads` は、変わらなければ読まない、失敗の後は同じ `(rev, 世代)` でも読み直す、成功したら止まる、世代が変われば読む、を見る。ループ自体がこの関数を通るので、round 1 で指摘した「`rev` を見ずに毎回読む」配線の変異もこのテストで捉えられる形になった（`consumer.load_graph(..)` は `async fn` の future なので、変わっていないときは poll されず DB に触れない）。
- m2 解消: `rebuilding_resets_revision_and_advances_graph_generation`（作り直しの途中で `rev = 0`、世代 +1、読んだグラフも `rev = 0`）、`evidence_is_filtered_sorted_and_deduplicated`（対象での絞り込み、`row_id` 順、重複と存在しない `uid` の除去）を足した。`outline_rows` の試料に、issue の下の生きた decision（`resolves`）と action（`assigns`）が入った。
- m3 解消: `GraphStatus.failure_count` を足し、`load_backend` の失敗と `Run::log_failure` で 1 ずつ増やす。`prune_idle` で外すときも `failures` に理由と回数を残し、`session()` で戻すので、履歴のセッションでも押す前と後で比べられる。`continue_is_pending` は回数が変わったら印を下ろす。`repeated_backend_failure_survives_idle_executor_pruning` は、同じ `Backend` で 2 回失敗したときに回数が 1 → 2 になることを、外された後の `status()` で見る。
- n1 解消: しきい値を signal にし、ポーリングのたびに読み直して、変わったときだけ書く。
- n2 未対応（コアの follow-up。今回の範囲外）。

## 確認したこと
- 範囲: 変更は `graph_consumer.rs`・`graph_outline_view.rs`・`graph_view.rs` だけ。ログの追加は無い（`log_failure` の既存の `warn!` は種類だけで本文を出さない）。
- CI: 19d9d4b で linux・windows・macos・e2e-windows の 4 ジョブが pass。Windows のログで新しい 7 テストが `ok`（104 passed）。手元で `cargo fmt --all --check` pass（`desktop` は Linux でビルドできないので、UI は読んで確かめた）。
- ロック: `log_failure` は `session.state()` を取る。呼び出し元 7 か所（`:473`・`:491`・`:622`・`:713`・`:731`・`:794`・`:867`）は、どれも `state()` のガードを持っていない（`live` の `:627` のガードは呼び出しの後で取る）。std の `Mutex` を二重に取って固まることは無い。
- 再試行: 読み込みは 1 つずつ（`await` してから 250 ms 眠る）で、同時には走らない。失敗が続いても毎秒 4 回まで。ログも出さない（下の n1）。
- m3 が早く下ろしすぎるか: 回数が増えるのは、その会議の実行の中の失敗だけ。履歴のセッションは録音中ではなく、押す側は `running`・`rebuilding` でないことを確かめてから押すので、ほかの実行の失敗を拾って早く下ろすことはない。`:794`（応答の形の誤りを 1 回だけ再試行）と `:867`（`publish` の失敗）は、成功する実行の中でも数えるが、そのときは `running` が立っているので、どちらにしても下ろしてよい。
- m3 が下ろさないままになるか: 回数は単調に増え、外されても残るので、失敗を見逃すことはない。残るのは、何もせずに終わる実行（未抽出が 0 で整理済み）を 250 ms 以内に見逃した場合だけで、ボタンは未抽出が 1 件以上のときにしか押せないため、普通は起こらない。30 秒で戻る。

## Findings
### m1（Minor）: `log_failure` で回数を増やすことにテストが無い
- 根拠: `graph_consumer.rs:903` の `self.session.state().status.failure_count += 1` を消しても、どのテストも落ちない。`failure_count` を見るテストは、`load_backend` の失敗（`:445`）を通る `repeated_backend_failure_survives_idle_executor_pruning` と、純粋関数の `continue_releases_after_a_new_failure_with_the_same_reason` だけ。流し切りや整理が速く失敗した（DB の失敗、`input_limit`）ときに印を下ろす経路は、この行に頼っている。挙動は正しいので Minor。
- 修正案: 流し切りの失敗（`Fake::scripted(0..3, …)` など）で `Continue` を回し、外された後の `status().failure_count` が増えていることを確かめる。

### n1（nit）: 読み込みが失敗し続けると、間をあけずに 250 ms ごとに読み直す
- 根拠: `graph_outline_view.rs:88-101`。`db_open` で失敗するなら、毎回 `GraphStore::open` をやり直す。グラフの接続のロックを、ランナーと毎秒 4 回取り合う。読み込みは 1 つずつなので際限なく増えることはなく、ログも出ない。DB が壊れているならランナーも失敗するので、害は小さい。
- 修正案: 失敗が続くあいだは間隔をのばす（例: 1 秒 → 5 秒）。

### n2（nit）: 作り直しの後の読み込みが失敗すると、消したグラフの木と選択がそのまま残る
- 根拠: `:118-130`。選択を消すのは成功したときだけで、`snapshot` も前のまま。エラーの文は出るので、誤りとまでは言えない。
- 修正案: 世代が変わったのに読めなかったときは、前の木を出さない（または「前の版」と添える）。

## 判定
converged: yes（Blocker 0 / Major 0 / Minor 1 / nit 2）
