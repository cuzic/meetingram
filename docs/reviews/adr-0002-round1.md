# ADR 0002 r1 のレビュー（round 1）

対象: `docs/adr/0002-e2e-scripted-backends.md`（PR #22, b77a7a4）。照合先: meetingram-core main（440f215, v0.4.0）、app の main（349f77a）、PR #15（`graph_consumer.rs`。行番号は PR #15 の head 版）。

**結論**: 方式 A（台本どおりに返す）と配置 P（偽物はアプリ側）は妥当です。ただし「ヒントを録画に写す」は今の設計では実現できません（Blocker 1 件）。ほかに、STT の差し替え口を足す場所、グラフのバックエンドを差し替える場所、エッジの根拠の仕様、イベントのチャネルを閉じたときの再接続に、Major の穴が 5 件あります。

---

## Blocker

### B1. ヒントは偽の LLM を通らず、出る条件も CI で満たせない（§3・§5）

§3 は「Rhai のヒントなど、それ以外の `complete`」を `fallback.txt` で返すとしていますが、コードと合いません。

- `complete` を呼ぶのは `graph_consumer`（PR #15 `graph_consumer.rs:809-834`）だけです。ヒントは `hint.rhai` → `std::call_ai` → コアの `rhai-engine/src/dispatcher.rs:72-133` を通ります。dispatcher は自分でキーリングから `build_summarizer` を組み立て（`:106`）、`summarize()` を呼びます（`:124`）。アプリが渡す `Summarizer` は使われません。
- ヒントを生成するのは `%APPDATA%/meetingram/plugins` から読むプラグインです（app `main.rs:161-165`）。CI では誰もそこに置いていません。
- `hint_enabled` は opt-in です（app `actions.rs:53-75`、既定は false）。
- `hint.rhai:69-72` は先に `rag_search`（既定は Cloudflare。`rhai-engine/src/rag.rs:43-47`）を呼び、ネットワークと認証情報を必要とします。
- デバウンスは確定セグメントが 1 件来るたびにリセットされます（`rhai-engine/src/lib.rs:326`。既定は 15 秒、`actions.rs:61`）。10 倍速では平均約 0.6 秒ごとに確定が来るので、会議の途中では発火しません。

**修正案**: この ADR の目標を「文字起こし → 議論グラフ（＋要約）」に絞り、§5 の「ヒントの表示」を外してください。ヒントを写すなら別の段にします。その段では次の 4 つをすべて決めます。
1. コアの `RhaiEngine` に `Summarizer` と RAG を外から渡す口を足す。
2. E2E でプラグインを配置し、`hint_enabled` を設定する。
3. `hint_debounce_seconds` を短くするか、台本に間（ま）を入れる。
4. RAG の偽物を用意する。

---

## Major

### M1. STT の差し替え口を足す場所が違い、影響の範囲が書かれていない（§2・影響）

- アプリは `run_live_transcription` を直接呼んでいません。呼んでいるのはコアの `windows_session.rs:108-117` です。アプリが呼ぶのは `run_windows_capture_session`（app `recording.rs:131-148`。引数はすでに 15 個）です。したがって変わるコアの公開 API は `run_windows_capture_session` の側です。
- 「macOS の空実装（`:2832`）」は誤りです。これは `not(feature = "live-transcription")` のときの stub（`live_transcription.rs:2827-2840`）です。この構成では `stt-api` が optional です（`app-service/Cargo.toml:23` と `:93`）。そのため、`dyn SttProvider` を含む `SttOverride` を stub のシグネチャに載せるとコンパイルできません。
- 差し替えたときは、`:815`（credential_store が `None`）と `:828`（`build_stt_provider`）の両方より前で分岐する必要があります。
- 今は 1 つの `provider` を両トラックで共有しています（`:887-902`、`:1052`、`:1056`）。トラックごとにするなら、この 4 か所すべてを差し替えます。

**修正案**: `SttOverride` は `live-transcription` feature の中だけで定義します。`run_windows_capture_session` には「設定の束」を表す構造体（例: `LiveTranscriptionOptions { silence_gate_enabled, stt_override }`）を 1 つ足し、引数を増やさずに渡します。こうすれば今後の破壊的変更も 1 回で済みます。アプリ側は `#[cfg(windows)]` の中でだけ組み立てます。§影響と実装の分割 1 も、これに合わせて直してください。

### M2. グラフは `Summarizer` を差し替えるだけでは起動しない（§3・影響）

PR #15 の `GraphConsumer::backend`（`graph_consumer.rs:257-276`）→ `build_backend`（`:463-498`）は、次の順に判定します。
1. キーリングからプロバイダを読む。
2. 読めなければ `Claude` とみなす。
3. `supports_graph()` を確かめる（`:476`）。
4. `cli.is_available()` を確かめる（`:270`）。

CI にはキーも CLI も無いので、`DisabledReason::Backend` で起動しません。さらに、`Backend` には `model` も要ります。要約の側（`summary_consumer.rs:273-315`）は、これとは別の組み立て方とゲートを持っています。

**修正案**: 「`Summarizer` を作る所を 1 か所に寄せる」ではなく、`AppState` に LLM のバックエンドを作る工場（トレイト）を置きます。工場は `graph() -> Result<Backend, DisabledReason>` と `summary() -> Result<(Box<dyn Summarizer>, String), _>` を返し、偽物の実装はゲートを通さずに `Ok` を返します。この形を PR #15 の作者（cuzic-93）と先に合意してから、タスク 3 に入ってください。

### M3. エッジの `grounds` と `confidence` が決まっていない（§3）

`patch.rs` の検証は、すべての操作に空でない `grounds` を求めます（`:201`、`:260-265`）。`add_edge` には `confidence` も必須です（`:207-208`）。一方、正解グラフのエッジには根拠がありません（`gold/design.json` の edges は `src`/`dst`/`kind` だけ）。ADR のとおりに実装すると、エッジは全件 `EmptyGrounds` で落ちます。

**修正案**: エッジの `grounds` は「後からそろった側の端点の、最初の根拠の `uid`」とし、`confidence` は 0.8 にする、と明記します。その `uid` は今回の new に含まれるので、allowed の条件を満たします。

なお、正解の 3 本では、6 件ずつの区切りを仮定して文脈の窓（未解決の issue ＋直近 10 rev）をシミュレートすると、エッジ 32/36/35 本が全部出ました。また、正解のエッジは `valid_edge`（`patch.rs:530-541`）を全件満たし、`sub_issue_of` もありません。

### M4. 偽の STT がチャネルを閉じると、再接続して最初から流し直す（§2）

イベントのチャネルが閉じると「unexpectedly closed」として扱われ、`note_disconnect` → `open_gap`（`transcription_gaps` に行を書く）→ 再接続、と進みます（`live_transcription.rs:1124-1137`、`:1152-1160`）。新しいセッションは時計を 0 から始めるので、発話が二重に出て、欠落の行も残ります。

**修正案**: 次の 3 点を ADR に書きます。
- 送信側（sender）はセッションが持ち、`finalize` で初めて手放す。
- 台本の進み具合（epoch と次の行）はプロバイダが持ち、再接続しても続きから出す。
- 両トラックで 1 つの時計を共有する（epoch は最初の `start_session`）。

3 点目の理由: 発話の間隔は 10 倍速で約 100 ms しかありません。トラックごとに時計がずれると Self と Remote の順が入れ替わり、`uid` の順（`store/mod.rs:67-107` は `transcript_segments.id` の順）が台本とずれます。

### M5. 偽の LLM の照合が `windows-e2e` でしか確かめられない（§3 末尾・§5）

プロンプトの形は、`prompt.rs:239-250` の `context` / `reference` / `new` の見出し行と、JSON 1 行ずつの並びです。この形を検査しているのが Windows の E2E（録画を含む）だけだと、壊れたときに気づくのが遅くなります。さらに `app-build.yml:60,64` は feature を付けずに clippy と test を回すので、`e2e-scripted` のコードは普段の CI でコンパイルすらされません。

**修正案**:
- `apps/e2e-scripted` に単体テストを置きます。本物の `discussion_graph::build_incremental` と `apply_patch` に台本の全行を 6 件ずつ通し、ノード 39 / エッジ 32 がすべて適用され、却下が 0 件であることを確かめます。
- `app-build.yml` に `--features e2e-scripted` 付きの clippy と test を足します。

---

## Minor

- **m1. 照合のキー（§3）**: 先頭 40 文字では「うん」「はい」「了解です」が 3 本とも重複します。本文は `limit/3`（8000 字）まで切られずに届きます（`prompt.rs:125`、`:143`）。照合は（`speaker_label`、本文の全文、出現順）にし、行は `serde_json` で読んでください。`reference` の `uid` も根拠に使えると明記してください（`prompt.rs:155-156`）。
- **m2. 呼び出しの見分け方（§3）**: ADR に書かれていません。基準は「user が `context\n` で始まり `\nnew\n` を含む → 増分、それ以外 → 整理（`[]`）」とします。system を見分けに使わないでください（`BASELINE` も `extraction.txt` で始まるため。`prompt.rs:14`）。parse に失敗して再試行するときは、user の末尾に `RETRY_NOTE` が付きます（PR #15 `graph_consumer.rs:172`）。
- **m3. 二重の add_node**: 失敗して同じ発話がもう一度 new に来たときは、context に同じ本文のノードがあれば返さないようにします。
- **m4. silence gate**: アプリの既定は無効です（`recording.rs:116-121`）。そのため §2 で「壁時計にする」理由として挙げた「Remote の時計が進まない」は、既定の CI では当たりません。逆に gate を有効にすると、`TimestampMapper` が捨てた分の時間を足すので（`timestamp_mapper.rs:75-100`）、壁時計で出した Remote の時刻が二重にずれます。差し替えたときは gate を無効にすると書いてください。
- **m5. 「キーリングを読まない」（§1）**: 要約の `provider_model_string`（`summary_consumer.rs:317-340`）、設定画面、`gap_retranscription.rs:43` はキーリングを読みます。「使わない」に弱めるか、読む所を列挙してください。
- **m6. 配布物の確認（§5）**: 配布物は `dx build --package desktop --release`（`desktop-bundle.yml:110`）で作られ、feature は付きません。`cargo tree` で確かめるよりも、次の 2 つのほうが確実です。
  - `#[cfg(all(feature = "e2e-scripted", not(debug_assertions)))] compile_error!(...)` で、release ビルドに feature が付くこと自体を禁じる。
  - 出来た exe にマーカーの文字列（`e2e scenario active`）が無いことを grep で確かめる。
- **m7. §5 の判定**: 「`NotConfigured` でないこと」ではなく、`transcription_status.self_status` と `remote_status` がどちらも `Connected` であることを確かめます（`control-protocol/src/lib.rs:96`、`:106-107`）。
- **m8. Q1（rev を読む口）**: 偽の LLM が info で「適用した rev と件数」をログに出し、`collect-windows-e2e-log.ps1` で拾えば、`control-protocol` を変えずに済みます。
- **m9. `latency_ms` でタイムアウトを確かめる件（§3）**: 既定の 500 ms では、上限の 60 s（`graph_consumer.rs:731-736`）に届きません。外すか、上限を超えるシナリオを別に作ってください。
- **m10. scenario.toml による設定の上書き（§4）**: 上書きはメモリ上だけにし、`AppSettings` に保存しない（設定画面で保存しても残らない）と明記します。
- **m11. コアのタグ（実装の分割 2）**: app は今 v0.3.0 で、PR #15 が v0.4.0 に上げます。分割 2 はコアの新しいタグに上げるため、`Cargo.toml` と `Cargo.lock` が PR #15 と衝突します。「触るファイルが重ならない」は成り立たないので、PR #15 のマージ後に回してください。
- **m12. 「毎回同じ見た目」（§A）**: 区切り（6 件か 45 秒）と LLM の呼び出しの直列化によって、途中のグラフは毎回変わります。毎回同じになるのは、停止後に流し切った後の最終形だけだと書き分けてください。
