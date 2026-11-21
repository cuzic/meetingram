# ADR 0001 レビュー（round 1）

対象: cuzic/meetingram docs/adr-0001-discussion-graph @ 2d3ee4aa29709b9f165492e02823a41e615b5990
（照合したコード: meetingram-core main @ 25e2f8d、アプリはこのブランチ）

## 結論（先に）
converged: **no**。方向（コアの純粋ロジック＋アプリの consumer、差分パッチ、会議後の確認）は妥当で、背景の行番号の引用もほぼ正しい。
ただし (1) 「録音から独立したタスク」の前提が、録音と共有している単一の `Mutex<Connection>` で崩れている、
(2) 発話の結び付け `(track, start_ms, end_ms)` が OpenAI の STT と Self トラックの再接続で成り立たない、
(3) 作り直しの「新しい世代」が表の定義では実装できない、(4) 会議の終わりの発話が抽出されずに残る、の 4 点を r2 で直す必要がある。

## 前ラウンドの解消状況
初回のため無し。

## Findings

### M1（Major）: グラフの DB 読み書きが録音と同じロックを取るのに、§7 は「独立したタスクなので録音を止めない」としている
- 根拠: アプリは `SessionStore` を 1 つだけ開き（`apps/desktop/src/main.rs:96`）、録音・文字起こし・UI で共有する。
  `SessionStore` は `conn: Mutex<Connection>`（std の Mutex、core `crates/session-store/src/store.rs:87`）で、全メソッドが `self.conn.lock().unwrap()`。
  録音中の音声セグメントの登録 `store.register_segment`（`crates/segment-store/src/segment_writer.rs:108`）と、
  文字起こしの保存 `insert_transcript_segment`（`store.rs:571-591`、`live_transcription.rs:1441,1451`）が同じロックを取る。
  さらに `run_live_transcription` は capture と `tokio::join!` されている（`crates/app-service/src/windows_session.rs:123`）。
  - 長く持つと: §7 の `Lagged` 復旧と §3 の作り直しは `transcript_segments` を読み直すが、既存の `list_transcript_segments`（`store.rs:594-628`）は
    interim 行も含めて全件を読み、その間ロックを持ち続ける。1 時間の会議では interim 行が大量にあり、録音側の登録と UI（`ui.rs:562,567` が 250ms ごとに同じロック）が待たされる。
  - panic すると: グラフの保存コードがロックを持ったまま panic すると Mutex が poison され、以後の `lock().unwrap()` がすべて panic する。
    `insert_transcript_segment` の panic は `join!` で capture 側も巻き込み、録音が失われる（Blocker 級の帰結）。
- 修正案: §7 に「グラフの読み書きは (a) 確定行だけ・`id` より後だけを読む新しい API（`is_final = 1 AND id > ?`）を使う、
  (b) 1 回のロックで行うのは 1 トランザクションだけで LLM 呼び出しや JSON 解析をロックの中でしない、
  (c) ロックの中で panic しうるコード（`unwrap`/`expect`/添字）を置かない」を明記し、タスク 3 の受け入れ条件に入れる。
  WAL なので、グラフ専用に 2 本目の `Connection` を開く案（poison が録音側に波及しない）も r2 で比べる。

### M2（Major）: `(track, start_ms, end_ms)` による発話の結び付けが、選べる STT の 1 つと Self の再接続で成り立たない
- 根拠:
  - OpenAI の STT は確定の `FinalTranscript` で常に `audio_start_ms: None, audio_end_ms: None`（core `crates/stt-openai/src/lib.rs:373-379`）。
    `SttProviderKind::OpenAi` はライブで選べる（`crates/app-service/src/stt_provider_kind.rs:30-35`）。ADR が「未確認の前提」に置いている点は、
    コードで既に「成り立たない場合がある」と分かる。
  - Self トラックは再接続後の時刻が新しいセッションの 0 から数え直しになり補正されない（`live_transcription.rs:679-690` の Known caveat、`:971` の警告）。
    同じ `(SelfMic, start, end)` が会議の前半と後半で衝突しうる。
  - 逃げ道の `transcript_row_id` は得られない: `insert_transcript_segment` は行 ID を返さず（`store.rs:571`）、`SegmentData` にも載らず、
    `list_transcript_segments` も `id` を返さない（`store.rs:597-600`）。
  - `persist_pending_interim`（`live_transcription.rs:1397-1425`）が作る確定行はイベントを出さないので、ライブのグラフには入らず、DB だけにある。
  - 帰結: `Lagged` の読み直し（§7）と作り直し（§3）で重複した発話や取りこぼしが黙って起き、根拠が別の発話を指す。
- 修正案: 発話の同一性を DB の行 ID に寄せる。`insert_transcript_segment` が行 ID を返し、`SegmentData` に `row_id: Option<i64>` を足す
  （transcript-event の公開型の変更なので v0.3.0 に含める）か、グラフの consumer はイベントを「合図」としてだけ使い、
  発話は常に DB の確定行から `id > 最後に取り込んだ id` で取り込む（`Lagged` の特別扱いも不要になる）。どちらかを §1 と §7 で決める。

### M3（Major）: 作り直しの「新しい世代として作る」が、表の定義では実装できない
- 根拠: §3 は「既存のグラフを消さず、新しい世代として作る。UI で確認してから置き換える」とするが、
  `graph_nodes`／`graph_edges`／`graph_utterances`／`graph_patches` の主キーはどれも `(session_id, …)` だけで世代の列が無い（ADR §1）。
  同じ `nid = "n1"` や `rev` を 2 世代で持てず、「確認してから置き換える」までの間、どちらを表示するかも決まらない。
  `graph_grounds` には主キーも一意制約も無く、どの世代の `uid` かも区別できない。
  さらに schema の変更には移行が無い（`schema.rs:5-7`）ので、v1 で入れ損ねると後から列を足せない（ADR の「影響」も認めている）。
- 修正案: 全表に `generation INTEGER` を足して主キーに含め、`graph_generations(session_id, generation, source, state('building'|'candidate'|'active'|'discarded'), created_at)` で
  表示する世代を 1 つに決める。または v1 では作り直しを「確認の上で既存を消して作り直す」に縮める。どちらかを決めて §1 と §3 を一致させる。

### M4（Major）: 会議の終わりの発話と、抽出が止まった後の発話が、どこでも抽出されない
- 根拠: §3 の会議中の抽出は「6 件か 45 秒」で走り、会議の後の整理（consolidate）の入力は「グラフ全体と各ノードの根拠の発話」だけ。
  終了の直前に溜まった 5 件以下・45 秒以内の発話、終了時に動いていた呼び出しの分、`persist_pending_interim` の確定行（M2）、
  §7 の「3 回連続失敗で停止」以降の発話は、どの経路にも入らない。作り直しが自動で走るのは「会議中のグラフが無いとき」だけで、部分的なグラフがあれば走らない。
  会議の最後は決定・宿題が出やすく、§6 の「`decision` の無いまま終わった issue」が誤って数えられる。
- 修正案: `graph_utterances` に「抽出済み」の印（または `extracted_rev`）を持ち、会議の後は「動いている呼び出しを待つ → 未抽出の発話を増分抽出で流し切る → 整理」の順にすると §3 に書く。
  タスク 5 の受け入れ条件に「終了直前の 1〜5 件が抽出されるテスト」「停止後の発話が会議後に抽出されるテスト」を足す。

### m1（Minor）: macOS では作り直しの元になる文字起こしが無い
- 根拠: §3 は作り直しの用途に「macOS」を挙げるが、macOS の録音（`crates/app-service/src/macos_session.rs`）は `transcript_segments` に何も書かず、
  後からの文字起こし（`apps/desktop/src/gap_retranscription.rs`）も Windows 専用。macOS ではグラフを作れない。
  また `graph_enabled` を `hint_enabled` と同じ扱いにするなら、録音開始時に 1 回読むだけ（`apps/desktop/src/actions.rs:51-55`）で、「会議中に切っていた」は起きない。
- 修正案: v1 のグラフは Windows だけと明記し、macOS ではタブと作り直しのボタンを出さない（または「文字起こしが無い」と出す）。作り直しの用途から macOS と「会議中に切った」を外す。

### m2（Minor）: 検証の規則が足りず、適用の意味が決まっていない
- 根拠（§2）:
  - `tmp` を参照する操作（例の `add_edge` の `src: "a"`）で、参照先の `add_node` が落とされたときに連鎖して落とすかが無い。例の `merge` の `from: "b"` はどこにも定義されていない。
  - `merge` が既存の `nid` 同士なのか新しいノードなのか、統合されたノードのエッジと根拠を付け替えるか、`merged_into` の連鎖・循環を許すかが無い。
  - 人の判断の保護がノードだけ（「`review = approved/rejected` のノードを変えようとしている」）。エッジの `review`、却下したノードへの `merge`・エッジの追加は扱いが無い。
  - `set_status` の値と種類の組み合わせ（issue を `decided` にできるか等）、自己ループ・同じエッジの重複が無い。
  - 整理を「issue ごとの部分グラフ」に分けると、主な役目である issue 同士の重複の `merge` ができない。
- 修正案: 規則の表（操作 × 落とす条件 × 連鎖）を §2 に置き、タスク 2 の「規則ごとの単体テスト」がその表に対応するようにする。

### m3（Minor）: 背景の記述 2 か所が不正確
- 根拠:
  - 「Rhai の dispatcher に外部への HTTP 送信は無い」: `rag_search`／`rag_index`（`dispatcher.rs:54-55` → `crates/rhai-engine/src/rag.rs:28-61`）が Vertex・Bedrock・Cloudflare に HTTP で送り、`ai_summarize` も LLM に送る。
    正しくは「任意の URL に送る汎用のコマンドは無い」。選択肢 C を捨てる理由としてはこちらで足りる。
  - 「`summary_consumer.rs:56` が会議終了時に 1 回だけ呼ぶ」: 手動の `generate_summary_now`（`summary_consumer.rs:151`）と、Rhai の `ai_summarize`（`dispatcher.rs:82-88`）も `Summarizer::summarize` を呼ぶ。
  - 引用の行番号（`lib.rs:29-67,155,165`、`live_transcription.rs:103`、`schema.rs:5-7,98-110`、`Cargo.toml:61`、`ui.rs:507-576`、`summary_consumer.rs:229-271`）は正しかった。
- 修正案: 上の 2 文を直す。

### m4（Minor）: `Summarizer::complete` の形と、プロバイダ選択の 3 つ目の写し
- 根拠: `Summarizer` の実装はコアの `GenaiSummarizer`／`CliSummarizer` の 2 つだけで、アプリにもテスト用の実装は無い（`grep "impl .*Summarizer for"`）ので、既定の実装を置かない変更は安全。
  ただし `SummarizeOptions` は既に `system_prompt` を持つ（`crates/summarize/src/lib.rs:90-97`）ので、`complete(system, user, &options)` ではどちらが勝つかが曖昧。
  CLI 経路は prompt を 1 つのコマンドライン引数で渡す（`cli_backend.rs:137-138`）ので、Windows のコマンドラインの上限（32,767 文字）に、整理や作り直しの大きい入力が当たる。
  プロバイダの組み立ては既に `summary_consumer.rs:229-278` と `dispatcher.rs:90-128` に 2 回写されており、`graph_consumer.rs` で 3 回目になる。
- 修正案: `complete(&self, req: &CompletionRequest)`（system・user・model）にして `system_prompt` を使わない形にする。CLI 経路の入力の上限（文字数）を §3 の「上限」として決める。
  プロバイダの組み立てを summarize の関数 1 つにまとめるのをタスク 4 に含める。

### m5（Minor）: ログの規則に、エラー文が本文を含みうる点が無い
- 根拠: §8 は「エラーの種類」だけを出すとするが、`CliSummarizeError::NonZeroExit` は CLI の stderr を丸ごと持つ（`cli_backend.rs:82-83,145-149`）。
  `%err` でそのままログに出すと、CLI が prompt の一部を stderr に出した場合に会話の本文が漏れる。要約の consumer は `SummaryEvent::Failed { error: e.to_string() }` で同じ文を流している。
- 修正案: §8 に「`SummarizerError` は `Display` をログに出さず、種類（timeout／not_found／non_zero_exit／parse／provider）に写してから出す」と書き、タスク 5 のテストに含める。

### m6（Minor）: `graph.{sid}.updated` の受け手が居ない
- 根拠: 表示は 250ms のポーリングで `rev` を読む（§5）ので、v1 にこの subject の購読者は居ない。`LocalBroker::publish` は購読者が居ないと `Err(NoSubscribers)` を返す（`crates/local-broker/src/lib.rs:67-84`）ので、
  他の consumer のように `warn!` すると更新のたびに警告が出る。
- 修正案: `publish_session_stopped`（`apps/desktop/src/recording.rs:51-55`）と同じく `NoSubscribers` を無視すると書くか、v1 では subject を出さない。

### m7（Minor）: owner の決定と実装の分割の食い違い
- 根拠: Q5 は「推奨」で日付が無い（未決定）のに、「影響」と設定の節は「既定は無効」を決定として書いている。
  タスク 1 の S1 は「マージしないブランチ」だが、そこで作る合成データと正解をタスク 3 の評価テストで使うとしている（マージしない枝のファイルは使えない）。
  `discussion-graph` と `session-store` のどちらがどちらに依存するか（保存の関数がグラフの型を受けるか）が無い。
- 修正案: Q5 を owner に確認して日付を入れる。合成データと正解はタスク 2 か 3 で main に入れると書く。crate の依存の向きを §「影響」に書く。

## 判定
converged: no（Blocker 0 / Major 4 / Minor 7 / nit 0）
