# PR #32 レビュー（round 1）

対象: cuzic/meetingram feat/e2e-scripted-stt @ 5744777577b53d36646fdb9f8393e6ee713734e0

## 結論（先に）
converged: **yes**。Blocker と Major はありません。本番に混ざらない仕組みは揃っています。`compile_error!`、環境変数が無いときに何も動かないこと、feature の統合で有効にならないこと、配布 exe の grep を確かめました。`ScriptedSttProvider` は ADR §2 のとおりです。
コア v0.13.0 では、本番と同じ経路の保存を E2E で確かめています。変異（v0.12.0）では、job の check が `failure` になりました。
指摘は、テストの効き目の穴と仕様からの小さなずれ（Minor 5）、および nit 3 です。follow-up に回してかまいません。

## 前ラウンドの解消状況
round 1 のため、前ラウンドの指摘はありません。owner のコメント（停止後の保存の回避策を外し、コアを上げ、E2E に (a)(b)(c) を足す）は、次のとおり解消しています。
- 回避策の削除: 69a173d で足した `recording.rs` の `insert_transcript_segment` の部分と、ps1 の `could not persist E2E scenario` の確認は、9bca4b1 で全部消えています。main との差分で `recording.rs` に残る変更は、`LiveTranscriptionOptions` の組み立てだけです。
- コア: 全 crate が tag `v0.13.0` です（`stt-api` も同じ tag で追加）。

## 確かめたこと
- 手元（Linux）: `cargo fmt --all --check` は pass です。`cargo test --locked -p e2e-scripted -p desktop-cli` は 4 + 5 passed、`cargo clippy --locked -p desktop-cli -p e2e-scripted --all-targets --no-deps -- -D warnings` は pass でした。
  release + feature の `compile_error!` は、Linux からの Windows 向け check が `ring` などのビルドスクリプトで止まるため、実行して確かめられていません。読んだ範囲では、`main.rs:1-2` の `cfg(all(feature, not(debug_assertions)))` は bin の root にあります。`[profile]` に `debug-assertions` の上書きはありません。
- feature の統合: `cargo tree --workspace --target x86_64-pc-windows-msvc -i e2e-scripted` で `e2e-scripted` に依存する crate は無く、`-p desktop`（feature 無し）では依存グラフに入りません。`desktop-cli` も依存していません。
- 環境変数が無いとき: `E2E_SCENARIO` は `Some(None)` になり、`scripted_stt_active()` は false、`recording.rs:125` の分岐も `None` です。上書き・帯・ログは出ず、consumer も止まりません。
- 配布 exe の grep（`desktop-bundle.yml`）: `dx build --release` の出力の `$exe[0]` を読みます。これは `dist` に写すのと同じファイルです。シェルは `pwsh` で、`Latin1` を使えます。目印 `e2e scenario active` は `main.rs` の `tracing::info!` のリテラルなので、feature 付きの exe には入ります。
- CI（head）: Linux・Windows・macOS の `build` が pass です。Windows と macOS では feature 付きの clippy と test も走っています（Windows: 118/4 passed、macOS: 96/4 passed）。`e2e-windows` は pass で、ログには `Final transcript rows: 81`（約 50 秒時点）、停止後 `150`（期待 150）、gap 0 と出ています。
- 変異の run 45089557956（v0.12.0）: run の conclusion は `success`（`continue-on-error`）です。一方 job と commit の check-run `e2e-windows` は `failure` で、PR の checks では赤く見えます。(a) は `Final transcript rows: 0` で失敗しました。(c) はそこまで到達していません。ただしその run の `app.log` には `failed to persist final transcript` が 96 行あり、`Select-String -SimpleMatch 'failed to persist'` は当たります。コア v0.13.0 の文言（`live_transcription.rs:1668` など）とも一致しています。
- 個人データ: head と変異の `app.log`、`status-*.json` に、台本の本文（6 文字以上の発話 150 件）は 1 件も含まれません。DB の確認は `?mode=ro` の URI で数だけを出します。シナリオの読み込みエラーは静的な文字列で、パーサの診断（入力の本文を含みうる）は捨てています。
- 欠落の再文字起こし: 入口は `ui.rs` のボタン（`can_retranscribe`）だけで、`ui.rs:884` で scripted のときは false です。scripted のときは `selected_provider_kind` のキーリング読み取りもしません。scripted でないときの挙動は変わりません。
- `ScriptedSttProvider` の手書き変異（`apps/e2e-scripted/src/lib.rs`）:

| 変異 | 結果 |
| --- | --- |
| 再接続で `next = 0` | `reconnect_resumes…` が落ちる |
| `words: Some(vec![])` | 2 件落ちる |
| `remaining.is_zero()` → `true` | `tracks_share_a_clock…` が落ちる |
| `audio_start_ms` を `/ speed` しない | 2 件落ちる |
| `Drop` で slot を消さない | テストが**終わらない**（m2） |
| `start_session` ごとに時計を作り直す | **通る**（m1） |
| `Drop` で worker を `abort` しない | **通る**（m1） |

## Findings

### m1（Minor）: 「時計が 1 つ」と worker の後始末をテストが固定していない
- 根拠: `lib.rs:210` の `state.started.get_or_insert_with(Instant::now)` を `state.started = Some(Instant::now())` に変えても、テストは 4 件とも通ります。この変異では、再接続のたびに時計が戻ります。順序が入れ替わらないのは worker が 1 つで行を順に出すためで、`tracks_share_a_clock_and_preserve_script_order` は時刻を見ていません。
  `lib.rs:246` の `worker.abort()` を消しても通ります。そのとき worker は `notified()` で止まったまま残り、録音のたびにタスクが漏れます。
  実装は ADR §2 どおりで、挙動は正しいです。
- 修正案: 再接続の後に出る行の受信時刻が、最初の `start_session` からの経過で決まることを `start_paused` で確かめます。両方の session を finalize した後に worker の `JoinHandle` が終わっていることも確かめます。

### m2（Minor）: finalize で送信側を手放さない退行は、テストが止まり続けるだけで失敗にならない
- 根拠: `Drop` の `*slot = None` を消すと、`reconnect_resumes_and_finalize_closes_without_flushing` の `own.recv().await` が終わらなくなります（手元で 120 秒のタイムアウト）。CI では job のタイムアウトまで待つことになります。
- 修正案: `recv()` を `tokio::time::timeout` で包み、`None` が返らなければ失敗させます。

### m3（Minor）: 環境変数による有効化の判定（`main.rs`）をテストが覆っていない
- 根拠: Issue の受け入れ条件は「環境変数が無いときに上書きが渡らないことのテストが落ちる」です。`absent_environment_disables_override`（`tests.rs:14`）は `load_optional(None)` だけを見ています。`main.rs:103` で `var_os` 以外（既定のパスなど）を渡す変異は検出できません。PR 本文の変異（`load_optional` を書き換えたもの）は、これとは別の変異です。現状の挙動は読んだ範囲で正しいです。
- 修正案: 環境変数の値から `Option<Scenario>` を作る関数を `e2e-scripted` 側に移し、`main.rs` はそれを呼ぶだけにします。あわせて、空でない値と未設定の両方をテストします。

### m4（Minor）: 欠落の再文字起こしを止めたことのログが無い
- 根拠: Issue #31 の注意書きは「`e2e-scripted` で動いているときは欠落の再文字起こしをしない（ログに 1 行、本文なし）」です。`ui.rs:884` は黙って `false` にしています。画面には「選択中のSTTプロバイダは再文字起こしに未対応です」と表示され、理由と食い違います。
- 修正案: scenario が有効になったときに `main.rs` で 1 回 `info!` を出します（例: `gap retranscription disabled under e2e scenario`）。

### m5（Minor）: scripted のとき、確定行ごとに `failed to publish SegmentFinalized … no subscribers` の WARN が出る
- 根拠: head の E2E の `app.log` で、WARN 150 件がこの 1 種類でした。summary、rhai、hint、graph の consumer を止めたため、broker に購読者がいません。本物の警告が埋もれます。今後 `failed to` で grep する確認を足すと、これに当たります。
- 修正案: E2E では、ログの確認を件数の決まった文言（今の `failed to persist` のような）に限ることを `windows-e2e-scenario.ps1` に注記します。または、broker の no-subscribers を debug に落とすかどうかを、コアの follow-up に回します。

### n1（nit）: (b) の期待件数を PowerShell の stopwatch で数えている
- `windows-e2e-scenario.ps1:77-80` は、`end_ms // speed <= 100000` で期待件数を数えます。台本の時計は最初の `start_session` から始まるので、ctl の `start` から数える stopwatch より遅れます。今は台本が 90.2 秒で終わり、全件が範囲に入るので、150 との一致は安定しています。100 秒を超える台本では、境界の行のぶん件数が揺れます。コメントで「台本は録音時間内に収まること」を前提として書いておくとよいです。

### n2（nit）: `status-recording.json` を上書きしている
- `windows-e2e-scenario.ps1:111` が、`Connected` を確かめた時点のスナップショット（`:100`）を 50 秒時点の状態で上書きします。成果物から接続時の状態が読めません。`status-recording-mid.json` などに分けるとよいです。

### n3（nit）: 帯以外の UI の変更が、Issue の「触ってよいファイル」の外にある
- 次の 3 つです。`ui.rs` の自動スクロール（`:544`）、要約ボタンの無効化（`:1123`）、`main.rs` のウィンドウ最大化。どれも ADR §1（キーリングの LLM を使わない）と録画の見やすさのための変更で、問題はありません。PR 本文の「変更」にはこの 3 つを列挙しておくとよいです。

## 判定
converged: yes（Blocker 0 / Major 0 / Minor 5 / nit 3）
