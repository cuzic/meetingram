# PR #29 レビュー（round 2）

対象: meetingram feat/graph-review @ 7c56b6a1ad1ec460323b4dc1d10898e3eef1abfd（新しく見たのは a8baf67..7c56b6a の 7c56b6a）

## 結論（先に）
converged: no。round 1 の M1・m1〜m3・n1・n2 はすべて直っていて、変異もテストで捕まる。ただ、「整理が済んだか」をコア v0.10.0 の `is_consolidated`（保存したパッチから決める）だけで判定するようにしたため、2 つのずれが出た。
1 つ目は後退で、再文字起こしの後の抽出が 1 回走ると、確認の一覧が消えて承認も却下もできなくなる。戻す手段は作り直しだけで、作り直すと承認・却下も消える（M1）。2 つ目は、同じ実行の中で整理の段 2 が失敗しても一覧が出て、承認できてしまうこと（M2）。
rev で書き込みを止める仕組みには、無限ループも、関係の無い rev の変化ですべての押下が弾かれ続けることも無い。`is_consolidated` は UI スレッドの外で、読み直しのときだけ読む。

## 前ラウンドの解消状況
- M1 解消: harness で 3 回ずつ変異を入れた。世代の確認を消すと `review_rejects_a_rebuilt_graph_even_when_revision_and_node_id_match` が落ちる。`recording || rebuilding` を消す、または片方だけにすると `review_rejects_recording_or_rebuilding_sessions` が落ちる。実行者のロックを外すと `review_waits_for_the_session_executor`（待ちを 200 ms に変えた）と `queued_review_rejects_a_changed_revision` が落ちる。両端が生きているかの確認を消すと `review_rejects_edges_with_a_merged_or_rejected_endpoint` が落ちる。どれも 3 回中 3 回落ちた。
- m1 解消: エッジは承認してから `remove_edge` を返させるようになった。コアの `RemoveEdgeReviewed` の確認を消すと `human_reviews_…` が落ちる（`:1677`）。
- m2 解消: 試料に承認済みで 0.5 のノード（`n7`、`n8`）と、2 つの親を持つ `n2`（`e7`）を足した。`dimmed` と `reference_label` を比べている。n1 の待ち時間は `next_retry_delay` に、n2 は `reset_on_generation_change` に出して、どちらもテストしている。
- m3 解消: `set_review` は、一覧を作ったときの `rev`（`graph.rev`）をロックの後に DB の `revision` と比べる（`:283-287`）。この確認を消すと `queued_review_rejects_a_changed_revision` が落ちる。走っている間（`status.running`）は、ボタンも押した時の処理も無効になる。
- n1 解消: 成功した後も `review_stale` の後も、同じ `(世代, rev)` のあいだは `submitted` でボタンを無効にする（`graph_review_view.rs:161-164`）。
- n2 解消: `edge_kind_label` で日本語の名前を出す。

## 確認したこと
- CI: 7c56b6a で linux・windows・macos・e2e-windows の 4 ジョブが pass。Windows は 115 passed で、新しいテスト 7 件が `ok`。コアは 15 crate すべてが `v0.10.0` で、`Cargo.lock` に v0.9.0 は残っていない。
- harness: コア v0.10.0（ローカルの checkout を `[patch]` で指す）で作り直し、47 passed。変異と下のプローブはコミットしていない。
- rev で止める書き込み（観点 2）:
  - ループは無い: `set_review` は 1 回だけ試し、`review_stale` を返す。画面は `submitted` でボタンを止め、次の読み直しで解く。
  - 押下が弾かれ続けることも無い: このプロセスで rev を進める書き込み（`apply`・`set_review`・作り直しの削除）は、どれも同じ閉包の中で `revisions` を更新する。そのため、画面の `status.rev` は 250 ms 以内に DB に追いつき、一覧も読み直す。`status.rev` を読んでから DB を読むまでの間にパッチが入っても、`revisions` が先に進んでいるので、次のポーリングで読み直す。`pending_count` の取り込みはパッチを書かないので、rev を進めない。
  - 再起動の後は `status.rev = 0` で DB は N だが、一覧は `graph.rev`（DB の値）を渡すので弾かれない。弾かれるのは、rev が実際に進んだときの 1 回だけになる。
  - `review_stale` のときに `revisions` を DB に合わせる行（`:285`）を消しても、テストは通る。上の理由で、プロセスの中ではほぼ冗長なので、指摘にはしない。
- `is_consolidated` を読む場所（観点 3）: 読むのは `load_outline`（`history` のときだけ、`db` = `spawn_blocking`）、`set_review` の閉包の中、`flush` の中だけ。どれも UI スレッドの外で、`load_outline` は `(rev, 世代)` が変わったときにしか呼ばれない（`load_changed_graph`）。パッチは必ず rev を進めるので、読み直しのときだけ読めば足りる。押した時の処理で同期的に呼ぶのは `status()` だけ。
- 後退が無いか（観点 4）:
  - `flush` の `interrupted_flushes`: 前の流し切りや整理が途中で終わっていれば、新しいパッチが無くても整理する。新しいパッチが無く、DB が整理済みなら整理を省く。再起動の後でも、新しいパッチがあれば整理する。これは round 1 の `was_consolidated` より正しい。
  - `set_review` の閉包の中で `SessionLease` を落とすときのロックの順は store → sessions で、`apply` と同じ。
  - ログの追加は無い。`review_list_uses_saved_consolidation_after_reopening` は、再起動の後に一覧が出ることを確かめている。

## Findings
### M1（Major）: 再文字起こしの後の抽出が 1 回走ると、確認の一覧が消えて戻らない
- 根拠: 再文字起こしの後の抽出（`PostMeeting::Retranscribed`、`graph_consumer.rs:550`）は、`source = flush` のパッチを保存して、整理はしない（ADR §3）。コアの `is_consolidated` は、最後の live か flush のパッチより後に consolidate のパッチがあるときだけ true を返す。そのため、抽出の後は `load_outline` が false を返し（`:322`）、パネルそのものが消える（`graph_outline_view.rs:261`）。`set_review` も `review_unavailable` になる（`:288`）。
  プローブで確かめた: 整理が済んだセッションに再文字起こしの行を足して `Retranscribed` を回すと、rev が 3 → 4、`consolidated=false`、`pending_count = 0`、`set_review` は `Err("review_unavailable")` になった。
  未抽出が 0 なので「続きを抽出」は押せない（`graph_view.rs:96` の `status.pending > 0`）。残る手段は作り直しだけで、作り直すと承認・却下も消える。ADR §3 は「再文字起こしの後: …承認・却下は消さない。整理はしない」としていて、round 1 の head では一覧はそのまま出ていた。
- 修正案: 再文字起こしの後の抽出のパッチを、「整理済み」の判定から外す。たとえば、コアに再文字起こし用の source を足すか、`is_consolidated` が見るのを「最後の consolidate より後の、再文字起こしでない flush」にする。アプリの側だけで直すなら、再文字起こしの後の抽出の後に整理を 1 回走らせる（ただし ADR を変える）。どの方法でも、「整理済みのセッションで `Retranscribed` を回しても一覧が残り、承認できる」テストを足す。

### M2（Major）: 同じ実行の中で整理の段 2 が失敗しても、一覧が出て承認できる
- 根拠: 段 1 のパッチ（`source = consolidate`）を保存した後に段 2 が失敗すると、`is_consolidated` は true を返す（consolidate の rev が flush の rev 以上になるため）。アプリは `interrupted_flushes` にこのセッションが残っている（整理を終えていない）ことを知っているが、`load_outline`（`:322`）、`set_review`（`:288`）、`review_allowed`（`:247`）のどれもそれを見ない。
  プローブで確かめた: `Fake::scripted(2..3, [TWO_ISSUES, ATTACH_ISSUE])`（流し切りと段 1 は成功し、段 2 の呼び出しが失敗）で「続きを抽出」を回した。結果は `failure_count = 1`、`consolidated=true`、`review_allowed=true`、`set_review(n1, Approve)` は `Ok(())`。
  Issue #28 は「整理が終わる前は出ない」としていて、round 1 では、整理が失敗すると一覧は出なかった。ここで承認したノードやエッジは、次に「続きを抽出」で整理し直しても、統合（`merge`）も付け替えもされない（§2）。段 2 の重複の統合が、人の承認で止まってしまう。段 2 の途中で取り消されたとき（`cancelled`・`disabled`）も同じになる。
- 修正案: プロセスの中では `interrupted_flushes` に入っているセッションを未整理として扱う（`load_outline` と `set_review` で `is_consolidated && !interrupted_flushes.contains(sid)`）。テストは、上の台本で `load_outline(..).1 == false` と `set_review` が `Err` になることを見る。再起動をまたいだ同じ状況（段 1 だけが残った DB）はコアの `is_consolidated` の定義の問題なので、別に owner が決める。

## 判定
converged: no（Blocker 0 / Major 2 / Minor 0 / nit 0）
