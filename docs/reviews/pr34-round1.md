# PR #34 レビュー（round 1）

対象: cuzic/meetingram feat/e2e-scripted-llm @ 35ccca036b48ade21db5b7b4b9bdd1a96f6943d5

## 結論（先に）
converged: **yes**。Blocker と Major はありません。
本番の要約とグラフの経路は、工場に移しただけで挙動が変わっていません。Q6、Q7、CLI の `is_available`、会議後の操作も同じです。
巡回と偽の LLM は `cfg(all(windows, feature = "e2e-scripted"))` の中にあり、環境変数があるときだけ動きます。
E2E の確認は、主張どおりの事実を見ています。空の ops の変異では job が `failure` になりました。
指摘は、ADR §3 の照合の規則にテストが効いていないこと、`status.graph` が DB エラーを黙って既定値に置き換えること、作り直しの回復の確認が弱いことなどです（Minor 4、nit 2）。follow-up に回してかまいません。

## 前ラウンドの解消状況
対象は PR #32 round 1（`docs/reviews/pr32-round1.md`）の指摘です。手書きの変異で確かめました。
- m1: 解消。`lib.rs:269` で時計を `start_session` のたびに作り直す変異と、`lib.rs:305` の `worker.abort()` を消す変異で、どちらも `reconnect_resumes_and_finalize_closes_without_flushing` が落ちます。
- m2: 解消。`lib.rs:301` の `*slot = None` を消すと、テストは 1 秒の上限で落ちます（止まりません）。
- m3: 解消。判定を `Scenario::from_environment()` に移し、`main.rs` はそれを呼ぶだけです。テストは子プロセスで、環境変数が未設定のときと設定したときの両方を見ています。
- m4: 解消。起動時に `gap retranscription disabled under e2e scenario` を 1 行出します。画面の文言は Issue のとおりで、ps1 が 1 件であることを確かめています。
- m5: 解消。head の `app.log` に `failed to publish SegmentFinalized` は 0 件です。WARN は remote registration の 2 種類（各 2 件）だけで、ps1 が 0 件を確かめています。
- n1: 解消。期待件数を台本の長さと速度から計算し、停止の 5 秒前までに流し終わることも確かめています。
- n2: 解消。`status-recording.json` と `status-recording-mid.json` に分けて保存しています。
- n3: 対象外（文書の指摘）。
- 巡回の follow-up: `graph-settings` は、節の全体が画面に収まってから撮っています（`step-7` で見出しから保存ボタンまで写っていることを確認）。`approved` は DOM で opacity が 1 になることを確かめています。

## 確かめたこと
- 手元（Linux）: `cargo test --locked -p e2e-scripted -p desktop-cli` の結果は 5 + 5 passed です（`incremental_replay_applies_the_gold_graph` を含む）。
- CI（head）: Linux、Windows、macOS の `build` と `e2e-windows` は、どれも pass です。workflow から `continue-on-error: true` を外したので、失敗は run の conclusion にも出ます。
- 変異の run 45092028714（空の ops）: conclusion は `failure` です。`Latest applied live graph revision: 0` と `Scripted LLM did not match any new utterances` で落ち、巡回は `step-1` の `.ready` が無いことで落ちました。手元でも、`increment` が `{"ops":[]}` を返す変異で単体テストが落ちます。
- 本番の挙動:
  - `ProductionLlm::graph` は、元の `GraphConsumer::backend` の本体（`build_backend` → CLI の `is_available`）をそのまま写したものです。`Self::enabled` も今までどおり先に呼ばれます。
  - `ProductionLlm::summary` は元の `summarize` と同じ読み取りをしています。`provider_model` を要約の後ではなく前に読むようになりましたが、使ったモデルを記録することになるので、退行ではありません。
  - `graph_settings` は既定では何もしません。そのため `effective_settings` は `AppSettings` の複製と同じです。
  - `graph_configured` に `cfg!(windows)` を足しましたが、呼び出し元の `graph_status_view` は Windows でしかコンパイルされません。
  - `actions.rs` の `spawn_auto_summary` を外に出した変更は、本番では今までと同じです。
- 本番に混ざらないこと:
  - `e2e_tour` mod と `use_tour` の呼び出し、`ScriptedLlm`、review の保存ログは、すべて `cfg(all(windows, feature = "e2e-scripted"))` の中です。
  - `use_tour` は、`E2E_SCENARIO` と `MEETINGRAM_E2E_TOUR_DIR` の両方があるときだけ動きます。
  - 通常のビルドに入るのは `id` と `data-node-id` / `data-node-kind` の属性だけです。DOM を評価する新しい口は足していません（`document::eval` は Dioxus に元からあるものです）。
  - `compile_error!` も残っています。
  - scripted のときは Rhai のプラグインを読み込まないので、要約ボタンの `trigger_manual_summary` がキーリングの LLM に届くことはありません。
- `ScriptedSummarizer`（ADR §3）:
  - 照合のキーは（`speaker_label`、全文）です。`uid` から `row_id` への対応を覚え、台本の行を前から順に消費します。
  - `context` にある本文のノードは返しません。エッジは、少なくとも一方の端点が今回の `add_node` のときだけ返します。
  - 整理には `{"ops":[]}` を、要約には `summary.md` を返します。
  - ログに出すのは種類と件数だけです。
  - CI の `app.log` には、台本の発話（6 文字以上の 150 件）が 1 件も含まれていません。
- E2E の成果物（run 45094325897）:
  - 録音中（50 秒）の `rev` は 13 です。停止後は `rev` 27 で consolidated、作り直しの後は `rev` 3 で consolidated に戻りました。2 本目は別の sid で `rev` 11、consolidated です。
  - 作り直しの増分は `matched=150 operations=71` で、39 ノード + 32 エッジです。
  - review の保存ログは、承認・却下とも 1 件ずつです。
  - 巡回の 11 段は、すべて `result="ok"` です。

| 変異（`scripted_llm.rs`） | `incremental_replay_applies_the_gold_graph` |
| --- | --- |
| 増分に `{"ops":[]}` を返す | 落ちる |
| `context` の本文で重複を除かない | 落ちる（エッジ数） |
| 両端とも `context` のエッジも返す | 落ちる（却下） |
| 行の消費（`consumed`）をしない | **通る**（m1） |
| 本文の先頭 2 文字で照合する | **通る**（m1） |
| `uid` → `row_id` の対応を覚えない | **通る**（m1） |

## Findings

### m1（Minor）: ADR §3 の照合の規則（全文で照合、行の消費、`uid` の記憶）がテストで固定されていない
- 根拠: 上の表のとおり、3 つの変異でテストがすべて通ります。`design` の台本で（話者、本文）が重なる組は 3 組 6 行あります。しかし、どのノードの根拠にもなっていません。再試行（同じ `new` が 2 回来る場合）のテストもありません。
- 修正案: 合成の小さな台本で、同じ（話者、本文）を 2 行置き、2 つ目の行を根拠にするノードを用意します。そのうえで、(a) 2 回目の `new` でそのノードが出ること、(b) 同じ `new` を再試行しても同じ行に対応すること、(c) 先頭が同じで本文の違う発話を取り違えないこと、の 3 つを確かめます。

### m2（Minor）: 照合の状態が「セッションごと」ではなく、`Backend` を作るたびに初めからになる
- 根拠: `lib.rs:152` の `summarizer()` は、呼ぶたびに新しい `matches` を作ります（`scripted_llm.rs:63`）。録音中の 1 本の run では問題ありません。ただし「続きを抽出」や再文字起こしの後の増分では、新しい偽物が `reference` の発話から行を消費します。そのため、窓の外に同じ（話者、本文）の行があると、`new` の発話を前の行に対応させます。そうなると、その行を根拠に持つノードは作られません。今の台本では、根拠に重なりがないので起きません。
- 修正案: 対応表を `ScriptedLlm` 側で sid ごとに持つか、`row_id` を `uid` から決まる形にします。そうでなければ、ADR との違いをコメントに残します。

### m3（Minor）: `status.graph` が、DB の読み取りエラーを黙って既定値に置き換える
- 根拠: `graph_consumer.rs:271` の `.ok()` と `:275` の `unwrap_or((status.rev, status.pending, false))` です。`GraphStore` を開けないときや、ロックで失敗したときに、ctl は `consolidated: false` とメモリ上の `rev` を正しい値のように返し、ログにも何も出ません。E2E でも本番でも、「整理が終わっていない」と区別できません。
- 修正案: 失敗したときは `tracing::warn!`（種類だけ）を出し、`graph` を `None` にします。

### m4（Minor）: 作り直しの「回復」の確認が、件数を見ていない
- 根拠: ps1 の `:233` は、`consolidated` と `rev >= 1` だけを確かめます。v0.13.0 は空のパッチにもリビジョンを振るので、`rev >= 1` は ops が 0 件でも成り立ちます。巡回の `RebuildComplete`（`e2e_tour.rs:342`）も、見えるノードが 1 つ以上あることしか見ていません。作り直しでノードの一部が落ちる退行は、検出できません。
- 修正案: 録音中の確認と同じように DB を `?mode=ro` で読み、作り直しの後のノード 39・エッジ 32、または ops のある patch が 1 件以上あることを確かめます（件数だけを出します）。

### n1（nit）: 承認の段が「先頭の項目」ではなく「アウトラインで薄い最初のノード」を選ぶ
- `e2e_tour.rs:185-204` は、Issue の「先頭の項目を承認、次を却下」と少し違います。DOM で確かめるための変更なので妥当です。PR 本文にそう書いておくとよいです。

### n2（nit）: 設定画面は、シナリオの上書き（窓 6 / 45、しきい値 0.9）ではなく、保存済みの値（しきい値 0.6）を表示している
- `step-7` では、確認のしきい値が 0.6 と表示されています。上書きはメモリ上だけなので仕様どおりです。ただ、録画を見た人が、確認の対象が 0.9 で決まっていることに気づけません。帯に「シナリオの設定を適用中」と添えることを検討してください。

## 判定
converged: yes（Blocker 0 / Major 0 / Minor 4 / nit 2）
