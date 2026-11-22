# PR #15 レビュー（round 2）

対象: cuzic/meetingram feat/graph-consumer @ b1f8823ac4fd2b100959afc6bcb7f9127e2753a1

## 結論（先に）
converged: no（Major 1）。round 1 の M1・m1〜m3・n1・n2 は、変異を入れるとテストが落ちることで、すべて解消を確かめた。録音の経路を止めるものは今回の差分にも無い。
ただし m3 の直し方のうち「前の整理の後に新しい抽出のパッチがあるか」の判定が、アプリから SQLite の 3 本目の接続を開き、グラフの表（`graph_patches`）を直接読んでいる。ADR 0001 の「`discussion-graph` の接続だけがグラフの表を読み書きする」に反する（M1）。

## 前ラウンドの解消状況
- M1: 解消。`three_live_failures_stop_calls_but_keep_importing` は、3 回だけ失敗するダミーで、止めた後の行が流し切りのプロンプトに入り、`pending_count == 0` になることを確かめる。3 回で止めたら `live()` から抜けて流し切りをしない変異で落ちる。
- m1: 解消。`post_meeting_actions_are_rejected_while_recording` は、録音中の 3 つの操作を拒み、停止後なら受け付けることを確かめる。`state.recording ||` を外すと落ちる。
- m2: 解消。`unreadable_response_is_retried_once_with_error_kind` は、再試行のプロンプトに種類が付くことと、2 回目も読めなければ諦めることを確かめる。再試行を消すと落ちる。整理の上限を 60 秒にすると、`consolidation_uses_a_longer_timeout_than_extraction` ともう 1 件が落ちる。
- m3: 挙動は解消。生きているノードが 0 件なら整理しない（判定を外すと `consolidation_skips_graphs_without_live_nodes` が落ちる）。前の整理の後に live / flush のパッチが無ければ整理しない（判定を外すと `continue_only_consolidates_after_a_new_extraction_patch` が落ちる）。後者の実装の仕方が下の M1。
- n1: 解消。状態のロックはすべて `unwrap_or_else(into_inner)` を通る。`unwrap()` に戻すと `run_guard_drop_recovers_a_poisoned_state` が落ちる。
- n2: 解消。使い終わったセッションのエントリは `SessionLease` の drop と `recording_stopped` で消える。消すのを止める変異で 2 件落ちる。

## 確認したこと
- CI: 3 ジョブ pass。Windows・macOS のログで `graph_consumer::tests` の 18 件が全部 `ok`。
- Linux: round 1 と同じ一時 crate（コミットしない）で 24 件全部 pass。上の変異もこの crate で走らせた。
- 新しい差分の `SessionLease` と `prune_idle`: ロックを取る順序は、どこでも `sessions` → `session.state`（`RunGuard` は `state` だけ）で、逆順の箇所は無い。動いているタスクは lease を持つので、走っている間にエントリは消えない。作り直しの 2 回押しを防ぐ `rebuilding` も、作り直しのタスクが lease を持つ間は残る。`disabled` の表は、起動しなかったセッションごとに 1 件残る（macOS では毎回）。小さいので指摘にしない。
- 読み取り専用の接続（`has_unconsolidated_patches`）の影響: `db()` のクロージャの中なので `spawn_blocking` で走り、tokio のワーカーは塞がない。読み取り専用で WAL なので録音側の書き込みは止めず、rusqlite 既定の busy timeout（5 秒）も付く。録音への影響は無い。

## Findings
### M1（Major）: アプリが 3 本目の SQLite 接続でグラフの表を直接読んでいる（ADR 0001 §1・§「保存の接続」P に反する）
- 根拠: `apps/desktop/src/graph_consumer.rs:806` の `has_unconsolidated_patches` が `rusqlite::Connection::open_with_flags(database, SQLITE_OPEN_READ_ONLY)` を開き、`graph_patches` の `source` と `rev` を SQL で読む（呼び出しは `:691`。`apps/desktop/Cargo.toml` に `rusqlite` を追加）。ADR §1 は「同じ DB ファイルに表を足し、`discussion-graph` の接続だけが読み書きする」、§P は「`discussion-graph` の中に閉じ込める」と決めている。これでコアの表の定義（列名、`source` の文字列）がアプリに写る。コアが表を変えても、アプリのコンパイルは通り、実行時に `db` のエラーとなって整理が毎回落ちる（往復テストで守れない）。`load_graph` とは別の接続なので、同じスナップショットからも読んでいない。コードのコメントにあるとおり v0.3.0 の `GraphStore` に必要な関数が無いからで、Issue #14 は、コアの変更が要るなら Open questions に書くよう決めていた。
- 修正案: どちらか。(a) コアに `GraphStore` の関数（例: `needs_consolidation(sid)`、またはソースごとの最新の rev）を足してタグを上げ、アプリはそれを呼ぶ。(b) アプリの中だけで判定する。その回の流し切りが抽出のパッチを 1 件以上保存したとき、または作り直しのときだけ整理する（「生きているノードが 0 件なら飛ばす」はそのまま）。どちらでも `rusqlite` の直接の依存は外す。(a) を選ぶなら owner の判断として Open questions に挙げる。

## 判定
converged: no（Blocker 0 / Major 1 / Minor 0 / nit 0）
