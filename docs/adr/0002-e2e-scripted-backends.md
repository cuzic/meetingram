# ADR 0002: E2E 用の台本どおりに動く文字起こしと LLM

- Status: Accepted
- 日付: 2026-11-21
- 関連: ADR 0001（議論グラフ）、`.github/workflows/windows-e2e.yml`（PR #19）、meetingram-core の S1 の合成会議
  （meetingram-core PR #9、`spikes/s1-discussion-graph/data/*.jsonl` と `gold/*.json`）、
  レビュー `docs/reviews/adr-0002-round1.md`

## 背景

PR #19 で、Windows のランナーで実際にアプリを起動し、`meetingramctl` で録音を開始・停止し、その画面を録画するようになった。
録画に写るのは録音の開始・停止と音量の表示までで、このアプリの目的である「文字起こし → 議論グラフ → 図」は写らない。
CI に API キーも認証済みの CLI も無いので、文字起こしは `NotConfigured`、要約とグラフは起動の条件を満たさない。

- 文字起こしの経路（meetingram-core）: アプリは `run_windows_capture_session`（app `recording.rs:131-148`）を呼び、
  コアの `windows_session.rs:108-117` が `run_live_transcription`（`live_transcription.rs:791`）を呼ぶ。
  プロバイダはその中で、資格情報ストアが無ければ止め（`:815`）、あればキーリングの選択から `build_stt_provider` で組み立てる（`:828`）。
  1 つのプロバイダを両トラックで共有する（`:887-902`、再接続 `:1052`、`:1056`）。外から差し替える口は無い。
  `SttSessionConfig`（`crates/stt-api/src/lib.rs:163-173`）にトラックの情報は無い。
- イベントのチャネルが閉じると「予期しない切断」として欠落（`transcription_gaps`）を記録し、再接続する（`live_transcription.rs:1124-1160`）。
- 議論グラフ（PR #15 の `graph_consumer.rs`、作業中）は `build_backend` の中で、キーリングのプロバイダ → `supports_graph()`（Q6）→
  `resolve_model`（Q7）→ CLI の `is_available()` の順に確かめ、`Backend { model, summarizer }` を作る。
  テスト用に `start_recording_with(sid, windows, |settings| -> Result<Backend, DisabledReason>)` がある。
  要約（`summary_consumer.rs:273-315`）は別に `summarize::build_summarizer` を呼ぶ。
- ヒントは `hint.rhai` → `std::call_ai` → コアの `rhai-engine/src/dispatcher.rs:72-133` が自分でキーリングから組み立てた
  `Summarizer` を使い、先に RAG（既定は Cloudflare）を呼ぶ。プラグインの配置と `hint_enabled`（既定は無効）も要る。
- 合成の会議データ: S1 の `design.jsonl` は 150 発話・約 15 分、話者は「自分」「相手」。`gold/design.json` は正解の IBIS グラフ
  （ノード 39、エッジ 32。ノードには根拠の発話の `row_id`、エッジには根拠が無い）。どちらも合成。

## 決めること / 決めないこと

- 決める: 偽の文字起こしと LLM の置き場所、有効にする条件、返すもの、本番に混ざらない仕組み、E2E で確かめること。
- 決めない:
  - **ヒント**。偽の LLM を通らず、RAG・プラグインの配置・デバウンスの問題もあるので、別の段（別の ADR か改訂）に回す。
  - グラフと図の画面（ADR 0001 のタスク 6・7）。できたら、この仕組みでそのまま録画に写る。
  - 本物の LLM・STT を使った確認（GCP の VM。owner の判断待ち）。

## 選択肢

### 偽物の作り方

#### A. 台本（シナリオ）どおりに返す（採用）

文字起こしは、シナリオの発話を時刻どおりに確定として流す。LLM は、増分のプロンプトに出てきた発話をシナリオの発話と照合し、
正解グラフから「その発話で初めて根拠がそろうノード」と、両端がそろったエッジを返す。
会議の進みに合わせて図が育つ。**停止後に流し切った後の最終形**は毎回正解グラフと同じになる。
途中の形は、区切り（6 件か 45 秒）と呼び出しの直列化によって毎回少し変わる。

#### B. 録音した音声を流し、本物の STT と LLM を使う

本番どおりだが、キーが要る。CI では使えない。GCP の VM での確認に回す。

#### C. 決まった応答を呼び出し順に返す

区切りが時刻に左右されるので、順番だけで返すと存在しない発話やノードを指して検証で落ちる。採らない。

### 置き場所

#### P. 偽物はアプリの新しい crate `apps/e2e-scripted`、コアには差し替えの口とプロンプトの解析だけ（採用）

シナリオの形式と偽物はアプリの都合なのでアプリに置く。コアには、STT を外から渡す口（§2）と、
増分のプロンプトから発話とノードを取り出す公開関数（§3）を足す。後者は、プロンプトの形を変える PR で一緒に直せるようにするため。

#### Q. コアに偽物の crate を置く

コアの公開 API が E2E の都合で増える。採らない。

## 決定

### 1. 有効にする条件（本番に混ざらないこと）

- アプリの cargo feature `e2e-scripted`（既定は無効）を付けたときだけ、偽物のコードが入る。
  - release ビルドでこの feature を付けるとコンパイルできないようにする（`desktop` crate の feature で判定し、`desktop` に置く）:
    `#[cfg(all(feature = "e2e-scripted", not(debug_assertions)))] compile_error!(...)`。
  - `desktop-bundle.yml`（`dx build --release`）で出来た exe に、目印の文字列（`e2e scenario active`）が無いことを grep で確かめる。
- その上で、環境変数 `MEETINGRAM_E2E_SCENARIO=<シナリオのディレクトリ>` があるときだけ使う。無ければ本番と同じ動き。
- 有効なときは:
  - 画面の上部に「E2E シナリオ」の帯を出す。録画を本物の会議と取り違えないため。
  - ログに `e2e scenario active` を 1 回だけ出す（シナリオの名前だけ）。
  - STT・要約・グラフの LLM には、キーリングの設定とキーを**使わない**（設定画面などがキーリングを読むことはある）。
  - silence gate は無効にする（gate は捨てた時間を時刻に足すので、壁時計で出す偽物の時刻とずれる）。

### 2. 偽の文字起こし（`ScriptedSttProvider`）

- **コアの変更**（meetingram-core）:
  - `run_windows_capture_session` に、引数を 1 つ足す: `LiveTranscriptionOptions`。今の `silence_gate_enabled` 引数はこの中に移す。
    今後の追加もこの構造体に足す。構造体は常に定義し（`windows-supervisor` だけの構成でも stub が使う）、作るのは `Default` とビルダー。
    `stt_override: Option<SttOverride>` のフィールドと `with_stt_override` だけを `#[cfg(feature = "live-transcription")]` にする。
  - `SttOverride { self_track: Arc<dyn SttProvider>, remote_track: Arc<dyn SttProvider> }`。トラックごとにするのは、
    `SttSessionConfig` にトラックが無く、1 つのプロバイダではどちらのセッションか分からないため。
  - `run_live_transcription` は、`stt_override` があれば `:815`（資格情報ストアが無い）と `:828`（組み立て）の**前**で分岐し、
    トラックごとのプロバイダを使う。共有している 4 か所（最初の接続、再接続 `:1052`・`:1056` を含む）をトラックごとに変える。
    目標のサンプリングレートは入力のまま。
  - `live-transcription` feature が無い構成の stub（`:2827-2840`）には `SttOverride` を出さない（`stt-api` が optional のため）。
- **アプリの実装**（`apps/e2e-scripted`）:
  - シナリオの `utterances.jsonl`（`row_id`、`speaker_label`、`start_ms`、`end_ms`、`text`）を読む。
    「自分」の発話を Self のプロバイダに、「相手」の発話を Remote のプロバイダに持たせる。
  - **時計は両トラックで 1 つ**: 最初の `start_session` の時刻を起点に、経過時間（壁時計）×`speed`（既定 10 倍）が
    `end_ms` を過ぎた発話を `FinalTranscript` で出す。10 倍では発話の間隔が約 100 ms なので、トラックごとの時計では順が入れ替わる。
    `audio_start_ms` / `audio_end_ms` は `start_ms / speed`、`end_ms / speed`。
  - **進み具合はプロバイダが持つ**: 次に出す行をプロバイダが覚え、再接続のときは続きから出す。
  - **送信側はセッションが持ち続ける**: イベントのチャネルは `finalize` まで閉じない（閉じると欠落と再接続になる）。
    `finalize` では、まだ出していない発話を出さずに終える（録音を止めた時点までが会議）。出す役のタスクを止め、
    送信側をすべて手放してから戻る（残すと `run_live_transcription` のループ（`:941`）が終わらず、停止が終わらない）。
  - `FinalTranscript` の `words` は `None` にする（話者の番号があるとラベルが「自分 (話者1)」になり、§3 の照合が外れる）。
  - `send_audio` は音を捨てる。
  - 15 分のシナリオを 10 倍で流すと 90 秒。CI では 100 秒録音してから止める。
- 再文字起こし（`BatchSttProvider`）は偽物を作らない。E2E では欠落が起きない前提で、起きたら §5 の確認で落ちる。

### 3. 偽の LLM（`ScriptedSummarizer`）

- **差し込み方**（PR #15 の作者と合意済み）: PR #15 のマージ後、アプリの `AppState` に LLM のバックエンドを作る工場（トレイト）を置く。
  - `graph() -> Result<Backend, DisabledReason>` と `summary() -> Result<(Box<dyn Summarizer>, String /* モデル名 */), _>`。
  - 要約の consumer とグラフの consumer は、どちらもこの工場から作る。本番の実装は今の `build_backend` と `build_summarizer` をそのまま使う。
  - 偽物の工場は `Backend` を直接返し、キーリングのプロバイダ・Q6・CLI の `is_available` を通らない。
    `SummaryProvider` に偽の値は足さない（保存済みの鍵の互換とコアのテストのため）。
  - `graph_enabled` と Windows の判定は工場の外なので、偽物でも効く。
  - `graph()` は、録音の開始だけでなく、会議の後の操作（続きを抽出、再文字起こしの後、作り直し）でも使う。
  - シナリオによるグラフの設定の上書き（§4）は、`AppSettings` を書き換えず、工場か consumer に渡す所で当てる。
- **プロンプトの解析はコアの公開関数**（`discussion_graph`）: 増分のプロンプトの user から、
  `new` と `reference` の発話（`uid`、`speaker_label`、本文）と、`context` のノード（`nid`、`kind`、本文）を取り出す。
  増分でなければ `None`（それで整理などと見分ける。system では見分けない）。
  `prompt.rs` の形を変える PR は、この関数と、§5 の単体テストも一緒に直す。`prompt.rs` に触る前に PR #15 の作者に声をかける。
- **増分の抽出への応答**:
  - 発話の照合は（`speaker_label`、本文の全文）で行う。先頭の数文字では「はい」などが重なる。同じ組が複数あるときは、
    偽の LLM がセッションごとに「`uid` → `row_id`」の対応を覚え、台本の行を前から順に消費する
    （再試行や流し切りで同じ `uid` がもう一度来たら同じ行）。
  - 正解グラフのノードのうち、根拠の最初の `row_id` が今回の `new` にあるものを `add_node` で返す
    （`tmp` は正解の id、`grounds` は根拠のうち今回見えている発話（`new` と `reference`）の `uid`、`confidence` は 0.8）。
    同じ本文のノードが `context` にすでにあれば返さない（失敗後の再試行で二重にしない）。
  - エッジは、**端点の少なくとも一方が今回の `add_node`（`tmp`）**で、もう一方が `tmp` か `context` のノード
    （本文の完全一致で `nid`）のときだけ `add_edge` で返す。両端とも `context` のエッジは返さない
    （`context` には既存のエッジが載らず、偽物は状態を持たないので、返すと毎回重複・根拠の範囲外で却下される）。
    `grounds` は、今回作る端点の根拠の最初の発話の `uid`（必ず今回の `new` にある）、`confidence` は 0.8。
    片方がまだ無ければ返さず、後で作られたときに返す。`context` から外れて見えないときは出さない。
  - 何も返すものが無ければ `{"ops":[]}`。
- **それ以外の `complete`**（整理など）: `{"ops":[]}`。整理の効き目はこの仕組みでは確かめない（S2 の役目）。
- **要約**（`summarize`）: シナリオの `summary.md` をそのまま返す。
- 応答までの待ち時間: シナリオの `latency_ms`（既定 300）。タイムアウトの確認には使わない。
- ログには、呼び出しの種類、照合できた発話の件数、返した操作の件数だけを出す。

### 4. シナリオの置き場所と形

- `apps/e2e-scripted/scenarios/design/`（最初は 1 本）:
  - `utterances.jsonl`: S1 の `data/design.jsonl` をそのまま写す。
  - `graph.json`: S1 の `gold/design.json` をそのまま写す。
  - `summary.md`: 合成で書く。
  - `scenario.toml`: `speed`、`latency_ms`、グラフの設定の上書き（`graph_batch_utterances` など）。
    上書きはメモリ上だけで、`AppSettings` には保存しない。
- 合成のデータだけを置く。実際の会議の内容、人名、社名は入れない（AGENTS.md）。

### 5. 確かめること

- **単体テスト**（`apps/e2e-scripted`、Linux でも動く）: 本物の `discussion_graph::build_incremental` と `apply_patch` に、
  台本の全発話を 6 件ずつ通し、偽の LLM の応答を適用する。ノード 39・エッジ 32 がすべて適用され、却下が 0 件であること。
  プロンプトの形が変わったら、ここで落ちる。
- **普段の CI**: `app-build.yml` に `--features e2e-scripted` 付きの clippy と test を足す（feature 無しではコンパイルすらされないため）。
- **Windows の E2E**（`windows-e2e.yml`。`--features e2e-scripted` でビルドし、`MEETINGRAM_E2E_SCENARIO` を付けて起動）:
  - 録音中の `meetingramctl --json status` で、`transcription_status.self_status` と `remote_status` がどちらも `Connected`。
  - 停止後、セッションに文字起こしの行が 1 件以上ある（確かめ方は実装で決める。本文はログに出さない）。
  - 停止後、`last_session_id` のセッションのグラフの `rev` が 1 以上（ctl の `status` に載せる。`GraphStatus.rev` は PR #15 の作者が足す。
    コアの `control-protocol` の `StatusDto` に `#[serde(default)]` 付きで足すのは分割 3、アプリの control_server は分割 4）。
  - 照合できた発話の件数が 0 でない（偽の LLM のログ）。
  - `transcription_gaps` が 0 件（偽の STT の切断が無い）。
  - 録画に、文字起こしが流れる様子と「E2E シナリオ」の帯が写ること（人が見る。図はタスク 6・7 の後）。
- **配布物**: §1 の `compile_error!` と exe の grep。

### 6. ログ

ADR 0001 §8 と同じ。偽物も本文・プロンプト・応答をログに出さない。

## 影響

- コア: `run_windows_capture_session` の引数が変わる（破壊的変更。マイナー版を上げる）。`discussion_graph` に解析の公開関数が増える。
- アプリ: 新しい crate 1 つ、`desktop` に feature 1 つ、起動時の分岐、帯、LLM の工場。
- CI: `app-build.yml` に feature 付きの clippy と test。`e2e-windows` は録音が 100 秒に延び、全体で 2 分ほど増える。

## 実装の分割

1. **コア（STT の口）**: `LiveTranscriptionOptions` と `SttOverride`。テストは偽の `SttProvider` で、トラックごとに渡したものが使われる、
   再接続でも同じものが使われる、`None` なら今までどおり。→ マイナー版を上げてタグ。
2. **アプリ（文字起こし）**: `apps/e2e-scripted` の `ScriptedSttProvider`、シナリオの読み込み、feature と環境変数、
   `compile_error!`、帯、E2E の延長と §5 の文字起こしの確認、配布物の grep。
   コアのタグを上げるので `Cargo.toml` と `Cargo.lock` が PR #15 と重なる。**PR #15 のマージ後**に始める。
3. **コア（解析と DTO）**: `discussion_graph` のプロンプト解析の公開関数とテスト、文脈の窓の選び方（PR #15 の `recent_nodes`）を
   単体テストと共有できるよう公開する、`StatusDto` にグラフの `rev`。→ マイナー版を上げてタグ。
   `prompt.rs` と `graph_consumer` の作業と重なりうるので、始める前に PR #15 の作者と時期と置き場所を合わせる。
4. **アプリ（LLM）**: LLM の工場、`ScriptedSummarizer`、§5 の単体テスト、ctl の `status` にグラフの `rev`、E2E のグラフの確認。
   2・3 と `GraphStatus.rev` の後。

## Open questions

なし（ヒントの扱いは「決めない」に移した）。

## 改訂履歴

| 版 | 内容 |
| --- | --- |
| r1 | 初版（Proposed） |
| r2 | round 1 を反映。B1: ヒントを範囲外に。M1: 口は `run_windows_capture_session` の `LiveTranscriptionOptions`、feature の中だけ、両方の分岐の前。M2: AppState の LLM の工場（PR #15 の作者と合意）。M3: エッジの根拠と確信度。M4: 送信側を持ち続ける、進み具合と時計を共有。M5: 単体テストと feature 付きの CI。m1〜m12: 照合のキー、見分け方は解析の関数で、二重の add_node、silence gate を無効、「使わない」、`compile_error!` と exe の grep、`Connected`、rev は ctl の status、タイムアウトの確認を外す、上書きはメモリだけ、分割 2 は #15 の後、決まるのは最終形だけ。解析はコアの公開関数に（PR #15 の作者の提案） |
| r3 | round 2 を反映して Accepted。M-1: エッジは少なくとも一方が今回の `add_node` のときだけ。m-a: `LiveTranscriptionOptions` は常に定義し `stt_override` だけ cfg。m-b: `finalize` で送信側を手放す。m-c: `words` は `None`。m-d: `uid` → `row_id` を覚えて順に消費。m-e: 文脈の窓の選び方を共有。m-f: `StatusDto` はコア（分割 3）。m-g: 会議の後の操作でも工場、上書きは `AppSettings` に書かない。m-h: `compile_error!` は `desktop` |
