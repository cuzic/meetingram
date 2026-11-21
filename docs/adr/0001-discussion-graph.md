# ADR 0001: 議論グラフ（discussion graph）の持ち方と作り方

- Status: Accepted
- 日付: 2026-11-21
- 関連: meetingram-core `docs/decouple-summary-transcription.md`、`docs/rhai-plugin-summary.md`、`docs/logging-policy.md`、
  レビュー `docs/reviews/adr-0001-round1.md`

## 背景

meetingram の目的は、会議の議論を図（マインドマップ、TOC の CRT・対立解消図）にし、
次に何を聞くべきかを支援すること（`AGENTS.md:3`）。今のコードにあるのは、文字起こしの表示、
要約、Rhai のヒントだけで、議論の構造を持つものは無い。

- 文字起こしは `TranscriptEvent`（meetingram-core `crates/transcript-event/src/lib.rs:29-67`）で
  `transcription.{sid}.segment.finalized` などに流れる。`SegmentData` には `segment_id`、`text`、
  `speaker_label`（「自分」「相手」＋話者番号、`crates/app-service/src/live_transcription.rs:103`）、
  `track`、`start_ms` / `end_ms` がある。単語単位の時刻と確信度は載らない。
- **発話を指す安定した鍵が今は無い**:
  - `segment_id` は、ライブでは `{sid}:{track}:{counter}`（`lib.rs:155`）、DB から組み直すと
    `{sid}:{track}:{start}-{end}`（`lib.rs:165`）で、同じ発話でも経路によって違う。
  - `transcript_segments` 表（`crates/session-store/src/schema.rs:98-110`）には `segment_id` 列が無い。
  - OpenAI の STT は確定に時刻を付けない（`crates/stt-openai/src/lib.rs:373-379`）。
    Self トラックは再接続後に時刻が 0 から数え直しになる（`live_transcription.rs:679-690`）。
  - `persist_pending_interim`（`live_transcription.rs:1397-1425`）が作る確定行はイベントを出さず、DB にだけある。
  - 一方で、`transcript_segments.id`（AUTOINCREMENT）は、どの経路の確定行にも必ずある。
- ライブの文字起こしは Windows でしか有効にしていない（`apps/desktop/Cargo.toml:61`）。
  macOS の録音は `transcript_segments` に何も書かない（`crates/app-service/src/macos_session.rs`）。
- Local Broker（`crates/local-broker/src/lib.rs`）はプロセス内の `broadcast`（subject ごとに 256 件）で、
  保持しない。遅れた購読者は `Lagged` を受ける。購読者が居ないと `publish` は `NoSubscribers` を返す（`lib.rs:67-84`）。
- SessionStore は `Mutex<Connection>`（std、`crates/session-store/src/store.rs:87`）1 つで、アプリは 1 つだけ開き
  （`apps/desktop/src/main.rs:96`）、録音のセグメント登録・文字起こしの保存・UI のポーリングが同じロックを取る。
  DB は WAL で `busy_timeout` 5 秒（`schema.rs:156-163`）。今は書き手が全員この Mutex に並ぶので、プロセス内で `SQLITE_BUSY` は起きない。
  Windows の録音は音声をメモリに溜め、録音の停止後に `run_pipeline` でセグメントを 1 つずつ保存する（`windows_session.rs:123-141`）。
  ここで書き込みが失敗すると、残りの音声は保存されない。`UtteranceEnded(SessionEnd)` はこの保存の**前**に出る（`live_transcription.rs:902-914`）。
  `session.{sid}.stopped` は保存の後に出る（`apps/desktop/src/recording.rs:170-179`）。移行の仕組みは無く、`CREATE TABLE IF NOT EXISTS` に足していく（`schema.rs:5-7`）。
- 要約は `Summarizer::summarize(&[TranscriptTurn], &SummarizeOptions) -> String`
  （`crates/summarize/src/lib.rs:158`）で、自由文しか返さない。会議終了時（`summary_consumer.rs:56`）、
  手動の要約ボタン（`summary_consumer.rs:151`）、Rhai の `ai_summarize`（`dispatcher.rs:82-88`）から呼ばれる。
  プロバイダとモデルはキーリングに保存した設定から選ぶ（`summary_consumer.rs:229-271`。同じ組み立てが `dispatcher.rs:90-128` にもある）。
  CLI のバックエンドはプロンプトをコマンドライン引数 1 つで渡す（`cli_backend.rs:137-138`）。
- Rhai の dispatcher（`crates/rhai-engine/src/dispatcher.rs:45-56`）には、RAG と LLM への送信はあるが、
  任意の URL に送る汎用のコマンドは無い。
- アプリの UI は Dioxus 0.7 のデスクトップ（webview）で、`ui.rs` が 250ms ごとにバッファを読む（`ui.rs:507-576`）。
  SVG や JS の橋渡しは無い。

図もヒントも、元になる「議論の構造」が要る。図ごとに文字起こしから直接作ると、図の間で中身が食い違い、
会議中に何度も作り直すと LLM の費用がかさむ。そこで、文字起こしと図の間に、
会議ごとに 1 つの永続的な議論グラフを置き、図やヒントはそこから作る。

### 先行研究から借りるもの

- **IBIS**（Kunz & Rittel 1970）と、それを会議中に描く Dialogue Mapping（Conklin）。
  会議をリアルタイムで構造化するシステムは、ほぼ IBIS（問い・案・賛否）を使っている
  （MeetMap, CSCW 2025, arXiv 2502.01564／EchoMind, CSCW 2025／BCause, arXiv 2505.03584）。
- **Inference Anchoring Theory（IAT）**: 「誰がいつ何を言ったか（発話）」と「何が主張されたか（命題）」を
  別の層に分ける。言い換え・繰り返し・撤回を、命題を重複させずに表せる（QT30 コーパス、DialAM-2024）。
- **自動抽出だけでは精度が足りない**: 対話の議論マイニングの共有タスク DialAM-2024 では、
  最良でも議論マップ全体の F1 が 48〜67。LLM で抽出し、人が確認する形が主流
  （Anastasiou & De Liddo, DELITE 2024／AM4DSP, EMNLP 2025 demo）。
- **根拠への戻り道**: ノードから元の発話に戻れることが信頼の前提（ArguScale, arXiv 2610.15319）。
- **確信度で人の確認を絞る**（arXiv 2606.16047 の confidence gating）。
- **増分更新と重複の統合**は、会話メモリの知識グラフ（Zep/Graphiti, arXiv 2501.13956）の作り方を借りる。

## 決めること / 決めないこと

- 決めること:
  - 議論グラフのデータモデル（発話層・命題層・エッジ・確信度・根拠・状態）
  - 抽出の仕方（会議中の増分、会議後の流し切りと整理、作り直し）と LLM への入出力の形、検証の規則
  - どの crate・どの接続で動かすか、保存先、イベント
  - 会議後の承認・却下の UI と、v1 の表示
  - 失敗したときの挙動、ログに出すもの
- 決めないこと（別 ADR）:
  - 図への射影と描画（マインドマップ、CRT、対立解消図、svg-diagram の使い方）。v1 の表示はアウトラインだけ
  - グラフの穴から質問を作ってヒントに出すこと（v1 は穴の検出関数を用意するだけ）
  - 因果（`causes`）の抽出。CRT の ADR で決める（v1 のスキーマは種類を足せるようにしておく）
  - 会議をまたいだ蓄積（Q4）
  - ノードやエッジの自由な編集（追加・付け替え）
  - 音声の再生による根拠の確認
  - macOS でのグラフ（macOS は文字起こしを保存しないため。macOS の文字起こしの ADR の後に扱う）

## owner の決定

| # | 問い | 決定 | 日付 |
| --- | --- | --- | --- |
| Q1 | 文字起こしを送る LLM | 要約と同じ設定（プロバイダ・モデル）を使う。新しい送信先は増やさない | 2026-11-21 |
| Q2 | いつ作るか | 会議中に増分で作り、会議の後に全体を見直して整理する | 2026-11-21 |
| Q3 | v1 の人による確認 | 会議の後に、確信度の低いものを承認・却下するだけ。自由な編集は後回し | 2026-11-21 |
| Q4 | 会議をまたいだ蓄積 | v1 の範囲外。別 ADR。保存形式は会議をまたいで広げられる形にしておく | 2026-11-21 |
| Q5 | 既定で有効にするか | 既定で有効（`graph_enabled = true`）。会議中に文字起こしを何度も LLM に送り、要約より費用が増えることを承知のうえ | 2026-11-21 |

## 選択肢

### 動かす場所

#### A. Rust: コアの新 crate＋アプリの consumer（採用）
- 仕組み: 純粋なロジック（型、パッチの検証と適用、穴の検出、プロンプトの組み立て、応答の解析）と、
  グラフ専用の保存（自前の SQLite 接続）をコアの新しい crate `discussion-graph` に置く。LLM の呼び出しは summarize。
  アプリの `graph_consumer.rs` が `summary_consumer.rs` と同じ形でイベントを受けて動かす。
- 良い点: 既存の仕組み（Broker、同じ DB ファイル、Summarizer、キーリングの設定）にそのまま乗る。
  検証と適用を純粋関数にでき、Linux でテストできる。
- 悪い点: プロンプトを変えるにはビルドし直しが要る（Rhai なら差し替えられる）。

#### B. Rhai プラグイン（`graph.rhai`）
- 仕組み: `on_segment_finalized` で発話を集め、`ai_summarize` で LLM を呼び、JSON を Rhai で解析する。
- 捨てた理由: グラフの状態を (script, session) の Scope に持つことになり、保存と読み直しのコマンドを
  いくつも足すことになる。`max_operations` と文字列・配列の上限に、長い会議のグラフが当たる。
  hook から呼んだヘルパー関数がモジュールの変数を読めない制約（`plugins/default/hint.rhai` 冒頭）もあり、
  検証ロジックを書きにくい。プロンプトの差し替えだけなら、A で外部ファイルから読めばよい（将来）。

#### C. 別プロセスの TypeScript（svg-diagram 側）
- 仕組み: Rhai から HTTP で送り、TS のプロセスがグラフを持ち、そのまま svg-diagram で描く。
- 捨てた理由: dispatcher に任意の URL へ送るコマンドが無い。デスクトップアプリに Node の実行環境を同梱することになる。
  プロセス間の Broker（`decouple-summary-transcription.md` §5 の phase 2）もまだ無い。
  描画に TS を使うかどうかは、図の ADR で別に決める（グラフは保存形式で受け渡せる）。

### 保存の接続

#### P. グラフ専用に 2 本目の SQLite 接続を開く（採用）
- 同じ DB ファイルを `open_with_pragmas`（WAL、`busy_timeout` 5 秒）で別に開き、`discussion-graph` の中に閉じ込める。
- 良い点: グラフ側で panic しても、録音側の `Mutex<Connection>` は poison されない。
  録音側のロックを待たせない（WAL なので読みは書きを止めない）。
- 悪い点: プロセス内に 2 人目の書き手ができるので、グラフ側が書きのロックを長く持つと、録音側の書き込みが
  `busy_timeout`（5 秒）を超えて `SQLITE_BUSY` で失敗しうる（レビュー N1）。次の 3 つで防ぐ（§7）:
  (1) グラフ側の書き込みはすべて `BEGIN IMMEDIATE` で始め、1 トランザクションで行うのは 1 パッチの適用か 1 回の取り込みだけ
  （中で LLM を呼ばない・JSON を解析しない。ミリ秒の単位で終わる）。
  (2) 会議の後の処理は、音声の保存が終わった後の `session.{sid}.stopped` でだけ始める。
  (3) グラフ側が書きのトランザクションを開いている間に `SessionStore` の書き込みが待ってから成功するテストを置く。
  また `transcript_segments` の列を `discussion-graph` が直接読むので、session-store の表の定義と結び付く（往復テストで守る）。

#### Q. SessionStore に表とメソッドを足す
- 捨てた理由: 録音と同じロックを取るので、グラフ側の長い読み出しや panic が録音に波及する（レビュー M1）。

### 抽出の仕方

#### X. 差分パッチによる増分更新（採用）
- 仕組み: 新しく確定した発話と、関係しそうな既存のグラフの一部を LLM に渡し、
  「ノードを足す・既存のノードに統合する・エッジを足す・状態を変える」という操作の列を返させる。
- 良い点: 1 回の入力が会議の長さに比例して増えない。ノードの ID が安定し、UI がちらつかない。
- 悪い点: LLM が既存のノードを見落とすと重複する → 会議後の整理で統合する。

#### Y. 毎回グラフ全体を作り直す
- 捨てた理由: 入力が会議の長さに比例して増え、会議全体の費用は長さの 2 乗に比例する。
  毎回 ID が変わるので、承認・却下の結果を引き継げない。

#### Z. 二段階（発話の分類 → 結び付け）
- 捨てた理由: LLM の呼び出しが倍になる。分類だけでは結び付けの精度は上がらない。
  X の 1 回の呼び出しの中で、発話ごとの発語内行為も返させる。

## 決定

**A・P・X を採る。** 以下が詳細。

### 1. データモデル

会議（セッション）ごとに 1 つのグラフ。同じ DB ファイルに表を足し、`discussion-graph` の接続だけが読み書きする。

```
graph_utterances  発話層（IAT の L-node）
  session_id, uid TEXT ("u1", "u2", … 取り込み順の連番)
  row_id INTEGER NOT NULL          -- transcript_segments.id（発話の同一性の鍵）
  track, speaker_label, start_ms NULL, end_ms NULL   -- 表示用。鍵には使わない
  extracted_rev INTEGER NULL       -- 抽出に入れたパッチの rev。NULL は未抽出
  PRIMARY KEY (session_id, uid), UNIQUE (session_id, row_id)

graph_nodes       命題層
  session_id, nid TEXT ("n1", …), kind TEXT, text TEXT,
  status TEXT  ('open' | 'resolved' | 'decided' | 'retracted' | 'merged')
  merged_into TEXT NULL
  confidence REAL (0..1)
  review TEXT ('auto' | 'approved' | 'rejected')
  created_rev INTEGER, updated_rev INTEGER
  PRIMARY KEY (session_id, nid)

graph_edges
  session_id, eid TEXT, src TEXT, dst TEXT, kind TEXT,
  confidence REAL, review TEXT, created_rev INTEGER
  PRIMARY KEY (session_id, eid), UNIQUE (session_id, src, dst, kind)

graph_grounds     根拠（ノード・エッジ ↔ 発話）
  session_id, target TEXT ("n12" / "e7"), uid TEXT,
  act TEXT NULL ('asserts' | 'asks' | 'challenges' | 'agrees' | 'retracts' | 'rephrases')
  PRIMARY KEY (session_id, target, uid)

graph_patches     適用したパッチの記録（追記のみ）
  session_id, rev INTEGER, source TEXT ('live' | 'flush' | 'consolidate' | 'review'),
  provider_model TEXT, ops_json TEXT, rejected_ops INTEGER, created_at
  PRIMARY KEY (session_id, rev)
```

- **発話の同一性は `transcript_segments.id` で決める**（レビュー M2）。イベントは「新しい確定行がある」という合図としてだけ使い、
  発話は常に DB の確定行から `is_final = 1 AND id > 最後に取り込んだ row_id` で取り込む。
  これで、時刻の無い STT、Self の再接続、`persist_pending_interim` の行、後からの再文字起こし（`is_retranscribed = 1`）も、
  同じ経路で 1 回ずつ取り込まれる。`Lagged` の特別扱いも要らない。
  発話の並びは取り込み順（`row_id` の順）とし、時刻では並べ替えない。ただし再文字起こしの行（会議の後に穴を埋めたもの）は
  会話の上で隣ではないので、その回は「参照用の直前の 3 発話」を付けない。
- 取り込みは、録音中は確定のイベント、会議の後は `stopped`、再文字起こしの完了、作り直しで行う（§3）。
- **v1 のノードの種類**: `issue`（問い）、`position`（案・立場）、`argument`（賛否の理由）、
  `decision`（決定）、`action`（宿題）。
- **v1 のエッジの種類**: `responds_to`（position → issue）、`supports` / `objects_to`（argument → position か argument）、
  `resolves`（decision → issue）、`assigns`（action → decision か issue）。
- 種類は TEXT で持ち、Rust の enum は `#[non_exhaustive]` にする。CRT の ADR で `cause` / `effect`、`causes` を
  足しても表を変えなくてよい。組み合わせは §2 の規則で検査する。
- **世代は持たない**（レビュー M3）。作り直しは「確認のうえで、そのセッションのグラフを消して作り直す」（§3）。
- **会議をまたぐ拡張（Q4）のため**、ID はセッション内で閉じ、表はすべて `session_id` を先頭に持つ。
  会議をまたぐリンクは将来、別の表で `(session_id, nid)` の組を指す。
- 移行の仕組みが無いので、v1 の後に列を変えるときは移行の仕組みを先に入れる（表を足すだけなら不要）。
- 本文（`graph_nodes.text`）は要約と同じく DB にだけ保存し、ログには出さない。

### 2. LLM への入出力と検証

summarize に、自由なプロンプトを渡して文字列を受ける汎用の呼び出しを足す:
`Summarizer::complete(&self, req: &CompletionRequest) -> Result<String, SummarizerError>`。
`CompletionRequest { system, user, model }` で、`SummarizeOptions::system_prompt` は使わない（どちらが勝つかの曖昧さを無くす）。
genai と CLI の両方のバックエンドで実装する。プロバイダとモデルは要約と同じ設定を使う（Q1）。
今アプリと Rhai に 2 か所ある「設定からバックエンドを組み立てる」処理は、summarize の関数 1 つにまとめ、グラフもそれを使う。

**入力の上限**: 1 回の入力（system＋user）は 24,000 文字まで（仮の値）。CLI のバックエンドが引数で渡すため、Windows の
コマンドラインの上限（UTF-16 で 32,767、パスと他の引数とエスケープを含む）に余裕を残す。CLI が `.cmd` のシムなら
cmd.exe の 8,191 文字が先に当たるので、S1 で両 CLI の実際の上限を Windows の実機で測り、r4 で値を決める
（足りなければ CLI にはプロンプトを標準入力で渡す経路を足す）。超える場合の分け方は各経路（§3）で決める。

入力（1 回の増分呼び出し）:
- システムプロンプト: スキーマ、操作の形、規則（根拠になる発話 ID の無い操作を出さない、
  既存のノードと同じ内容なら `merge` する、推測で決定を作らない）。
- **枠の配り方**: 入力の上限からシステムプロンプトの長さを引いた残りを、文脈・参照用の発話・新しい発話に配る。
  参照用の 3 発話は 1 件 200 文字に切り詰める。
- **文脈**: 状態が `open` の issue（新しい順）と、直近 K 回のパッチで触ったノード。文脈全体で入力の上限の 1/3 の文字数まで
  （超えたら古いものから落とす）。各ノードは `nid`、種類、本文、状態だけ。`review = rejected` と `status = merged` のノードは入れない。
- **新しい発話**: 未抽出（`extracted_rev IS NULL`）の発話を `uid` 順に、残りの文字数まで。各発話は `uid`、話者ラベル、本文。
  1 件で上限の 1/3 を超える発話は、先頭からその長さに切り詰めて入れる（先頭の 1 件が入らずに進まなくなるのを防ぐ）。
  直前の 3 発話も「参照用」として付ける（会話のつながりのため。参照用の発話も根拠にしてよい）。

出力: JSON 1 つ。

```json
{"ops": [
  {"op": "add_node", "tmp": "a", "kind": "issue", "text": "…", "confidence": 0.8,
   "grounds": [{"uid": "u41", "act": "asks"}]},
  {"op": "add_node", "tmp": "b", "kind": "position", "text": "…", "confidence": 0.6,
   "grounds": [{"uid": "u42", "act": "asserts"}]},
  {"op": "add_edge", "src": "b", "dst": "a", "kind": "responds_to", "confidence": 0.7, "grounds": [{"uid": "u42"}]},
  {"op": "merge", "from": "n9", "into": "n5", "grounds": [{"uid": "u43", "act": "rephrases"}]},
  {"op": "set_status", "nid": "n2", "status": "decided", "grounds": [{"uid": "u44"}]}
]}
```

- `tmp` はその応答の中だけの仮の名前で、適用のときに `nid` を振る。`src` / `dst` / `from` / `into` / `nid` は、
  同じ応答の `tmp` か既存の `nid` を指す。
- 応答は寛容に読む（前後の文やコードフェンスを除き、最初の JSON オブジェクトを取る）。読めなければ、
  エラーの種類を添えて 1 回だけ再試行し、それでも駄目ならその回は捨てる（発話は未抽出のまま残り、次の回に入る）。

**検証の規則**（コアの純粋関数。操作を順に検査し、不正な操作だけを落とす。落とした数は `graph_patches.rejected_ops` に残す）:

| 操作 | 落とす条件 | 連鎖 |
| --- | --- | --- |
| すべて | `grounds` が空、`grounds` の `uid` がその回の入力（新しい発話＋参照用）に無い、`act` が未知の値、`confidence` が 0..1 の外、未知の `op` | — |
| `add_node` | 未知の `kind`、本文が空、同じ応答で `tmp` が重複 | 落ちた `tmp` を指す後続の操作も落とす |
| `add_edge` | 端点が存在しない（落ちた `tmp`、無い `nid`、`merged`・`rejected` のノード）、種類の組み合わせが表に無い、自己ループ、同じ `(src, dst, kind)` が既にある | — |
| `merge` | `from` と `into` が同じ、どちらかが存在しない・`merged`、種類が違う、`from` が人の判断済み（`approved` / `rejected`）、`from` に人の判断済みのエッジが付いている、`into` が `rejected` | 適用: `from` の状態を `merged`・`merged_into = into` にし、`from` のエッジと根拠を `into` に付け替える。付け替えで自己ループや既存と同じ `(src, dst, kind)` になるエッジは捨てる。`merged_into = from` のノードも `into` に向け直す（連鎖を残さない） |
| `set_status` | ノードが存在しない・`merged`・`rejected`、人の判断済み、状態と種類の組み合わせが不正（`decided` は `issue` だけ、`resolved` は `issue` だけ、`retracted` は `position` と `argument` だけ）、`merged` を直接指定 | — |

検証は、同じ応答のそれまでの操作を順に適用した状態に対して行う（`merge A→B` の後の `merge A→C` は、A が既に `merged` なので落ちる）。
人の判断（`review = approved / rejected`）はノードもエッジも守る。却下したノードへのエッジの追加と `merge` の `into` も落とす。
適用した操作は 1 つのパッチ（`rev` を 1 つ進める）として、入力に入れた発話の `extracted_rev` の更新と合わせて 1 トランザクションで保存する。
LLM の呼び出しと JSON の解析はトランザクションの外で行う。

### 3. いつ動かすか

- **会議中（`source = live`）**: `graph_enabled` が有効なとき（既定で有効、Q5。録音の開始時に読む）。
  `transcription.{sid}.segment.finalized` を合図に、DB から新しい確定行を取り込み、次のどちらかで抽出する:
  未抽出の発話が 6 件以上たまった、または最後の抽出から 45 秒たって未抽出の発話が 1 件以上ある
  （どちらの値も設定で変えられる。初期値は spike S1 で見直す）。
  **同時に動く呼び出しは 1 つまで**。動いている間に来た発話は次の回にまとめる。
- **起動の条件**: 録音の開始時に、`graph_enabled` が有効で、Windows で、要約の設定からバックエンドを組み立てられる
  （キーがある・CLI が見つかる）ときだけ consumer を起動する。組み立てられなければ起動せず、設定画面に理由を出す
  （既定で有効にしたので、キーの無い環境で毎会議失敗し続けないため。`hint_enabled` を opt-in にした理由と同じ）。
- **会議の後（流し切り `source = flush` → 整理 `source = consolidate`）**: `session.{sid}.stopped` の後に 1 回だけ行う。
  `UtteranceEnded(SessionEnd)` では始めない（音声の保存と重なるため。N1）。順序（レビュー M4）:
  1. 動いている呼び出しが終わるのを待つ（時間の上限まで）。
  2. DB から確定行を最後まで取り込み、未抽出の発話が無くなるまで増分の抽出を続ける（会議中に止めていても行う）。
     連続 3 回失敗したら止めて、UI に「未抽出 N 件」と出す（作り直しか「続きを抽出」のボタンで再開する）。
     失敗で止めたときは 3 の整理を行わない（「続きを抽出」が流し切りを終えたら整理する）。
     アプリの再起動などで流し切りが途中で終わった場合も、未抽出の発話は DB に残るので、履歴画面の「未抽出 N 件」と「続きを抽出」から再開する。
  3. 整理を 1 回行う。入力は、`merged`・`rejected` でないノードとエッジの全部（本文、状態、確信度）と、各ノードの根拠の発話の本文。
     主な役目は重複の `merge`、抜けたエッジの追加、確信度の見直しで、操作の形と検証は §2 と同じ。
     上限の文字数を超えるなら、ノードを issue ごとのまとまりに分けて呼ぶ。そのとき、どの呼び出しにも issue の一覧（`nid` と本文だけ）を付け、
     issue 同士の重複も統合できるようにする。1 つの issue のまとまりだけで上限を超えるなら、根拠の発話の本文を 1 件 200 文字に切り詰め、
     それでも超えるなら根拠を 1 ノード 1 件に絞る。整理の失敗は 1 回で諦める（グラフは流し切りの状態のまま使える）。
  `graph_enabled` が無効でも、会議の後の流し切りと整理は走らせない（設定どおり何も送らない）。
- **会議の後の操作にも起動の条件を当てる**: 再文字起こしの後の抽出、「続きを抽出」、作り直しは、その時点で `graph_enabled` が有効で、
  バックエンドを組み立てられるときだけ行う（グラフを無効にした人の文字起こしを送らない）。
- **再文字起こしの後**: 履歴画面からの再文字起こしが終わったら、そのセッションの取り込みと増分の抽出を 1 回走らせる
  （承認・却下は消さない。整理はしない）。
- **作り直し**: 履歴画面の「グラフを作り直す」ボタンから。録音中のセッションには出さない。承認・却下の結果も消えることを確認の画面で示し、
  了承されたら、そのセッションのグラフの表の行を 1 つの短いトランザクションで消して、会議の後と同じ流し切り→整理を行う。
- **セッションごとに実行者は 1 つ**（レビュー N3）: 会議中の抽出、会議の後の処理、再文字起こしの後の抽出、作り直しは、
  アプリの `graph_consumer` が持つセッションごとの実行者（`HashMap<SessionId, …>`）だけが行う。走っている間の作り直しは、
  走っているものを止めて終わるのを待ってから消す。作り直しの 2 回押しは、走っている作り直しが終わるまでボタンを無効にする。
  `rev` と `uid` は書きのトランザクションの中で `MAX(...) + 1` で振る。

### 4. イベント

- 新しい subject `graph.{sid}.updated`。ペイロードは `EventEnvelope` に包んだ `{rev, source, added, merged, rejected}`
  （件数だけで、本文を入れない）。受け手は DB から読み直す。subject 名の関数は `discussion-graph` crate に置く。
- v1 の UI はポーリングで `rev` を読むので購読者が居ない。`publish` の `NoSubscribers` は、`publish_session_stopped`
  （`apps/desktop/src/recording.rs:51-55`）と同じく無視し、ログにも出さない。ヒントや図の ADR で購読者を足す。

### 5. 表示と承認・却下（v1）

- **Windows だけ**。macOS ではグラフのタブとボタンを出さない（文字起こしが無いため）。
- **アウトライン表示**: ライブ画面と履歴画面にタブを 1 つ足す。issue ごとに、その下に position、
  その下に argument（賛成・反対の印）を字下げで並べ、decision と action を issue の下に付ける。
  `rev` が変わったら読み直す（今の 250ms ポーリングに乗せる。読むのはグラフ側の接続で、`rev` が変わったときだけ全体を読む）。
  確信度がしきい値（初期値 0.6）より低いものは薄く表示する。`merged`・`rejected` は出さない。
- **根拠の表示**: ノードを選ぶと、根拠の発話（話者ラベル、時刻があれば時刻、本文）を出す。
- **承認・却下（Q3）**: 会議の後の整理が終わったら「確認」の一覧を出す。対象は `review = auto` で確信度がしきい値より低いノードとエッジ。
  承認・却下の結果は `review` 列と、`source = review` のパッチとして残す。却下したものは表示から外すが消さない。
  人の判断は、その後の整理でも変えられない（§2 の規則）。
- 図（マインドマップ、CRT など）は出さない（別 ADR）。

### 6. 穴の検出（v1 は関数だけ）

`discussion-graph` に、グラフから次を数え上げる純粋関数を置く。v1 は UI に「未回答の問い」の件数を出すだけで、
ヒントへの接続は別 ADR。
- 答え（`responds_to`）の無い issue
- 賛否（`supports` / `objects_to`）が片方しか無い position
- `decision` の無いまま会議が終わった issue（会議の後の流し切りが終わってから数える）
- `action` の付いていない decision

### 7. 失敗したときの挙動

- グラフの処理は、録音・セグメントの保存・文字起こし・要約から独立したタスクで動き、録音側の `SessionStore` のロックを取らない（§「保存の接続」P）。
  LLM の失敗、遅延、パースの失敗、DB への書き込みの失敗は、その回を捨てて記録するだけで、他を止めない。発話は未抽出のまま残る。
- グラフ側の DB の処理では、ロックの中（トランザクションの中）で `unwrap` / `expect` / 添字アクセスをしない。エラーは `Result` で返す。
- LLM の呼び出しには時間の上限（初期値 60 秒）を付ける。上限を超えたら捨てる。
- 会議中に連続で 3 回失敗したら、その会議中は抽出を止め、UI に「グラフの更新を止めた」と出す。
  発話の取り込みは続け、会議の後の流し切りで抽出する（§3）。
- グラフ側の書き込みは `BEGIN IMMEDIATE` で始め、1 トランザクションに 1 パッチの適用か 1 回の取り込みだけを入れる（録音側を
  `busy_timeout` 以上待たせない。§「保存の接続」P）。グラフ側が `busy_timeout`（5 秒）を超えて待たされたら、その回を捨てる。

### 8. ログ（`docs/logging-policy.md` に従う）

出してよいもの: セッション ID、`rev`、`source`、操作の件数、落とした件数（規則ごと）、所要時間、トークン数（取れるなら）、
エラーの種類。出さないもの: 発話やノードの本文、プロンプト、LLM の応答（解析に失敗したときも本文は出さない）。
`SummarizerError` は `Display` をそのまま出さない（`CliSummarizeError::NonZeroExit` は CLI の stderr を丸ごと持ち、本文を含みうる）。
種類（timeout / not_found / non_zero_exit / parse / provider）に写してから出す。

## 計測（spike）

まだ取っていない。下の見積もりは仮の値で、実装の分割の 1 番目（S1）で測ってから r4 で置き換える。

見積もり *(未確認)*: 1 時間の日本語の会議で確定発話が約 600、本文が約 2 万トークン。
45 秒ごとの増分で約 80 回、1 回の入力はシステムプロンプト 1.5k＋文脈 2k＋新しい発話 1k ≒ 4.5k トークン、
出力は約 0.5k。合計は入力約 36 万、出力約 4 万トークン（会議の後の要約 1 回の約 15 倍）。

**S1 で測るもの**（合成の日本語の会議の文字起こし 3 本。実在の会議は使わない）:
- 1 回あたりの入出力トークン、所要時間の p50 / p95（要約で使っているモデル 2 つ以上、CLI のバックエンドを含む）
- JSON として読めた割合、検証で落ちた操作の割合（規則ごと）
- 手で作った正解の IBIS に対する、ノードの再現率・適合率、エッジの F1
- 窓の大きさ（6 件／45 秒）と文脈の上限 N を変えたときの、重複の数と費用

**go / no-go**: 所要時間の p95 が 45 秒を超えるなら窓を広げる。読めた割合が 90% を下回るなら、
プロバイダの構造化出力（JSON schema 指定）を使う経路を足すかを r4 で決める。

## 影響

- **公開 API**（core、v0.3.0 の minor を上げる）:
  - 新 crate `discussion-graph`。依存は `session-store`（2 本目の接続を開く関数と、`transcript_segments` の読み出しのため）、
    `transcript-event`（`EventEnvelope`）、`recorder-domain`（`SessionId`、`TrackKind`）。
  - session-store が、今は private な `open_with_pragmas` を公開する（または `open_secondary_connection` を足す）。
  - 話者ラベル（「自分 (話者2)」）を作る関数を `recorder-domain` に 1 つ置く（今は `app-service`・アプリ・`rhai-engine` に写しがある）。
    `session-store` と `summarize` は `discussion-graph` に依存しない。アプリが `summarize` の結果を `discussion-graph` に渡す。
  - `Summarizer` に `complete` を足す（既定の実装は置かない。実装はコアの 2 つのバックエンドだけで、アプリとテストに他の実装は無い）。
  - summarize に、設定からバックエンドを組み立てる関数を足す（アプリと Rhai の写しを置き換える）。
- **イベント**: `graph.{sid}.updated` を足す。既存の subject は変えない。
- **設定**: アプリに `graph_enabled`（既定は有効、Q5）、`graph_batch_utterances`（6）、`graph_batch_seconds`（45）、
  `graph_review_threshold`（0.6）。
- **保存形式**: 同じ DB ファイルに 5 表を足す（`CREATE TABLE IF NOT EXISTS`。既存の表は変えない）。
- **プライバシー**: 送り先は要約と同じプロバイダ（Q1）。送る内容は文字起こしの本文と、グラフのノードの本文。
  既定で有効なので、会議中に何度も送る（Q5）。保存はローカルの DB だけ。今の session-store にはセッションを消す API が無く、
  文字起こしと同じく残り続ける。セッションの削除を足すときは、グラフの表の行も消す（`discussion-graph` のセッションごとの削除を呼ぶ）。
- **失敗**: 録音を止めない（§7）。
- **プラットフォーム**: v1 のグラフは Windows だけ（macOS は文字起こしを保存しないため）。
- **未確認の前提**:
  - CLI のバックエンド（claude / codex）が、45 秒ごとの呼び出しに耐える速さで返すか *(未確認)*
  - 話者ラベルが「自分」「相手」の 2 つしかない会議で、誰の立場かを区別できるか（多人数の会議の相手側） *(未確認)*

## 実装の分割

| 順 | repo | タスク | 依存 | 受け入れ条件 |
| --- | --- | --- | --- | --- |
| 1 | core（`spike/discussion-graph`、マージしない） | S1: プロンプトと操作の形を試し、§「計測」の数値を取る。Windows の実機で両 CLI の引数の上限を測る | — | 数値と再現手順を ADR r4 に書く。合成データと正解はタスク 2 で main に入れる |
| 2 | core | `discussion-graph` crate の純粋部分: 型、操作、§2 の検証の規則と適用（メモリ上）、穴の検出、プロンプトの組み立て、応答の解析、subject 名。S1 の合成データと正解を `testdata/` に入れる | 1 | §2 の規則の表の 1 行ごとに単体テスト。変異（規則を 1 つ外す）で落ちるテストを確かめる |
| 3 | core | session-store の接続を開く関数の公開、話者ラベルの関数の共通化。`discussion-graph` の保存: 2 本目の接続（`BEGIN IMMEDIATE`）、5 表、確定行の差分の取り込み（`is_final = 1 AND id > ?`）、パッチの適用を 1 トランザクションで、セッションごとの削除。評価用の ignored テスト（正解に対する再現率・適合率・F1） | 2 | session-store で書いた確定行（時刻の無い行、再文字起こしの行を含む）を 1 回ずつ取り込むテスト、トランザクションの途中で失敗したら何も残らないテスト、グラフ側でエラーが起きても SessionStore が使えるテスト、グラフ側が書きのトランザクションを開いている間に `SessionStore::register_segment` が待ってから成功するテスト。トランザクションの中に `unwrap` / `expect` が無いこと |
| 4 | core | summarize: `CompletionRequest` と `Summarizer::complete` を genai と CLI で実装。設定からバックエンドを組み立てる関数。`SummarizerError` の種類への写し | — | 両バックエンドのテスト（CLI はダミーのコマンドで）、入力の上限を超えたらエラーになるテスト。2〜4 をマージしてからタグ v0.3.0 |
| 5 | app | `graph_consumer.rs`: 起動の条件、会議中の増分、会議の後の流し切りと整理（`stopped` でだけ開始）、再文字起こしの後の抽出、作り直し、セッションごとの実行者 1 つ、失敗時の挙動、設定 4 つ、`graph.{sid}.updated`（`NoSubscribers` は無視）。アプリと Rhai のバックエンドの組み立てを 4 の関数に置き換える | 4 のタグ | LLM をダミーにして: 窓の区切り、同時 1 つ、3 回失敗で停止、終了直前の 1〜5 件が会議の後に抽出される、停止後の発話が会議の後に抽出される、LLM が常に失敗しても流し切りが有限回で終わる、上限を超える 1 件の発話の後続も抽出される、作り直しと流し切りが同時に走らない、`SessionEnd` では流し切りが始まらない、バックエンドが組み立てられないときと macOS では起動しない、再文字起こしの後に新しい行だけが抽出される、`graph_enabled` が無効なら再文字起こしの後も作り直しも送らない、失敗で止めた流し切りでは整理しない、ログに本文が出ない。録音の経路に触れないこと |
| 6 | app | アウトライン表示と根拠の表示（ライブ・履歴、Windows だけ）、「未回答の問い」と「未抽出 N 件」の件数、「続きを抽出」と作り直しのボタンと確認 | 5 | 表示用のデータ変換（グラフ → 字下げの行）の単体テスト。画面は owner が実機で確認 |
| 7 | app | 会議後の確認の一覧（承認・却下） | 5 | 承認・却下が `review` とパッチに残るテスト、その後の整理の操作が弾かれるテスト |

## 改訂履歴

| 版 | 内容 |
| --- | --- |
| r1 | 初版 |
| r2 | round 1 の指摘を反映。M1: グラフ専用の 2 本目の接続（選択肢 P/Q）とロック中の規則。M2: 発話の鍵を `transcript_segments.id` にし、DB の確定行から差分で取り込む。M3: 世代をやめ、作り直しは確認のうえ消して作り直す。M4: `extracted_rev` と会議後の流し切り。m1: v1 は Windows だけ。m2: 検証の規則の表、整理の分け方。m3: 背景の 2 文を修正。m4: `CompletionRequest`、入力の上限、組み立ての共通化。m5: エラーの種類への写し。m6: `NoSubscribers` を無視。m7: Q5 を owner の決定（既定で有効）に、合成データの置き場所、crate の依存の向き |
| r3 | round 2 の指摘を反映。N1: 2 本目の接続の書き込みは `BEGIN IMMEDIATE` で 1 パッチずつ、会議後の処理は `stopped` でだけ開始、録音側が待ってから成功するテスト。N2: 流し切りの連続失敗の上限、文脈の文字数の枠、長い発話の切り詰め。N3: セッションごとに実行者 1 つ、録音中は作り直し不可。n1: 規則の表（付け替えの自己ループ・人の判断済みのエッジ・同じ応答の中の連鎖・逐次の検証・`act`）。n2: 上限は S1 で実測、1 issue が上限を超える場合。n3: 再文字起こしの後の抽出と並び。n4: 起動の条件（バックエンドが無い・macOS）。n5: 公開する API |
| r4 | round 3 の Minor を反映して Accepted。R1: 会議の後の操作にも起動の条件。R2: §2 の例を規則の表に合わせる。R3: 枠の配り方（システムプロンプトと参照用の発話を含める）。R4: 「未抽出 N 件」と「続きを抽出」をタスク 6 に、失敗時は整理しない、再起動後の再開、再文字起こしのテスト |
