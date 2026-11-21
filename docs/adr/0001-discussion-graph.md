# ADR 0001: 議論グラフ（discussion graph）の持ち方と作り方

- Status: Proposed
- 日付: 2026-11-21
- 関連: meetingram-core `docs/decouple-summary-transcription.md`、`docs/rhai-plugin-summary.md`、`docs/logging-policy.md`

## 背景

meetingram の目的は、会議の議論を図（マインドマップ、TOC の CRT・対立解消図）にし、
次に何を聞くべきかを支援すること（`AGENTS.md:3`）。今のコードにあるのは、文字起こしの表示、
会議終了時の要約、Rhai のヒントだけで、議論の構造を持つものは無い。

- 文字起こしは `TranscriptEvent`（meetingram-core `crates/transcript-event/src/lib.rs:29-67`）で
  `transcription.{sid}.segment.finalized` などに流れる。`SegmentData` には `segment_id`、`text`、
  `speaker_label`（「自分」「相手」＋話者番号、`crates/app-service/src/live_transcription.rs:103`）、
  `track`、`start_ms` / `end_ms` がある。単語単位の時刻と確信度は載らない。
- `segment_id` は、ライブでは `{sid}:{track}:{counter}`（`lib.rs:155`）、DB から組み直すと
  `{sid}:{track}:{start}-{end}`（`lib.rs:165`）で、**同じ発話でも経路によって ID が違う**。
  `transcript_segments` 表（`crates/session-store/src/schema.rs:98-110`）には `segment_id` 列が無い。
- ライブの文字起こしは Windows でしか有効にしていない（`apps/desktop/Cargo.toml:61`）。
  macOS では会議中のイベントが来ない。
- Local Broker（`crates/local-broker/src/lib.rs`）はプロセス内の `broadcast`（subject ごとに 256 件）で、
  保持しない。遅れた購読者は `Lagged` を受け、SessionStore から読み直す約束になっている
  （`docs/decouple-summary-transcription.md` §6）。
- 要約は `Summarizer::summarize(&[TranscriptTurn], &SummarizeOptions) -> String`
  （`crates/summarize/src/lib.rs:158`）で、自由文しか返さない。アプリの `summary_consumer.rs:56` が
  会議終了時に 1 回だけ呼ぶ。プロバイダとモデルはキーリングに保存した設定から選ぶ（`summary_consumer.rs:229-271`）。
- Rhai の dispatcher（`crates/rhai-engine/src/dispatcher.rs:45-56`）に外部への HTTP 送信は無い。
- session-store に移行の仕組みは無く、`SCHEMA_SQL` の `CREATE TABLE IF NOT EXISTS` に足していく（`schema.rs:5-7`）。
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
  - 抽出の仕方（会議中の増分、会議後の整理、作り直し）と LLM への入出力の形
  - どのプロセス・crate で動かすか、保存先、イベント
  - 会議後の承認・却下の UI と、v1 の表示
  - 失敗したときの挙動、ログに出すもの
- 決めないこと（別 ADR）:
  - 図への射影と描画（マインドマップ、CRT、対立解消図、svg-diagram の使い方）。v1 の表示はアウトラインだけ
  - グラフの穴から質問を作ってヒントに出すこと（v1 は穴の検出関数を用意するだけ）
  - 因果（`causes`）の抽出。CRT の ADR で決める（v1 のスキーマは種類を足せるようにしておく）
  - 会議をまたいだ蓄積（Q4）
  - ノードやエッジの自由な編集（追加・付け替え）
  - 音声の再生による根拠の確認

## owner の決定

| # | 問い | 決定 | 日付 |
| --- | --- | --- | --- |
| Q1 | 文字起こしを送る LLM | 要約と同じ設定（プロバイダ・モデル）を使う。新しい送信先は増やさない | 2026-11-21 |
| Q2 | いつ作るか | 会議中に増分で作り、会議の後に全体を見直して整理する | 2026-11-21 |
| Q3 | v1 の人による確認 | 会議の後に、確信度の低いものを承認・却下するだけ。自由な編集は後回し | 2026-11-21 |
| Q4 | 会議をまたいだ蓄積 | v1 の範囲外。別 ADR。保存形式は会議をまたいで広げられる形にしておく | 2026-11-21 |
| Q5 | 既定で有効にするか | 推奨: 既定は無効（`graph_enabled`、`hint_enabled` と同じ扱い）。会議中に文字起こしを何度も LLM に送るため、要約（会議の後に 1 回）より送る回数が多い | |

## 選択肢

### 動かす場所

#### A. Rust: コアの新 crate＋アプリの consumer（採用）
- 仕組み: 純粋なロジック（型、パッチの検証と適用、穴の検出、プロンプトの組み立て、応答の解析）を
  コアの新しい crate `discussion-graph` に置く。保存は session-store、LLM の呼び出しは summarize。
  アプリの `graph_consumer.rs` が `summary_consumer.rs` と同じ形でイベントを受けて動かす。
- 良い点: 既存の仕組み（Broker、SessionStore、Summarizer、キーリングの設定）にそのまま乗る。
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
- 捨てた理由: dispatcher に HTTP 送信が無い。デスクトップアプリに Node の実行環境を同梱することになる。
  プロセス間の Broker（`decouple-summary-transcription.md` §5 の phase 2）もまだ無い。
  描画に TS を使うかどうかは、図の ADR で別に決める（グラフは保存形式で受け渡せる）。

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

**A と X を採る。** 以下が詳細。

### 1. データモデル

会議（セッション）ごとに 1 つのグラフ。保存は session-store の同じ DB に表を足す。

```
graph_utterances  発話層（IAT の L-node）
  session_id, uid TEXT ("u1", "u2", … セッション内で確定順の連番)
  track, speaker_label, start_ms, end_ms
  live_segment_id TEXT NULL     -- ライブで受けた segment_id（参考）
  transcript_row_id INTEGER NULL -- transcript_segments.id（分かれば）
  PRIMARY KEY (session_id, uid)

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
  PRIMARY KEY (session_id, eid)

graph_grounds     根拠（ノード・エッジ ↔ 発話）
  session_id, target TEXT ("n12" / "e7"), uid TEXT,
  act TEXT NULL ('asserts' | 'asks' | 'challenges' | 'agrees' | 'retracts' | 'rephrases')

graph_patches     適用したパッチの記録（追記のみ）
  session_id, rev INTEGER, source TEXT ('live' | 'consolidate' | 'rebuild' | 'review'),
  provider_model TEXT, ops_json TEXT, rejected_ops INTEGER, created_at
  PRIMARY KEY (session_id, rev)
```

- **v1 のノードの種類**: `issue`（問い）、`position`（案・立場）、`argument`（賛否の理由）、
  `decision`（決定）、`action`（宿題）。
- **v1 のエッジの種類**: `responds_to`（position → issue）、`supports` / `objects_to`（argument → position か argument）、
  `resolves`（decision → issue）、`assigns`（action → decision か issue）。
- 種類は TEXT で持ち、Rust の enum は `#[non_exhaustive]` にする。CRT の ADR で `cause` / `effect`、`causes` を
  足しても表を変えなくてよい。組み合わせの検査（例: `supports` の始点は `argument`）は検証関数で行う。
- **発話の ID は経路に依存させない**: グラフは自前の連番 `uid` を振り、`track`・`start_ms`・`end_ms` を持つ。
  ライブの `segment_id` は参考として残すだけ。DB から作り直すときは `(track, start_ms, end_ms)` で
  `transcript_segments` の確定行と結び付ける。`start_ms` が無い確定行は `transcript_row_id` だけで結ぶ。
- **会議をまたぐ拡張（Q4）のため**、ID はセッション内で閉じ、表はすべて `session_id` を先頭に持つ。
  会議をまたぐリンクは将来、別の表で `(session_id, nid)` の組を指す。
- 本文（`graph_nodes.text`）は要約と同じく DB にだけ保存し、ログには出さない。

### 2. LLM への入出力

summarize に、自由なプロンプトを渡して文字列を受ける汎用の呼び出しを足す
（`Summarizer` に `complete(system, user, &SummarizeOptions) -> Result<String, _>` を足す。
genai と CLI の両方のバックエンドで実装する）。プロバイダとモデルは要約と同じ設定を使う（Q1）。

入力（1 回の増分呼び出し）:
- システムプロンプト: スキーマ、操作の形、規則（根拠になる発話 ID の無い操作を出さない、
  既存のノードと同じ内容なら `merge` する、推測で決定を作らない）。
- **文脈**: 状態が `open` の issue 全部と、直近 K 回の更新で触ったノード（上限 N 件。超えたら古いものから落とす）。
  各ノードは `nid`、種類、本文、状態だけ。
- **新しい発話**: 前回の呼び出しより後に確定した発話。各発話は `uid`、話者ラベル、本文。
  直前の 3 発話も「参照用」として付ける（会話のつながりのため）。

出力: JSON 1 つ。

```json
{"ops": [
  {"op": "add_node", "tmp": "a", "kind": "issue", "text": "…", "confidence": 0.8,
   "grounds": [{"uid": "u41", "act": "asks"}]},
  {"op": "add_edge", "src": "a", "dst": "n3", "kind": "responds_to", "confidence": 0.7, "grounds": [{"uid": "u41"}]},
  {"op": "merge", "from": "b", "into": "n5", "grounds": [{"uid": "u42", "act": "rephrases"}]},
  {"op": "set_status", "nid": "n2", "status": "decided", "grounds": [{"uid": "u43"}]}
]}
```

- 応答は寛容に読む（前後の文やコードフェンスを除き、最初の JSON オブジェクトを取る）。読めなければ、
  エラーの要点を添えて 1 回だけ再試行し、それでも駄目ならその回は捨てる（次の回の新しい発話に含め直す）。
- **検証してから適用する**（コアの純粋関数）。1 つずつ検査し、不正な操作だけを落とす:
  根拠が無い、根拠の `uid` が入力に無い、指す `nid` が存在しない、種類の組み合わせが不正、確信度が 0..1 の外、
  `review = approved/rejected` のノードを変えようとしている。落とした数は `graph_patches.rejected_ops` に残す。
- 適用した操作は 1 つのパッチ（`rev` を 1 つ進める）として、1 トランザクションで保存する。

### 3. いつ動かすか

- **会議中（`source = live`）**: `graph_enabled` が有効なときだけ。`transcription.{sid}.segment.finalized` を受けて
  発話を `graph_utterances` に足し、次のどちらかで抽出する:
  新しい確定発話が 6 件以上たまった、または最後の抽出から 45 秒たって新しい確定発話が 1 件以上ある
  （どちらの値も設定で変えられる。初期値は spike S1 で見直す）。
  **同時に動く呼び出しは 1 つまで**。動いている間に来た発話は次の回にまとめる。
- **会議の後（`source = consolidate`）**: `session.{sid}.stopped` か `UtteranceEnded(SessionEnd)` の後に 1 回。
  入力はグラフ全体（ノード、エッジ、各ノードの根拠の発話の本文）で、出力は同じ操作の形。
  主な役目は重複の `merge`、抜けたエッジの追加、確信度の見直し。グラフが大きくて上限を超えるなら、
  issue ごとに部分グラフに分けて呼ぶ。
- **作り直し（`source = rebuild`）**: 会議中のグラフが無いとき（macOS、`graph_enabled` を会議中に切っていた、
  会議中の抽出が全部失敗した）と、履歴画面の「グラフを作り直す」ボタンから。保存済みの確定発話を、
  会議中と同じ窓の区切りで先頭から流し、増分の抽出を続けて実行し、最後に整理を 1 回行う。
  作り直しは既存のグラフを消さず、新しい世代として作る（承認・却下の結果は引き継がない。UI で確認してから置き換える）。

### 4. イベント

- 新しい subject `graph.{sid}.updated`。ペイロードは `EventEnvelope` に包んだ `{rev, source, added, merged, rejected}`
  （件数だけで、本文を入れない）。受け手は SessionStore から読み直す。保持しない Broker の性質（`Lagged`）に合わせる。
- subject 名の関数は `discussion-graph` crate に置く。

### 5. 表示と承認・却下（v1）

- **アウトライン表示**: ライブ画面と履歴画面にタブを 1 つ足す。issue ごとに、その下に position、
  その下に argument（賛成・反対の印）を字下げで並べ、decision と action を issue の下に付ける。
  `rev` が変わったら読み直す（今の 250ms ポーリングに乗せる）。確信度がしきい値（初期値 0.6）より低いものは薄く表示する。
- **根拠の表示**: ノードを選ぶと、根拠の発話（話者ラベル、時刻、本文）を出す。
- **承認・却下（Q3）**: 会議の後に「確認」の一覧を出す。対象は `review = auto` で確信度がしきい値より低いノードとエッジ。
  承認・却下の結果は `review` 列と、`source = review` のパッチとして残す。却下したものは表示から外すが消さない。
  整理（consolidate）の後に表示する。人の判断は整理で上書きしない（§2 の検証で弾く）。
- 図（マインドマップ、CRT など）は出さない（別 ADR）。

### 6. 穴の検出（v1 は関数だけ）

`discussion-graph` に、グラフから次を数え上げる純粋関数を置く。v1 は UI に「未回答の問い」の件数を出すだけで、
ヒントへの接続は別 ADR。
- 答え（`responds_to`）の無い issue
- 賛否（`supports` / `objects_to`）が片方しか無い position
- `decision` の無いまま会議が終わった issue
- `action` の付いていない decision

### 7. 失敗したときの挙動

- グラフの処理は、録音・セグメントの保存・文字起こし・要約から独立したタスクで動く。
  LLM の失敗、遅延、パースの失敗、DB への書き込みの失敗は、その回を捨てて記録するだけで、他を止めない。
- LLM の呼び出しには時間の上限（初期値 60 秒）を付ける。上限を超えたら捨てる。
- 連続で 3 回失敗したら、その会議中は抽出を止め、UI に「グラフの更新を止めた」と出す。会議の後の作り直しはできる。
- `Lagged` を受けたら、`transcript_segments` から確定行を読み直し、`(track, start_ms, end_ms)` で
  まだ `graph_utterances` に無いものを足す。

### 8. ログ（`docs/logging-policy.md` に従う）

出してよいもの: セッション ID、`rev`、`source`、操作の件数、落とした件数、所要時間、トークン数（取れるなら）、
エラーの種類。出さないもの: 発話やノードの本文、プロンプト、LLM の応答（解析に失敗したときも本文は出さない）。

## 計測（spike）

まだ取っていない。下の見積もりは仮の値で、実装の分割の 1 番目（S1）で測ってから r2 で置き換える。

見積もり *(未確認)*: 1 時間の日本語の会議で確定発話が約 600、本文が約 2 万トークン。
45 秒ごとの増分で約 80 回、1 回の入力はシステムプロンプト 1.5k＋文脈 2k＋新しい発話 1k ≒ 4.5k トークン、
出力は約 0.5k。合計は入力約 36 万、出力約 4 万トークン（会議の後の要約 1 回の約 15 倍）。

**S1 で測るもの**（合成の日本語の会議の文字起こし 3 本。実在の会議は使わない）:
- 1 回あたりの入出力トークン、所要時間の p50 / p95（要約で使っているモデル 2 つ以上）
- JSON として読めた割合、検証で落ちた操作の割合
- 手で作った正解の IBIS に対する、ノードの再現率・適合率、エッジの F1
- 窓の大きさ（6 件／45 秒）と文脈の上限 N を変えたときの、重複の数と費用

**go / no-go**: 所要時間の p95 が 45 秒を超えるなら窓を広げる。読めた割合が 90% を下回るなら、
プロバイダの構造化出力（JSON schema 指定）を使う経路を足すかを r2 で決める。

## 影響

- **公開 API**（core）: 新 crate `discussion-graph`。`Summarizer` に `complete` を足す（トレイトの変更なので、
  既存の実装は既定の実装が無いと壊れる → 既定の実装は置かず、コアの 2 つのバックエンドで実装する。
  0.x の minor を上げる: v0.3.0）。
- **イベント**: `graph.{sid}.updated` を足す。既存の subject は変えない。
- **設定**: アプリに `graph_enabled`（既定は無効、Q5）、`graph_batch_utterances`（6）、`graph_batch_seconds`（45）、
  `graph_review_threshold`（0.6）。
- **保存形式**: session-store に 5 表を足す（`CREATE TABLE IF NOT EXISTS`。既存の表は変えない）。
  移行の仕組みが無いので、v1 の後に列を変えるときは移行の仕組みを先に入れる。
- **プライバシー**: 送り先は要約と同じプロバイダ（Q1）。送る内容は文字起こしの本文と、グラフのノードの本文。
  会議中に何度も送る（Q5 で既定は無効）。保存はローカルの DB だけ。保存期間はセッションの削除に従う。
- **失敗**: 録音を止めない（§7）。
- **未確認の前提**:
  - CLI のバックエンド（claude / codex）が、45 秒ごとの呼び出しに耐える速さで返すか *(未確認)*
  - 確定発話の `start_ms` がほぼ常に入っているか（`(track, start_ms, end_ms)` での結び付けが成り立つか） *(未確認)*
  - 話者ラベルが「自分」「相手」の 2 つしかない会議で、誰の立場かを区別できるか（多人数の会議の相手側） *(未確認)*

## 実装の分割

| 順 | repo | タスク | 依存 | 受け入れ条件 |
| --- | --- | --- | --- | --- |
| 1 | core（`spike/discussion-graph`、マージしない） | S1: プロンプトと操作の形を試し、§「計測」の数値を取る。合成の会議 3 本と正解の IBIS も作る | — | 数値と再現手順を ADR r2 に書く。合成データと正解は 3 の評価に使う |
| 2 | core | `discussion-graph` crate: 型、操作、検証、適用（メモリ上）、穴の検出、プロンプトの組み立て、応答の解析、subject 名 | 1 | 検証の規則ごとに単体テスト。変異（規則を 1 つ外す）で落ちるテストを確かめる |
| 3 | core | session-store に 5 表と読み書き（パッチの適用を 1 トランザクションで）。`discussion-graph` に評価用の ignored テスト（1 の正解に対する再現率・適合率・F1） | 2 | 往復テスト、トランザクションの途中で失敗したら何も残らないテスト |
| 4 | core | summarize: `Summarizer::complete` を genai と CLI で実装 | — | 両バックエンドのテスト（CLI はダミーのコマンドで）。タグ v0.3.0（2〜4 をマージしてから） |
| 5 | app | `graph_consumer.rs`: 会議中の増分、会議後の整理、作り直し、失敗時の挙動、設定 4 つ、`graph.{sid}.updated` | 4 のタグ | 窓の区切り・同時 1 つ・3 回失敗で停止・`Lagged` の読み直しのテスト（LLM はダミー）。録音に触れないこと |
| 6 | app | アウトライン表示と根拠の表示（ライブ・履歴）、「未回答の問い」の件数、作り直しのボタン | 5 | 表示用のデータ変換（グラフ → 字下げの行）の単体テスト。画面は owner が実機で確認 |
| 7 | app | 会議後の確認の一覧（承認・却下）、整理が人の判断を上書きしないこと | 5 | 承認・却下が `review` とパッチに残るテスト、整理の操作が弾かれるテスト |

## 改訂履歴

| 版 | 内容 |
| --- | --- |
| r1 | 初版 |
