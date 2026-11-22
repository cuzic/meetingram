# PR #21 レビュー（round 1）

対象: meetingram feat/graph-ui-settings @ b7b57f666fea956fe62abb9d900adaccf929cbc9（差分は `origin/feat/graph-consumer...HEAD`）

## 結論（先に）
converged: no。録音を止める経路、本文のログ、許可外のファイルの変更は無い。ライブ画面は `status()` だけを読み、`pending_count` は履歴を開いたときと処理の後にだけ呼ぶ。作り直しの確認と二度押し防止、macOS の `cfg` も仕様どおり。
ただし次の 3 点が Major。(1)「続きを抽出」は、バックエンドを用意している数秒のあいだ二度押しできる。(2) 会議中に設定を無効にしても、その会議では送信が続く。画面はそのことを伝えない。(3) ライブ画面では「未抽出 N 件」が出ない。テストは、実際には起きない状態でこの表示を確かめている。

## 前ラウンドの解消状況
なし（round 1）。

## Findings
### M1（Major）:「続きを抽出」は、押してから実行が始まるまでのあいだにもう一度押せる
- 根拠: `graph_consumer.rs:361-458` の `post_meeting_with` は、作り直しのときだけ `rebuilding` をその場で立てる。「続きを抽出」の `running` は、spawn したタスクが `load_backend` を終えて実行権を取った後（`mark_running`）に初めて立つ。`load_backend` は `build_backend` の `spawn_blocking` と、CLI のときは `is_available()` によるプロセスの起動で、Windows の npm 版では数秒かかりうる。そのあいだ、`graph_status_view.rs:115` の `refresh_pending.set(true)` で次のポーリングが `pending_count` を取り直す。その時点ではまだ `running == false` で `pending > 0` なので、`can_continue` が true に戻り、ボタンが押せる。2 回目の押下は `post_meeting_with` のガード（`recording || rebuilding`）を通り、2 つ目の `Continue` タスクができる。`generation` は変わらないので、そのタスクは 1 つ目の後に `flush(false)` をもう一度走らせる。1 つ目が 3 回の失敗で止まっていれば、さらに最大 3 回 LLM に送る。タスク仕様 #20「走っている間は二度押しできない」に反する。純粋関数には「要求済みでまだ始まっていない」状態が無いので、テストもこの経路を捉えない。
- 修正案: UI に「要求済み」の印を持たせる。押下が受け付けられたら立て、`status().running` が true になったのを一度見たとき、または `disabled_reason` が付いたときに下ろす。その間は `GraphView` で両方のボタンを無効にする（`GraphView::new` の引数に足して単体テストを書く）。consumer の `post_meeting_with` で `running` をその場で立てるほうが筋は良いが、それは #15 の変更になる。そちらを選ぶなら Open questions に書く。

### M2（Major）: 会議中に「議論グラフを有効にする」を外して保存しても、その会議では送信が続く。画面は「保存しました」と出すだけ
- 根拠: `graph_consumer.rs:314-347` は、録音の開始時に設定を 1 回だけ複製する。その会議の増分の抽出と会議の後の流し切り（`run.flush(true)`）は、その複製とバックエンドで最後まで走る。ADR §3 は、会議中の抽出について「録音の開始時に読む」と決めている。会議の後については「`graph_enabled` が無効でも、会議の後の流し切りと整理は走らせない（設定どおり何も送らない）」としている。この PR で、録音中も設定画面から `graph_enabled` を切り替えられるようになった（`graph_settings_view.rs:120-131`）。会議中に外すと「議論グラフの設定を保存しました」と出るが、その会議の文字起こしは停止の後まで LLM に送られ続ける。`graph_model` と窓の値も、次の録音まで効かない。画面はどちらも示さない。
- 修正案: 節に「録音中の会議には反映されず、次の録音から効く」と常に書く（録音中に保存したときは、保存の結果の文にも添える）。流し切りの前に `graph_enabled` を読み直すかどうかは consumer 側の判断なので、Open questions に上げる（ADR の文言どおりなら、読み直して止めるのが正しい）。

### M3（Major）: ライブ画面に「未抽出 N 件」が出ない。テストは実際には起きない状態でこれを確かめている
- 根拠: `graph_consumer.rs:345` は、ライブのタスク全体（会議中から流し切りまで）で `mark_running(false)` を持ち続ける。そのため、録音中の `status().running` は常に true になる。`graph_view.rs:30-40` は `live_halted` でなければ `running` を `pending` より優先するので、ライブ画面の表示は会議のあいだずっと「抽出中」になる。追従のタスク仕様は「ライブ画面の『未抽出 N 件』は `status().pending` を使う」としていて、ライブで件数を出すことを前提にしている。一方、`graph_view.rs:61` の `live_view_never_refreshes_pending_count` は、`running == false` の録音中の状態で「未抽出 4 件」を確かめている。この状態はライブでは起きない。抽出を待っているだけのときも「抽出中」と出るので、表示自体も誤解を招く。
- 修正案: `running` のときも件数を付ける（例:「抽出中 · 未抽出 N 件」）。テストは `running: true, pending: 4, recording: true` の組で書き直す。

### m1（Minor）: `pending_count` が一度失敗すると、履歴の行が「未抽出件数を取得できませんでした」だけになり、作り直しのボタンも消えたままになる
- 根拠: `graph_status_view.rs:74-80` と `92-94`。`Some(Err)` は、押下か状態の変化が無いかぎり解消しない。ところが、ボタンが消えているので押下は起きない。グラフ側の `BEGIN IMMEDIATE` が `busy_timeout` を超えるなど、一時的な失敗でも、履歴を開き直すまで操作できない。
- 修正案: 失敗の文は状態の行に出し、ボタンは残す（または数秒おいて取り直す）。

### m2（Minor）: 設定でグラフを無効にすると、履歴の 2 つのボタンは理由を出さずに押せなくなる
- 根拠: `graph_view.rs:41` の `can_rebuild = enabled && …`。表示の文が理由を出すのは、セッションに `disabled_reason` が付いたときだけ。
- 修正案: `!enabled` のときは表示の文を `disabled_message(Disabled)` にする。

### m3（Minor）: 履歴の行が 250 ms ごとに、録音側の `SessionStore`（`capture_state`）を UI スレッドから同期で読む
- 根拠: `graph_status_view.rs:49, 68`。読むのは 1 行の SELECT で、既存のポーリング（`ui.rs:604-611`）にも同じ作りがあるので、録音を遅らせる新しい要因ではない。ただ、この値は開いたとき、処理の後、`state.current` が変わったときに読めば足りる。
- 修正案: `state.current` で録音中かどうかを毎回確かめ、`capture_state` は `pending_count` と同じ時機にだけ読む。

### m4（Minor）: Q6 の理由の文が「グラフを使えない」ことを言っていない
- 根拠: `graph_view.rs:13` の「要約が Codex CLI に設定されています」。タスク仕様の例は「要約が codex CLI なので使えない」。設定画面の「セッションの起動結果 · …」だけでは、何をすればよいかが分からない。
- 修正案:「要約が Codex CLI のため議論グラフは動きません（Claude CLI か API を選んでください）」のように書く。

### n1（nit）: 作り直しの確認の画面が、対象のセッションを示さない
- 根拠: `graph_status_view.rs:126-128`。選択欄はすぐ上にあるが、消える操作なので、確認の文に日時を入れると取り違えにくい。

## 確かめたこと
- CI（head b7b57f6）: Linux / Windows / macOS / e2e-windows が通過。Windows のジョブで `ui::graph_view::tests::*`（6 件）と `ui::graph_settings_view::tests::*`（2 件）が走り、すべて ok。macOS ではこれらのモジュールがコンパイルされない（ログに 0 件。`cfg(windows)` が効いている）。
- 変異（`graph_view.rs` を、型だけスタブにした使い捨ての crate に写して実施。コミットしない）: `can_rebuild` から `!recording` を外す、`!status.rebuilding` を外す、`enabled &&` を外す、`can_continue` から `!status.running` を外す、`should_refresh_pending` から `history &&` を外す、`&& needs_refresh` を外す。6 つとも、どれかのテストが落ちた。
- ライブ画面（`history: false`）は `recording = true` に固定されるので、`pending_count` は呼ばれない。履歴は、マウント時、押下の後、実行が終わって待機に戻った時の 1 回ずつだけ呼ぶ（`pending_count` は `spawn_blocking` なので UI スレッドを止めない）。
- 設定の保存は、既存の節と同じ流れ（`app_settings` のロック → `AppSettings::save`）。失敗したときはメモリ上の値を変えない（テストあり）。
- 新しいファイルにログの出力は無い。変更は `ui.rs` と新しい画面用モジュール 4 つだけで、許可された範囲に収まっている。

## 判定
converged: no（Blocker 0 / Major 3 / Minor 4 / nit 1）
