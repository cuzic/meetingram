# PR #15 レビュー（round 6）

対象: cuzic/meetingram feat/graph-consumer @ 0ba6cb6105ee4dd6b0ae441263c60d7b8d2a3011（差分は round 5 の 979567c / 57c3cc1 から。main の取り込み 589f68c の CI・ADR の変更は対象外）

## 結論（先に）
converged: no（Major 1）。v0.4.0 への追従、整理の 2 段（段 1 を保存してから、保存後のグラフで段 2 を組み立てる。どこかで失敗したら止める）、「整理が済んでいない」印、取り込んでから数える `pending_count`、Rhai の `cli-tmp` は、仕様のとおりでテストもある。録音の経路を止めるものは無い。
ただし仕様に無い挙動の変更が 1 つある。整理の応答が読めないときの 1 回の再試行を外している。ADR §2 に反し、PR 本文にも書かれていない（M1）。

## 前ラウンドの解消状況
round 5 で converged。今回は新しいスコープ（PR #15 の最新の「## [codex] タスク仕様」）を見た。

## 確認したこと
- 依存: ルートの `Cargo.toml` のコア 15 crate がすべて `tag = "v0.4.0"`。`Cargo.lock` のコアの source もすべて `?tag=v0.4.0#440f215`。
- Rhai: `main.rs` が `RhaiEngine::new_with_cli_temp_base(..., Some(cli_temp_base.clone()))` で作る。`cli_temp_base` は `<app_data_dir>/cli-tmp`（`main.rs` の `cli_temp_base` とそのテスト）。
- 整理の 2 段（`graph_consumer.rs:696-`）: 段 1 は `apply`（検証と保存）の後に `load_graph` で読み直し、そのグラフで `build_consolidate_groups` を組み立てる。段 1・段 2 のどれかが `Err` なら `?` で止める。保存済みのパッチは残り、途中のものは 1 トランザクションずつなので壊れない。外した件数は `debug!` に件数だけ出る。
- 印（`consolidated` の集合。入っていれば「済んだ」）: `flush` の初めに外し（`:665`）、整理が `Ok` のときだけ入れる。Continue で新しいパッチも印も無いときは入れ直して返る。失敗、cancel、流し切りの失敗での停止では入れない。作り直しは消した後に `flush(true)` なので必ず整理する。このプロセスで見ていないセッションは集合に無いので「済んでいない」。セッションのエントリの刈り込みとは別の集合なので、刈り込みで消えない。
- `pending_count` の取り込み: `db()`（`spawn_blocking` で `GraphStore` の std の `Mutex` を取る）を通る。UI から呼んでも、実行者の取り込み・適用と同じ `Mutex` で順に並ぶ。取り込みは `BEGIN IMMEDIATE` の中で `MAX(row_id)` / `MAX(uid)+1`、`UNIQUE (session_id, row_id)` なので重複しない（core PR #15 で 3 本の同時取り込みを確かめ済み）。実行者が `pending_utterances` を読んでから `apply_patch` するまでに UI が行を足しても、その行は未抽出のまま次の回に入るだけ。録音側への影響は、1 回あたりミリ秒の書きのトランザクションが 1 つ増えるだけ（下の n1）。
- CI: 4 ジョブ pass（Linux、Windows・macOS の build、e2e-windows）。Windows・macOS のログで `graph_consumer::tests` の 24 件が全部 `ok`。
- Linux の一時 crate（コミットしない）で 30 件 pass。変異の結果:
  - 印を無視する: 2 件落ちる
  - 整理の成功で印を入れない: 落ちる
  - 失敗でも印を入れる: 落ちる
  - 段 2 を段 1 の前のグラフで組み立てる: `consolidation_builds_groups_after_saving_issue_patch` が落ちる
  - 段 1 の失敗を無視して段 2 へ進む: 落ちる
  - `pending_count` で取り込まない: 18 件落ちる
  - 整理の再試行を戻す: `invalid_issue_response_…` が落ちる（下の M1。テストが再試行しないことを固定している）
  - 流し切りが失敗で止まったときに印を入れる: どのテストも落ちない（下の m1）

## Findings
### M1（Major）: 整理の応答が読めないときの 1 回の再試行を、仕様に無いまま外している
- 根拠: `graph_consumer.rs:743` の `Err(error) if source == PatchSource::Consolidate => return Err(parse_kind(error))` が、整理では再試行しない。round 5 までは、増分・整理とも `apply` の中で 1 回だけ再試行していた（round 1 の m2 で確かめた挙動）。ADR §2（303 行）は、応答が読めなければ種類を添えて 1 回だけ再試行すると決めている。§3（353 行）の「整理の失敗は 1 回で諦める」は、整理を繰り返さない（流し切りのような 3 回の予算を持たない）という意味で、round 1〜5 もそう読んできた。今回のタスク仕様の「やらないこと」は「上以外の挙動の変更」で、PR 本文の「v0.4.0 への追従」にもこの変更は書かれていない。整理の呼び出しは 1 会議に 2 回以上あり、1 回でも形の崩れた応答が返ると、それ以降の段は全部捨てられる。テスト `invalid_issue_response_skips_groups_and_can_resume_without_a_new_patch` は再試行しないこと（呼び出し 2 回）を固定している。
- 修正案: `:743` の分岐を外し、整理でも 1 回だけ再試行する（入力の上限は増分と同じく `INPUT_LIMIT` で再試行の注記の分を空ける）。テストは、段 1 の応答が 2 回続けて読めないと段 2 を呼ばない、に直す。整理では再試行しないのが意図なら、実装する前に Open questions に挙げて ADR に書く。

### m1（Minor）: 流し切りが失敗で止まった後も印が「済んでいない」のままであることのテストが無い
- 根拠: `:676-677` の停止の直前で印を入れる変異でも、全テストが通る。「整理の前に中断したセッションで続きを抽出が整理する」は、見ていないセッションと、段 1 の失敗の場合だけで確かめていて、流し切りの失敗で止めた場合は通らない。今の挙動は正しい（`return` で印を入れない）。
- 修正案: 流し切りが 3 回失敗した後、新しいパッチなしの Continue（行は抽出できる）で整理が呼ばれることを確かめる。

### n1（nit）: `pending_count` が書きのトランザクションになった
- 根拠: `:227` で取り込み（`BEGIN IMMEDIATE`）をしてから数える。タスク 6 の画面が録音中のセッションにこれを 250ms ごとに呼ぶと、録音側の書き込みと同じ DB に、毎秒 4 回の短い書きのロックが加わる。待ちはミリ秒で、busy timeout（5 秒）には遠い。
- 修正案: 録音中は `status().pending` を使い、`pending_count` は履歴画面だけで呼ぶことを、タスク 6 の仕様に書く。

## 判定
converged: no（Blocker 0 / Major 1 / Minor 1 / nit 1）
