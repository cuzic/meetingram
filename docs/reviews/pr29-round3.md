# PR #29 レビュー（round 3）

対象: meetingram feat/graph-review @ 92af0b7430684eaa46c759144ff8fbafb6e83e03（新しく見たのは 94a722b..92af0b7 の 92af0b7）

## 結論（先に）
converged: yes。round 2 の M1・M2 は直っている。整理の完了は、段 2 まで成功したときだけ `mark_consolidation_completed` で記録する。一覧を出す条件は「完了の記録がある、実行中でない、`interrupted_flushes` に入っていない」の 3 つ。仕様で名指しされた 3 つの変異は、どれもテストで落ちる。
新しく見つかったのは Minor 2 件で、どちらも挙動が明らかに誤っているわけではない（再起動をまたいだ場合の扱いと、テストの穴）。Blocker と Major は無い。

## 前ラウンドの解消状況
- M1 解消: 整理の後に再文字起こしの後の抽出を回しても、一覧は出たままで、承認もできる。プローブで確かめた（`available=true`、`set_review = Ok`）。`review_remains_available_after_retranscription_flush` がこの経路を見る。
- M2 解消: 段 2 が失敗したら（`Fake::scripted(2..3, …)`）、一覧は出ず、`set_review` は `Err("review_unavailable")` になる。プローブで確かめた。その後に整理が成功すると、記録は `Some(5)` になり、`interrupted_flushes` から外れて、一覧がまた出る。`unfinished_second_stage_cannot_be_reviewed_even_after_reopening` と `interrupted_second_stage_blocks_review_with_an_older_completion` が、記録の無い場合と、古い記録がある場合を見る。

## 確認したこと
- CI: 92af0b7 で App Build & Test と Windows E2E が success。Windows は 118 passed で、新しいテスト 3 件が `ok`。コアは 15 crate すべてが `v0.11.0` で、`Cargo.lock` に v0.10.0 は残っていない。
- harness: コア v0.11.0 で作り直し、50 passed。変異とプローブはコミットしていない。
- 仕様の 3 つの変異（2 回ずつ回した）:
  - 完了の記録を段 1 の直後に移す → `review_remains_available_after_retranscription_flush`・`unfinished_second_stage_…`・`review_waits_for_the_session_executor` が落ちる。
  - `review_allowed` から `interrupted_flushes` の条件を外す → `interrupted_second_stage_blocks_review_with_an_older_completion` が落ちる。
  - 一覧の条件（`load_outline` と `set_review`）を `is_consolidated` に戻す → `review_remains_available_…` と `unfinished_second_stage_…` が落ちる。
- そのほかの変異: `!status.running` を外す、`set_review` の中で `review_allowed` を見ない、`set_review` の中で完了の記録を見ない、のどれも落ちる。落ちないのは下の m2 だけ。
- 作り直しの後に記録をし直すか: 作り直しの `delete_session` が記録も消し、続く流し切りと整理が成功すると、また記録する（プローブ: 作り直しの後は `Some(..)`、`available=true`）。作り直しで整理が失敗すると記録は `None` のままで、一覧は出ない。
- 失敗の後の整理で一覧が戻るか: 上の M2 のとおり戻る。新しい発話が無いまま「続きを抽出」した場合も、`interrupted` が立っているので整理し直し（呼び出し 2 回）、一覧が出る。
- 記録をするのは段 2 と同じ実行者のロックの中か: `mark_consolidation_completed` を呼ぶのは `consolidate` の最後（`graph_consumer.rs:878-880`）だけ。`consolidate` は `Run::flush` からしか呼ばれない。`flush` は、会議の後の処理のタスク（`:542` の `_execution`）か、録音のタスク（`:445`）が実行者のロックを持ったまま走らせる。間に `set_review` が入ることは無い。段 2 の `apply` が取り消されれば `Err("cancelled")` で抜けるので、記録はしない。
- 生きたノードが 0 で整理を省いたとき: 記録はしないが、`interrupted_flushes` からは外す。一覧は出ないが、出すものも無い。
- 再読み込みの条件: `(rev, 世代, enabled)` をキーにしたので、`running` が下りたときにも読み直す。完了の記録は rev を進めないが、記録は実行の中で書かれ、`running` が下りるのはその後なので、読み直しで必ず拾う。ポーリングと DB の読み込みの間の競合で `enabled` と `review_available` が食い違っても、次の `running` の変化か rev の変化で読み直して直る。食い違ったまま残る経路は見つからなかった。
- ロック: `load_outline` と `set_review` の閉包の中で `status()` を呼ぶので、ロックの順は store → sessions → state になる。state を持ったまま sessions や store を取る経路も、sessions を持ったまま store を取る経路も無いので、デッドロックは起こらない。
- スキーマ: `graph_consolidations` は `CREATE TABLE IF NOT EXISTS` で足すので、既存の DB でも開ける。作り直しで行が消えることはコアのテストにある。

## Findings
### m1（Minor）: 段 2 が中断した後に再起動すると、古い完了の記録で一覧が出る
- 根拠: 整理済み（記録あり）のセッションで「続きを抽出」を回し、段 2 が失敗した。プロセスの中では、`interrupted_flushes` があるので一覧は出ない（`in-process=false`）。ところが consumer を開き直すと、`reopened=true` になり、`set_review = Ok(())` で承認できた。このとき `pending_count = 0` なので「続きを抽出」は押せず、整理し直されることも無い。round 2 の M2 と同じ状況が、再起動をまたぐと残る。コアの `consolidation_completed` の説明（「後の抽出のパッチは記録をそのまま残す」）どおりの挙動で、再文字起こしの後に一覧を残す（M1）ことと引き換えになっている。再起動をまたいだ判定は、当初から owner の範囲としている。
- 修正案: 方針を決めて明記する。たとえば、流し切りや整理を始めるときに記録を外し（または「始めた rev」を持ち）、再文字起こしの後の抽出だけは記録を残す。今のままにするなら、ADR か PR 本文に、この場合の制限を書く。

### m2（Minor）: 流し切りの後に整理を省く判定のうち、完了の記録の側にテストが無い
- 根拠: `flush` の `consolidation_completed(sid)?.is_some() && is_consolidated(sid)?`（`:813-814`）を `is_consolidated` だけに戻しても、50 件すべて通る。この条件が効くのは、consolidate のパッチはあるが記録が無い場合（段 2 の途中で落ちた後の再起動、v0.11.0 より前に整理したセッション）に、新しいパッチ無しで流し切りが終わったとき。画面の「続きを抽出」は未抽出が 1 件以上のときしか押せないので、普通は新しいパッチができて `is_consolidated` も false になる。そのため、画面から届く経路は狭い。記録が無ければ整理し直す、という挙動自体は正しい。
- 修正案: 段 2 を失敗させた DB を新しい consumer で開き、新しいパッチ無しで `Continue` を回したときに整理を呼び（呼び出しの回数で見る）、完了を記録して一覧が出ることを確かめる。

## 判定
converged: yes（Blocker 0 / Major 0 / Minor 2 / nit 0）
