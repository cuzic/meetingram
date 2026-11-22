# PR #29 レビュー（round 1）

対象: meetingram feat/graph-review @ ff00c16dc2191e238176a448d6bcc4641cbdfe0d

## 結論（先に）
converged: no。実装は読んだ範囲で正しい。`set_review` はセッションの実行者のロックを取ってから準備の状態と世代を確かめ、書き込みが終わるまでロックを持つ。却下はアウトラインと一覧の両方から外れ、承認すると薄い表示が戻る。共有された子の参照も正しく出る。
ただ、`set_review` を守る条件のテストが効いていない（M1）。世代の確認を消しても、`review_ready` から作り直し中と録音中の条件を消しても、42 件のテストはすべて通る。実行者のロックを外す変異も、6 回中 2 回は通った。
ほかに Minor 3 件と nit 2 件がある。Blocker は無い。

## 前ラウンドの解消状況
- round 1 なので無い。PR #27 round 2 から持ち越した項目は「確認したこと」に書いた。

## 確認したこと
- 範囲: 変更は `Cargo.toml`・`Cargo.lock`・`graph_consumer.rs`・`graph_outline_view.rs`・新しい `graph_review_view.rs`・`ui.rs` だけで、どれも触ってよいファイルに入っている。コアは 15 crate すべてを `tag = "v0.9.0"` で参照し、`Cargo.lock` に v0.7.0 は残っていない。`rhai` はアプリもコアも `=1.27.0` でそろっている。
- CI: ff00c16 で linux・windows・macos・e2e-windows の 4 ジョブが pass。Windows のログでは、新しいテスト（`human_reviews_are_saved_and_protected_during_consolidation`・`flush_and_consolidation_failures_increment_failure_count`・`review_waits_for_the_session_executor`・`candidates_require_live_automatic_items_strictly_below_threshold`）が `ok` で、合計 108 passed。
- 手元での実行: `desktop` は Linux でビルドできない。そこで `app_settings.rs` と `graph_consumer.rs` を `#[path]` で読み込む一時的なクレートを作り、コア v0.9.0 に対して回した（42 passed）。変異はこのクレートで入れ、コミットはしていない。
- ログ: 新しい `tracing` の呼び出しは無い。`set_review` と画面のエラーは `&'static str` か固定の文で、本文を含まない。
- 同時実行（観点 1）: すべての書き込みは `GraphConsumer.store` の `Mutex` を通るので、接続が同時に使われることは無い。`set_review` はさらに `session.execution` を `lock_owned` で取り、そのガードを `spawn_blocking` の閉包の中まで持っていく（`graph_consumer.rs:265`、`:277-278`）。このため、書き込みは実行中の流し切り、整理、再文字起こし、作り直しのどれとも重ならない。画面の future が捨てられても、取ったロックは書き込みが終わるまで放さない。ロックの取り方は store → sessions の順で、既存の `apply` と同じなので、デッドロックは起こらない。
- 作り直しとの競合（観点 2）: 作り直しは `rebuilding = true` を同期的に立ててから実行者のロックを待つ。そのため、ロックを待っていた `set_review` も、作り直しが終わった後の `set_review` も、`:267` の確認（`!review_ready` または世代の不一致）で `review_unavailable` になる。対象が `merged` のノードや判断済みのものなら、コアが `UnreviewableTarget` を返す。エッジの場合は、アプリが両端が生きていることも確かめる。
- 一覧の対象（観点 3）: `candidates` が生きていて `review == Auto` のノードと、両端が生きていて `Auto` のエッジを選ぶ。`visible_candidates` が `ready` と `confidence < threshold`（未満）で絞る。`review_ready` は「整理の完了の記録がある、録音中でない、作り直し中でない」の 3 条件。「続きを抽出」は流し切りの最初に完了の記録を外し（`flush`）、失敗したら記録を戻さない。整理が終わる前に一覧が出ないことは、`human_reviews_…` の冒頭と `flush_and_consolidation_failures_…` で確かめられている。比較を逆にする変異と、`review` の条件を外す変異は、`candidates_require_…` で落ちる（読んで確かめた。`equal` と `high`、`approved` の行が出てくるため）。
- 却下と承認（観点 4）: 却下したノードは `is_live()` が false になり、コアの `build_outline` は生きているノードと `live_edges`（却下したエッジを除く）だけを使う。どちらも消されずに表示から外れる。承認したノードは、`dimmed` が `review == Auto` を見るので濃く出る（`graph_outline_view.rs:34`、`:52`）。書くと `revisions` の `rev` が進み、次のポーリングで一覧とアウトラインを読み直す。
- 整理からの保護（観点 5）: コアで `set_status` の保護（`StatusReviewedNode`）を外すと、`human_reviews_…` は落ちる。エッジの側は下の m1。
- 参照（観点 6）: `is_reference` の行には「↑ 上に出たもの · 」が付く。選んだときに使う `nid` は元のノードと同じなので、出る根拠も同じになる（`selected_node` は `graph.nodes` から引く）。参照は子を持たない（コアの仕様）。
- PR #27 からの持ち越し（観点 7）: m1 は `log_failure` の `failure_count += 1` を消すと `flush_and_consolidation_failures_increment_failure_count` が落ちるので、解消。n1 は失敗のたびに待ち時間が 1 → 2 → 4 → 5 秒とのび、5 秒で止まる。世代が変わるか読み込みに成功すると 0 に戻る。n2 は世代が変わったら、読み込む前に `snapshot` と選択を消す（`:127-133`）。n1 と n2 は挙動としては正しいが、テストが無い（m2）。
- UI スレッドを止めないか（観点 8）: UI スレッドから同期的に呼ぶのは `status()` と `review_threshold()` だけで、どちらも std の `Mutex` を短い間だけ取る。`sessions` のロックを持ったまま DB に触る経路は無い。書き込みは `tokio::spawn` から `spawn_blocking` で行う。

## Findings
### M1（Major）: `set_review` を守る条件のテストが効いていない（世代、作り直し中・録音中、実行者のロック）
- 根拠: 一時的なクレートで次の変異を入れて、42 件のテストを回した。
  - `graph_consumer.rs:267` の `|| generation != status.graph_generation` を消す → 42 件すべて通る。
  - `:247-252` の `review_ready` から `!recording` と `!status.rebuilding` を消す → 42 件すべて通る。
  - `:265` のロックを取る行を `let execution = ();` に変える → 全体を回すと 6 回中 2 回通る。`review_waits_for_the_session_executor` は `timeout(Duration::ZERO, …)` が `Err` になることを見るが、`set_review` の中身は `spawn_blocking` なので、ロックを待たなくても最初の poll では普通 `Pending` になる。落ちるかどうかは、ブロッキング用のスレッドが先に終わるかの競争で決まる（`:1640`）。
  - `:284-294` のエッジの両端が生きているかの確認を消す → 42 件すべて通る。
  世代の確認が無いと、次のことが起こる。作り直しの後は `rev` も `nid` も既存の最大値 + 1 から振り直す（グラフが空なので `n1` から）ので、作り直しの前に表示していた `n5` と、作り直した後の別の `n5` が同じ ID になりうる。古い一覧から押した承認が、見ていない別のノードに付き、その後の整理でも変えられなくなる。Issue #28 の「同じ接続を同時に使わない」もこれらの条件に頼っているが、どれも外して CI が緑のまま通る。
- 修正案: consumer 経由のテストを足す。(a) 世代 0 で一覧を作ってから作り直しを終え、古い世代で `set_review` を呼ぶと `Err` になり、新しいグラフの同じ `nid` が `Auto` のまま残る。(b) 作り直しを要求した直後（`rebuilding` が立った状態）の `set_review` が `Err` になる。(c) 実行者のロックを持ったまま、十分な時間（例: 200 ms。`spawn_blocking` が終わる長さ）待っても `set_review` が終わらず、`rev` が変わらない。`timeout(ZERO)` ではなく、時間を取って確かめる。

### m1（Minor）: 整理からの保護を確かめるテストのうち、エッジの側が保護を通っていない
- 根拠: `human_reviews_are_saved_and_protected_during_consolidation`（`graph_consumer.rs:1560`）は、エッジを**却下**してから（`:1579`）、整理で `remove_edge` を返させている（`:1611`）。却下したエッジは `live_edges()` に入らないので、コアの `remove_edge` は保護を見る前に `RemoveEdgeMissing` で落とす（コア `patch.rs:417-421`）。コアの `RemoveEdgeReviewed` の確認（`patch.rs:422-424`）を消しても、このテストは通った。承認したエッジが整理で外されないことは、consumer 経由では確かめられていない（コアの単体テストにはある）。挙動は正しいので Minor。
- 修正案: エッジは**承認**してから `remove_edge` を返させ、承認したエッジが残ることを確かめる。却下したエッジは、別の操作（`add_edge` で同じ組を足し直すなど）で確かめる。

### m2（Minor）: 承認で濃くなること、参照の印、n1 の待ち時間、n2 の消去にテストが無い
- 根拠: `outline_lists_subquestions_first_and_unattached_last` の試料に、承認済みで確信度の低いノードが無い。そのため `:34` と `:52` の `review == Review::Auto &&` を消しても落ちない（試料のノードは、却下した `n6` を除いてすべて `Auto`）。同じテストは `is_reference` を `..` で読み飛ばしていて（`:461`）、共有された子も試料に無い。n1 の待ち時間（`:148-155`）と n2 の消去（`:127-133`）は `use_future` の中に書かれていて、テストが無い。`*2` を消す、上限を外す、`snapshot.set(None)` を消す、のどれも通る。Issue #28 の「承認したものは…薄く表示しない」と、PR #27 の n1・n2 は挙動としては正しい。
- 修正案: 試料に承認済みで 0.5 のノードと、2 つの親を持つノードを足し、`dimmed` と `is_reference` を比べる。待ち時間の計算と、世代が変わったときに消すかの判断を純粋関数に出してテストする。

### m3（Minor）: 書き込みが待たされている間に中身が変わったものでも、承認できてしまう
- 根拠: 再文字起こしの後の抽出（`PostMeeting::Retranscribed`）は、整理の完了の記録を外さない。このため、走っている間（LLM の呼び出しの間、実行者のロックを持つ）も `review_ready` は true のままで、一覧のボタンも押せる。押された `set_review` はロックを待ち、抽出が終わると確認を通って書く。その抽出が `merge` を含んでいると、コアの `redirect_edges` は `eid` を変えずに端点を付け替える。その結果、一覧で「A → B」として見て承認したエッジが、実際には「A' → B」に承認として付く。この承認は、その後の整理でも変えられない。「続きを抽出」でも、流し切りがロックを取ってから次のポーリング（250 ms 以内）までは同じことが起こる。merged のノードや判断済みのものはコアが弾くので、起こるのは端点の付け替えと状態の変更だけで、窓も狭い。
- 修正案: 一覧を作ったときの `rev` を `set_review` に渡し、ロックを取った後の `rev` と違えば `review_unavailable` にする。自分が書いた分は `revisions` の更新で吸収する。あるいは、`status.running` の間は一覧のボタンを無効にする。

### n1（nit）: 保存の直後、次のポーリングまでは同じ行をもう一度押せて、失敗の文が出る
- 根拠: `graph_review_view.rs:131-132` は、書き込みが終わると `saving` を下ろす。一覧を読み直すのは最大 250 ms 後なので、その間に同じ行の承認や却下を押すと `UnreviewableTarget` で「確認結果を保存できませんでした」が出る。最初の保存は成功している。
- 修正案: 成功した `id` を、次に一覧を作り直すまで隠すか、ボタンを無効にしておく。

### n2（nit）: エッジの行で、種類が英語の内部名のまま出る
- 根拠: `graph_review_view.rs:89` が `edge.kind.as_str()`（`sub_issue_of` など）を出す。ノードの行は `kind_label` で日本語にしている。
- 修正案: エッジの種類にも日本語の表示名を付ける（アウトラインの「＋賛成」「−反対」と合わせる）。

## 判定
converged: no（Blocker 0 / Major 1 / Minor 3 / nit 2）
