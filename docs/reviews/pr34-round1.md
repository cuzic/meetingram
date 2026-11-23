# PR #34 レビュー（round 1）

対象: cuzic/meetingram feat/e2e-scripted-llm @ 35ccca036b48ade21db5b7b4b9bdd1a96f6943d5

## 結論（先に）
converged: **yes**。Blocker と Major はありません。
本番の要約とグラフの経路は、工場に移しただけで挙動は変わっていません。Q6・Q7・CLI の `is_available`、会議の後の操作も同じです。巡回と偽の LLM は `cfg(all(windows, feature = "e2e-scripted"))` の中にあり、環境変数があるときだけ動きます。
E2E は録音中の rev、停止後の整理、作り直しでの rev の低下と回復、承認・却下の保存、`failed to persist` 0 件、`SegmentFinalized` の発行失敗 0 件を確かめています。変異の run は job が `failure` になりました。
指摘は、ADR §3 の照合の規則をテストが固定していないこと、作り直しの回復の確認が弱いことなどです（Minor 5、nit 2）。follow-up に回してかまいません。

## 前ラウンドの解消状況（pr32-round1）
- m1: 解消。`start_session` のたびに時計を作り直す変異（`lib.rs:269`）と、`worker.abort()` を消す変異（`:305`）は、どちらも `reconnect_resumes_and_finalize_closes_without_flushing` で落ちます（手元で確認）。
- m2: 解消。`*slot = None` を消す変異は `tests.rs:136` の 1 秒のタイムアウトで落ち、止まり続けません。
- m3: 解消。環境変数の読み取りを `Scenario::from_environment` に移しました。`main.rs` はそれを呼ぶだけです。子プロセスのテストで、未設定と設定の両方を確かめています。
- m4: 解消。起動時に `gap retranscription disabled under e2e scenario` を 1 行出します。画面の文言は「E2E シナリオでは再文字起こしをしません」です。ps1 はこの行がちょうど 1 件あることを確かめます。
- m5: 解消。head の `app.log` の WARN は、リモート登録の 4 件（api-token が無い。既存の挙動）だけです。`failed to publish SegmentFinalized` は 0 件で、ps1 でも確かめています。
- n1: 解消。台本の `end_ms // speed` の最大値に 5 秒を足した値が録音時間内に収まることを確かめます。期待値は台本の行数です。
- n2: 解消。`status-recording.json` と `status-recording-mid.json` に分けました。
- n3: 対応不要（PR 本文に記載あり）。
- 追加の 2 点（owner のコメント）: `graph-settings` の段は、見出しから保存のボタンまでが画面に収まっています（`step-7-graph-settings.png` を確認）。`approved` の段は、同じ `data-node-id` の opacity が 1 になることを DOM で待ちます。

## 確かめたこと
- 手元（Linux）: `cargo fmt --all --check` と `git diff --check` は pass です。`cargo test --locked -p e2e-scripted -p desktop-cli` は 5 + 5 passed です。
- CI（head）: Linux・Windows・macOS の `build` と `e2e-windows` が pass です。Windows では、feature 付きの clippy（`e2e_tour.rs` を含む）と test も走っています（118 passed）。`windows-e2e.yml` から `continue-on-error: true` が消え、E2E の失敗は run の失敗になります。
- 本番の挙動:
  - `ProductionLlm::graph` は、以前の `GraphConsumer::backend` の中身（`spawn_blocking(build_backend)` と CLI の `is_available`）をそのまま移したものです。
  - `Self::enabled`（Windows と `graph_enabled`）は工場の外に残っています。会議の後の操作（`post_meeting` → `backend`）も同じ工場を通ります。
  - `ProductionLlm::summary` は、以前の `summarize()` と `provider_model_string()` を 1 回の `spawn_blocking` にまとめたものです。違いは、キーリングを読む時点が要約の後から前に移ったことだけで、ラベルは実際に使ったモデルと一致する向きに変わります。
  - `graph_settings` は本番では何もしません。`effective_settings` は今の設定のクローンと同じです。
  - `graph_status_view` の `cfg!(windows)` は、モジュール自体が `#[cfg(windows)]` の中にあるので効きません。
- 偽物が混ざらないこと:
  - `e2e_tour` モジュール、`use_tour` の呼び出し、`ScriptedLlm`、レビューの保存ログは、すべて `cfg(all(windows, feature = "e2e-scripted"))` の中です。
  - `use_tour` は、シナリオと `MEETINGRAM_E2E_TOUR_DIR` の両方が無いと何もしません。
  - 通常のビルドに増えたのは `data-node-id` などの属性と `id` だけで、DOM の eval を呼ぶコードは入りません。
  - `compile_error!` はそのままです。シナリオがあるときは Rhai のプラグインを読まないので、要約ボタンの `trigger_manual_summary` がキーリングの LLM に届くことはありません。
- ScriptedSummarizer（ADR §3）:
  - 照合は（`speaker_label`、全文）で行い、`uid` → `row_id` を覚え、行を前から消費します。
  - `add_node` は、根拠の最初の行が `new` にあり、`context` に同じ本文が無いときだけ返します。`grounds` は見えている発話だけです。
  - `add_edge` は、少なくとも一方が今回の `tmp` のときだけ返します。増分でなければ `{"ops":[]}` で、要約は `summary.md` です。
  - ログは種類と件数だけです。
- 手書きの変異（`apps/e2e-scripted`、手元）:

| 変異 | 結果 |
| --- | --- |
| 増分の応答を `{"ops": []}` にする | `incremental_replay_applies_the_gold_graph` が落ちる |
| 両端とも `context` のエッジも返す | 落ちる（却下あり） |
| `context` の同じ本文での重複の回避を外す | 落ちる（エッジ数） |
| 全文ではなく先頭 2 文字で照合する | **通る**（m2） |
| 消費済みの行を除かない | **通る**（m2） |
| `uid` → `row_id` を覚えない | **通る**（m2） |

- E2E の成果物（run 45094325897）:
  - status:
    - 録音中（50 秒時点）は `rev 13`、適用されたライブのリビジョンは 13 です。
    - 停止後は `rev 27`、`consolidated true` です。
    - 作り直しの後は `rev 3`、`consolidated true` です。作り直しの増分は 1 回で、照合 150 件、操作 71 件（= 39 + 32）でした。
    - 2 本目は別の session_id で、`rev 11`、`consolidated true` です。
  - 巡回: 11 段すべてが `result="ok"` で、承認・却下の保存ログは 1 件ずつです。
  - 個人データ: `app.log`、`status-*.json`、job のログに、台本の本文（6 文字以上）、ノードの本文、`summary.md` の行は 1 件も含まれません。
- 変異の run 45092028714（空の増分）: job が `failure` になりました。`Latest applied live graph revision: 0` で `Assert-LiveGraph` が落ち、巡回の段 1 も `.ready` を出していません。

## Findings

### m1（Minor）: `uid` → `row_id` の対応が、セッションごとではなく工場の呼び出しごとに作り直される
- 根拠: `llm_factory.rs:99-104` の `ScriptedLlm::graph` は、呼ばれるたびに `self.0.summarizer()`（`lib.rs:152`）で新しい `Matches` を作ります。
  ADR §3 は「偽の LLM がセッションごとに対応を覚え」と定めています。録音と作り直しでは問題ありません。作り直しは最初から抽出し直すので、作り直すのがむしろ正しい動きです。
  一方、「続きを抽出」と「再文字起こしの後」では、`reference` の発話が空の対応表で照合されます。同じ（話者、本文）の組が台本にあると、先の行を消費してしまい、`new` の発話が 1 行ずれます。
  今の `design` には重複する組が 3 つありますが、どれも正解グラフの根拠ではないため、結果は変わりません。
- 修正案: 対応表をシナリオ側に `SessionId` → `Matches` で持たせます（工場の `graph()` に sid を渡すか、`Backend` を作る所でセッションを渡す）。または、ADR §3 に「バックエンドを作るごと」と書き直します。

### m2（Minor）: ADR §3 の照合の規則（全文、行の消費、`uid` の記憶）をテストが固定していない
- 根拠: 上の表のとおり、先頭 2 文字での照合、消費済みの行を除かない、`uid` を覚えない、の 3 つの変異で、テストは全部通ります。
  正解グラフの再生テストは、重複する組を根拠に持つノードが無いため、これらの規則を通りません。再試行（同じ `new` が 2 回来る）のテストもありません。
- 修正案: 小さな台本で単体テストを足します。「はい」が 2 回あり、2 回目が根拠のもの、同じ `uid` で 2 回呼ぶもの、ノードが `context` にあるときの再試行、の 3 つです。

### m3（Minor）: `control_status` が DB の読み取りの失敗を黙って `consolidated: false` に変える
- 根拠: `graph_consumer.rs:270-275` は、`self.db(...)` の失敗を `.ok()` で捨て、メモリの `rev` と `consolidated: false` を返します。ログもエラーも出ません。
  ctl の `status.graph` が「未整理」と報告され、E2E では `Wait-TourAction 'consolidated'` のタイムアウトとして現れるので、原因が分かりません。
  また、`with_graph` と `control_status` の写し方（最後のセッション、録音中だけメモリの `pending`、`running || rebuilding`）を確かめる単体テストがありません。確かめているのは Windows の E2E だけです。
- 修正案: 失敗の種類（`db_open` などの静的な文字列）を `warn!` で 1 行出します。`graph_consumer` の既存のフィクスチャで、`control_status` の単体テスト（録音中・停止後・作り直しの後）を足します。

### m4（Minor）: 作り直しの「回復」の確認が、空のパッチでも通る
- 根拠: `windows-e2e-scenario.ps1:231-235` は、作り直しの後に `consolidated` と `rev >= 1` だけを見ます。v0.13.0 は空のパッチにもリビジョンを振ります（PR 本文と `:164` の注記）。そのため、作り直しの経路だけ空の応答になる退行でも、この 2 つは通ります。
  今これを防いでいるのは、巡回の `RebuildComplete` の `reveal(NODES)`（ノードが 1 つ以上）だけです。録音中には `Get-LiveAppliedRevision` の確認がありますが、作り直しには同じものがありません。
- 修正案: 作り直しの後、DB でそのセッションのノード数（作り直しでレビューが消えるので 39）を数えます。または、`source` が flush で操作が空でないパッチがあることを確かめます。

### m5（Minor）: 巡回の段 1 の 30 秒の待ちが、アプリの表示から ctl `start` までの時間を含んでいる
- 根拠: `e2e_tour.rs:249-253` の `wait_status(STEP_TIMEOUT, recording && elapsed_ms >= 13_000 && rev > 0)` は、`App` が描画されて巡回が始まった時点から 30 秒を数えます。
  この 30 秒には、ps1 がウィンドウを前に出し、2 秒待ち、スクリーンショットを撮って `start` するまでの時間と、録音後の 13 秒が入ります。
  head の run では、ウィンドウが前に出たのが 01:28:26.5、録音の開始が 01:28:28.7 で、条件を満たしたのは約 01:28:41.7 です。30 秒のうち 15 秒以上を使っています。
  ランナーが遅く、ウィンドウの表示から `start` までが 17 秒を超えると、段 1 が `timed_out` で落ちます。
- 修正案: 段 1 は、まず `status.recording` を長めの上限（例: 120 秒）で待ちます。そのあとで、`elapsed_ms` と `rev` を 30 秒で待ちます。

### n1（nit）: 増分でない `complete` を、すべて `kind="consolidation"` とログに出す
- `scripted_llm.rs:79`。今は整理しか来ませんが、増分以外のすべてを含むので `kind="non_incremental"` などのほうが正確です。

### n2（nit）: 成果物の画面には、読み手が取り違えやすい点が 2 つある
- `step-7-graph-settings.png` の確認のしきい値は、保存済みの値（0.6）です。実際に効いているのは、シナリオの上書き（0.9）です。ADR §4 どおり `AppSettings` に保存していないためで、挙動は正しいです。
- 履歴の画面には「状態: failed (recoverable)」が出ます。api-token が無いため、リモート登録が失敗しているもので、PR #32 からの既存の挙動です。
- PR 本文の成果物の説明に、この 2 点を一言ずつ書いておくとよいです。

## 判定
converged: yes（Blocker 0 / Major 0 / Minor 5 / nit 2）
