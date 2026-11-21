# PR #15 レビュー（round 1）

対象: cuzic/meetingram feat/graph-consumer @ a8775e38aff6cf3eca7c63708f1783acd5331cf4

## 結論（先に）
converged: no（Major 1）。録音の経路を止める・遅らせるものは見つからなかった。理由: DB の処理はすべて `spawn_blocking`、録音側の `SessionStore` には触れない、`start_recording` / `recording_stopped` はロックを一瞬取るだけ、会議の後の処理は capture タスクの終了後に出る `session.{sid}.stopped` でだけ始まる。実行者は 1 つ、3 回の失敗で止まる、流し切りは有限回で終わり、失敗したら整理しない、起動の条件は全部の入口に当たる、Q7、rev、`NoSubscribers` も ADR どおり。
ただし、タスク 5 の受け入れ条件「停止後の発話が会議の後に抽出される」のテストが、抽出されたことを確かめていない（M1）。ほかに、テストの無い ADR の挙動がいくつかある（m1〜m3）。

## 前ラウンドの解消状況
round 1 のため無し。

## 確認したこと
- CI: 3 ジョブ pass。Windows（`build (windows-latest…)`）と macOS（`build (macos-latest…)`）のログで `graph_consumer::tests` の 11 件が全部 `ok` になっている。Linux ジョブは desktop-cli だけで、グラフのテストは走らない。
- Linux での実行: `app_settings.rs` と `graph_consumer.rs` を `#[path]` で読む一時 crate をワークスペースに足し（コミットしない）、17 件全部 pass。下の変異とプローブもこの crate で走らせた。
- 録音の経路: `recording::start` では、`start_recording` が capture の `tokio::spawn` の直前に呼ばれる。中身は設定のクローン、`try_lock_owned`、broker の購読 2 つ、spawn だけで、待たない。`publish_session_stopped` は、`stop` の中で capture タスクの `join_handle` を待った後に呼ばれる（音声の保存が終わった後）。`GraphStore` の std の `Mutex` を取るのは `spawn_blocking` の中だけ。await をまたいで持つのは `tokio::sync::Mutex`（実行者のロック）だけ。
- 変異で落ちるテストを確かめた: `SessionEnd` の subject で流し切りを始める（3 件落ちる）、post_meeting の実行者のロックを外す、会議中の 3 回で止める処理を外す、失敗で止めた流し切りでも整理する、再文字起こしで参照用の発話を付ける、rev を `patch_rev()` から取る、`graph_enabled` を見ない。
- プローブ: 3 回失敗で止めた後の行は、会議の後の流し切りで抽出される（pending 0）。応答が読めない場合は 1 回だけ再試行し、常に読めなければ 6 回の呼び出し（3 回分の失敗）で止まる。録音中は Continue・Rebuild・Retranscribed のどれも受け付けない。
- ログ: `debug!("graph patch saved")` と `warn!("graph operation failed")` だけで、出すのは `session_id`、rev、source、件数、所要時間、`kind`。本文・プロンプト・応答は出さない（テストでも確かめている）。

## Findings
### M1（Major）: 受け入れ条件「停止後の発話が会議の後に抽出される」を確かめるテストが無い
- 根拠: これに当たるのは `three_live_failures_stop_calls_but_keep_importing`（`graph_consumer.rs:1017`）だけ。ダミーは常に失敗するので、停止の後に入った行（`synthetic turn after halt`）が会議の後に抽出されたかは確かめていない。最後の `pending == 4` は、流し切りも失敗したことを確かめているだけ。`only_stopped_flushes_partial_window…` は会議中に失敗していない場合だけを見ている。挙動は正しい（最初の 3 回だけ失敗するダミーのプローブで、止めた後の行が流し切りのプロンプトに入り、pending 0 になった）。Issue #14 は受け入れ条件の各項目にテストを求めている。
- 修正案: 最初の 3 回だけ失敗し、その後は成功するダミーで、止めた後の行が流し切りのプロンプトに入り、`pending_count` が 0 になることを確かめるテストを足す。

### m1（Minor）: 録音中の作り直し・続きを拒む条件のテストが無い
- 根拠: `post_meeting_with` の `state.recording ||`（`graph_consumer.rs:305`）を外しても全テストが通る。外すと、録音中の作り直しが generation を進め、会議中の実行者の `live()` が `cancel.changed()` で false を返して終わる。その結果、その会議の残りの増分抽出も、停止後の流し切りも走らなくなる。ADR §3「録音中のセッションには出さない」。今の実装は正しい（プローブで 3 つとも拒まれた）。
- 修正案: `start_recording_with` の後に `post_meeting_with` の 3 つの操作が `None` を返し、`recording_stopped` の後なら受け付けるテストを足す。

### m2（Minor）: 応答が読めないときの 1 回だけの再試行と、整理の 120 秒にテストが無い
- 根拠: 再試行を消して最初の解析エラーで `Err` を返す変異でも、整理の上限を 60 秒にする変異でも、全テストが通る。テストのダミーは常に `{"ops":[]}` かプロバイダのエラーを返すので、解析エラーの経路を通らない。整理の呼び出しの `timeout` はどのテストも見ていない。挙動は正しい（プローブで、読めない応答 1 回の後に成功すると呼び出し 2 回で抽出された）。
- 修正案: 1 回目に読めない応答を返すダミーで、2 回目のプロンプトに解析エラーの種類が付き、成功することを確かめる。整理の呼び出しの `timeout == 120s` も確かめる。

### m3（Minor）: 抽出するものが無くても整理を呼ぶ
- 根拠: `flush`（`graph_consumer.rs:571`）は、未抽出が 0 件でもノードが 0 件でも `consolidate` を呼ぶ。プローブでは、発話の無い会議を止めると LLM が 1 回呼ばれた（空のグラフの整理）。Windows で STT のキーが無く要約のキーだけある環境では、会議ごとに 1 回、無駄な呼び出しと空のパッチ（rev が進む）ができる。本文は送らないので漏えいは無い。「続きを抽出」を未抽出 0 件で押した場合も、整理がもう一度走る。
- 修正案: 生きているノードが 0 件なら整理を飛ばす（`build_consolidate` の前にグラフを見る）。

### n1（nit）: `RunGuard::drop` の `lock().unwrap()`
- 根拠: `graph_consumer.rs:79`。セッションの状態の `Mutex` が poison されていると、巻き戻しの中の drop で 2 回目の panic が起き、プロセスごと abort する（録音も止まる）。今の実装では、ロックの中で panic しうるコードは無いので起きない。
- 修正案: `lock().unwrap_or_else(PoisonError::into_inner)` にする。

### n2（nit）: `sessions` の `HashMap` が削除されない
- 根拠: `status()` を含め、どの呼び出しもエントリを作り、消さない（`graph_consumer.rs:96`）。1 セッションにつき小さな構造体が 1 つなので、実害は無い。

## 判定
converged: no（Blocker 0 / Major 1 / Minor 3 / nit 2）
