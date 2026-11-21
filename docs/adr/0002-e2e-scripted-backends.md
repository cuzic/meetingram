# ADR 0002: E2E 用の台本どおりに動く文字起こしと LLM

- Status: Proposed
- 日付: 2026-11-21
- 関連: ADR 0001（議論グラフ）、`.github/workflows/windows-e2e.yml`（PR #19）、meetingram-core の S1 の合成会議
  （meetingram-core PR #9、`spikes/s1-discussion-graph/data/*.jsonl` と `gold/*.json`）

## 背景

PR #19 で、Windows のランナーで実際にアプリを起動し、`meetingramctl` で録音を開始・停止し、その画面を録画するようになった。
しかし、録画に写るのは録音の開始・停止と音量の表示までで、このアプリの目的である
「文字起こし → 議論グラフ → 図とヒント」は写らない。CI に API キーも認証済みの CLI も無いので、
文字起こしは `NotConfigured`、要約とグラフは起動の条件を満たさない。

- 文字起こしは meetingram-core の `app_service::live_transcription::run_live_transcription`
  （`crates/app-service/src/live_transcription.rs:791`）の中で、キーリングの選択とキーから
  `build_stt_provider` で組み立てる（同 `:828`）。外から差し替える口は無い。
  `SttProvider` / `SttSession`（`crates/stt-api/src/lib.rs:23-57`）はトレイトなので、実装を足すこと自体はできる。
- 要約と議論グラフは、アプリが `summarize::build_summarizer` で `Box<dyn Summarizer>` を作って使う
  （`apps/desktop/src/summary_consumer.rs:298`、グラフはタスク 5b の `graph_consumer.rs`。meetingram PR #15 で作業中）。
  `Summarizer` はトレイト（`crates/summarize/src/lib.rs:234`）なので、アプリの側で差し替えられる。
- 合成の会議データはすでにある: S1 の `design.jsonl` は 150 発話・約 15 分、話者は「自分」「相手」、
  `gold/design.json` は正解の IBIS グラフ（ノード 39、エッジ 32、各ノードに根拠の発話の `row_id`）。
  どちらも合成で、実際の会議の内容は含まない。

## 決めること / 決めないこと

- 決める: 偽の文字起こしと LLM をどこに置くか、どう有効にするか、何を返すか、本番に混ざらない仕組み、E2E で何を確かめるか。
- 決めない: グラフと図の画面（ADR 0001 のタスク 6・7）。それができたら、この仕組みでそのまま録画に写る。
  本物の LLM・STT を使った確認（GCP の VM。owner の判断待ち）。

## 選択肢

### 偽物の作り方

#### A. 台本（シナリオ）どおりに返す（採用）

文字起こしは、シナリオの発話を時刻どおりに確定として流す。LLM は、プロンプトに出てきた発話をシナリオの発話と照合し、
正解グラフから「その発話で初めて根拠が出るノード」と、両端がそろったエッジを返す。
毎回同じ見た目になり、正解グラフに近い図が会議の進みに合わせて育つ。

#### B. 録音した音声を流し、本物の STT と LLM を使う

本番どおりだが、キーが要る。CI では使えない。GCP の VM での確認に回す。

#### C. 決まった応答を呼び出し順に返す

LLM の呼び出しの区切り（発話 6 件か 45 秒）は時刻に左右されるので、順番だけで返すと、存在しない発話やノードを指してしまい、検証で落ちる。採らない。

### 置き場所

#### P. アプリの新しい crate `apps/e2e-scripted`＋コアには差し替えの口だけ（採用）

シナリオの形式と偽物の実装はアプリの都合なので、アプリに置く。コアの `app-service` には、
STT を外から渡せる口を 1 つ足す（下の §2）。LLM はアプリの側で差し替えられるので、コアの変更は要らない。

#### Q. コアに偽物の crate を置く

コアの公開 API が E2E の都合で増える。採らない。

## 決定

### 1. 有効にする条件（本番に混ざらないこと）

- アプリの cargo feature `e2e-scripted`（既定は無効）を付けてビルドしたときだけ、偽物のコードが入る。
  `desktop-bundle.yml`（配布物）はこの feature を付けない。付いていないことを CI で確かめる（§5）。
- その上で、環境変数 `MEETINGRAM_E2E_SCENARIO=<シナリオのディレクトリ>` があるときだけ使う。
  feature 付きのビルドでも、環境変数が無ければ本番と同じ動き。
- 有効なときは:
  - 画面の上部に「E2E シナリオ」の帯を出す。録画やスクリーンショットを本物の会議と取り違えないため。
  - ログに `e2e scenario active` を 1 回だけ出す（シナリオの名前だけ。本文は出さない）。
  - キーリングの STT・要約の設定とキーは読まない。設定の画面で変えても効かない。

### 2. 偽の文字起こし（`ScriptedSttProvider`）

- コアの変更（meetingram-core）: `run_live_transcription` に、STT を外から渡す引数を 1 つ足す。
  形は `Option<SttOverride>`（`SttOverride { self_track: Arc<dyn SttProvider>, remote_track: Arc<dyn SttProvider> }`。
  `None` なら今までどおりキーリングから組み立てる）。トラックごとにするのは、`SttSessionConfig`
  （`crates/stt-api/src/lib.rs:163-173`）にトラックが無く、1 つのプロバイダではどちらのトラックのセッションか分からないため。
  渡されたときは、プロバイダの種類は「外から渡されたもの」として扱い、目標のサンプリングレートは入力のまま。
  再接続（`attempt_reconnect`）も同じプロバイダを使う。macOS の空実装（`:2832`）にも同じ引数を足す。
- アプリの実装（`apps/e2e-scripted`）:
  - シナリオの `utterances.jsonl`（S1 と同じ形: `row_id`、`speaker_label`、`start_ms`、`end_ms`、`text`）を読む。
  - トラックとの対応: 「自分」の発話だけを持つプロバイダを Self に、「相手」の発話だけを持つプロバイダを Remote に渡す。
  - 時刻: セッションの開始からの**経過時間（壁時計）**に `speed`（シナリオの設定。既定 10 倍）を掛けて、
    `end_ms` を過ぎた発話を `FinalTranscript` として出す。音声のサンプル数は使わない
    （Remote は無音を送らない `silence_gate` があり、音声の時計が進まないことがある）。
    `audio_start_ms` / `audio_end_ms` は `start_ms / speed`、`end_ms / speed` にする。
  - `send_audio` は音を捨てる。`finalize` で、まだ出していない発話を**出さずに**終える（録音を止めた時点までが会議）。
  - 15 分のシナリオを 10 倍で流すと 90 秒。CI ではシナリオを 90〜120 秒録音してから止める。
- 再文字起こし（`BatchSttProvider`）は偽物を作らない。E2E では欠落が起きない前提。起きたらログで分かる。

### 3. 偽の LLM（`ScriptedSummarizer`）

`Summarizer` を実装し、`build_summarizer` の代わりにアプリが渡す。どの呼び出しかはプロンプトの形で見分ける。

- **増分の抽出**（ADR 0001 §2）: プロンプトの「新しい発話」の各行から `uid` と本文を取り、本文の先頭 40 文字で
  シナリオの発話と照合して `row_id` を得る。
  - 正解グラフのノードのうち、根拠の最初の `row_id` が今回の新しい発話にあるものを `add_node` で返す（`tmp` は正解の id、
    `grounds` は根拠のうち今回見えている発話の `uid`、`confidence` は 0.8）。
  - エッジは、両端のノードが「今回作るもの（`tmp`）」か「プロンプトの既存ノード（本文の完全一致で `nid` を得る）」で
    そろったときに `add_edge` で返す。そろわなければ返さない（後の呼び出しで片方が作られたときに返す）。
    既存ノードがプロンプトの枠から外れて見えないときは、そのエッジは出さない。
  - 照合できない発話は無視する。何も返すものが無ければ `{"ops":[]}`。
- **整理**（強めた整理、分割の経路）: `{"ops":[]}` を返す。整理の効き目はこの仕組みでは確かめない（S2 の役目）。
- **要約**（`summarize`）: シナリオの `summary.md` をそのまま返す。
- **Rhai のヒントなど、それ以外の `complete`**: シナリオの `fallback.txt` を返す。
- 応答までの時間: シナリオの `latency_ms`（既定 500）だけ待つ。`req.timeout` を超えるなら `Timeout` を返す（タイムアウトの経路の確認用）。
- 照合はプロンプトの形に依存する。プロンプトの形が変わって照合が 0 件になったら E2E で気づけるように、§5 で確かめる。

### 4. シナリオの置き場所と形

- `apps/e2e-scripted/scenarios/design/`（最初は 1 本）:
  - `utterances.jsonl`: S1 の `data/design.jsonl` をそのまま写す。
  - `graph.json`: S1 の `gold/design.json` をそのまま写す。
  - `summary.md`、`fallback.txt`: 合成で書く。
  - `scenario.toml`: `speed`、`latency_ms`、グラフの設定の上書き（`graph_batch_utterances` など）。
- 合成のデータだけを置く。実際の会議の内容、人名、社名は入れない（AGENTS.md）。

### 5. E2E で確かめること（`windows-e2e.yml` に足す）

- `--features e2e-scripted` でビルドし、`MEETINGRAM_E2E_SCENARIO` を付けて起動する。
- 録音を 90〜120 秒続け、`meetingramctl --json status` で `transcription_status` が `NotConfigured` でないこと。
- 停止後、セッションの DB に文字起こしの行が 1 件以上あること（確かめ方は実装で決める。
  ctl の出力を増やすか、PowerShell から読むか。どちらでも本文はログに出さない）。
- グラフ: 起動の条件を満たし、rev が 1 以上進んだこと（graph_consumer の状態を読む口があればそれで。無ければ Open questions）。
- 照合が 0 件でないこと（偽の LLM がログに件数だけ出す）。
- 配布物のビルド（`desktop-bundle.yml`）に `e2e-scripted` が入っていないこと（`cargo tree -e features` などで確かめる）。
- 録画に、文字起こしが流れる様子とヒントの表示が写ること（人が見る。図はタスク 6・7 の後）。

### 6. ログ

ADR 0001 §8 と同じ。偽物も本文・プロンプト・応答をログに出さない。出してよいのは、シナリオの名前、
照合できた発話の件数、返した操作の件数、呼び出しの種類だけ。

## 影響

- コア: `run_live_transcription` の引数が増える（破壊的変更。meetingram-core のマイナー版を上げる）。
- アプリ: 新しい crate 1 つ、`desktop` に feature 1 つ、起動時の分岐、帯の表示。`graph_consumer.rs`（PR #15）と
  `summary_consumer.rs` は、`Summarizer` を作る所を 1 か所に寄せ、そこで偽物と差し替える。
- CI: `e2e-windows` のビルドに feature が付き、録音が 90〜120 秒に延びる（全体で 2 分ほど増える）。

## 実装の分割

1. **コア**: `run_live_transcription` に STT の差し替えの口（`SttOverride`、トラックごと）。テストは偽の `SttProvider` で、
   渡したものが使われること、再接続でも同じものが使われること、`None` なら今までどおり。→ マイナー版を上げてタグ。
2. **アプリ（文字起こし）**: `apps/e2e-scripted` の `ScriptedSttProvider`、シナリオの読み込み、feature と環境変数、帯の表示、
   E2E の延長と文字起こしの確認。PR #15 と触るファイルが重ならない範囲。
3. **アプリ（LLM）**: `ScriptedSummarizer`、`Summarizer` を作る所の一本化、E2E のグラフの確認。
   PR #15（`graph_consumer`）がマージされてから。

## Open questions

- Q1: グラフの rev を E2E から読む口。graph_consumer の状態を ctl の `status` に載せるのが素直だが、
  ctl の応答の形はコアの `control-protocol` にある。タスク 3 で cuzic-93 の作業（タスク 5b・6）とすり合わせる。

## 改訂履歴

| 版 | 内容 |
| --- | --- |
| r1 | 初版（Proposed） |
