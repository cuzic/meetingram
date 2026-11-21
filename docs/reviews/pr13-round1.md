# PR #13 レビュー（round 1）

対象: cuzic/meetingram deps/core-v0.3.0 @ 97f758d65b3c11c1ac1353e5e112881f47f8045d

## 結論（先に）
converged: yes。コアの全 crate が `tag = "v0.3.0"`（`Cargo.lock` は全て `6cf164f`、v0.2.0 やブランチ参照の残りは 0）、`rhai` はアプリとコアで `=1.27.0`。
保存済みの鍵とモデルはコアの `SummaryProvider` で同じ値（コアの PR #17 で 13 種類を突き合わせ済み。アプリ側にも鍵のテスト）、利用者向けの日本語のメッセージは保たれている。
残りは、2 つ目の GUI を起動したときの起動時の掃除、Rhai の CLI が `cli-tmp` を使わない点、挙動の変化が PR 本文に無い点（すべて Minor）。

## 前ラウンドの解消状況
なし（round 1）。

## Findings
### m1（Minor）: 2 つ目の GUI の起動時の `cleanup_stale_cli_dirs` が、1 つ目の作業中の CLI のディレクトリを消しうる
- 根拠: `apps/desktop/src/main.rs` の `main` の冒頭で、無条件に `<app_data_dir>/cli-tmp` の `meetingram-cli-*` をすべて消す。アプリは 2 つ目の GUI の起動を止めない
  （`control_server.rs` の `bind` は `AddrInUse` で退くだけで起動は続く）。1 つ目が要約中（上限 600 秒）なら、codex の `-o` の出力ファイルや作業ディレクトリが消え、その要約は失敗する。
  ADR §2 の意図は「アプリが落ちて残ったもの」を消すこと。
- 修正案: 起動ごとのサブディレクトリ（例: `cli-tmp/<pid>`）にして、生きていない PID のものだけ消すか、一定時間（上限 600 秒より長く）より古いものだけ消す。条件が稀なので follow-up でよい。

### m2（Minor）: Rhai の `ai_summarize` の CLI は `cli-tmp` を使わず OS の一時ディレクトリに作る
- 根拠: コア v0.3.0 の `crates/rhai-engine/src/dispatcher.rs:91` が `build_summarizer(…, None)` を渡す。アプリの `AppSettingsProvider` には基底を渡す口が無い。
  落ちたときの残りは起動時の掃除の対象外で、ADR §2（データディレクトリの下）からも外れる。アプリだけでは直せない。
- 修正案: コアの follow-up で `RhaiEngine` か `SettingsProvider` に CLI の基底を渡す口を足し、アプリは `cli_temp_base` を渡す。

### m3（Minor）: 置き換えに伴う要約の挙動の変化が PR 本文に無い
- 根拠: v0.2.0 → v0.3.0 で、(a) CLI の要約に時間の上限 600 秒が付いた（v0.2.0 は上限なし）、(b) API キーのプロバイダで、選んだプロバイダ以外に解決されるモデル名
  （カスタム入力の一部）は送る前に拒否される、(c) Rhai の `ai_summarize` で ClaudeOAuth が組み立てられ、API キー・Vertex の未設定が呼び出し前に設定のメッセージになる、
  (d) Vertex の認証情報が不正なときのメッセージから `serde_json` の詳細が消えた。アプリの「要約の挙動は変えない」の例外として本文に書くべき。
- 修正案: PR 本文の「変更」に上の 4 点を足す（コードの変更は不要）。

## 確かめたこと
- 依存: `Cargo.toml` の 15 crate（`discussion-graph` を含む）が `tag = "v0.3.0"`。`Cargo.lock` の meetingram-core の参照は全て `?tag=v0.3.0#6cf164f…`（= タグ v0.3.0 の SHA）。
  `discussion-graph` は workspace の依存に足しただけで、`apps/desktop/Cargo.toml` は使っていない（仕様どおり）。`rhai` はアプリ `=1.27.0`、コア `=1.27.0`、ロックは 1.27.0 の 1 つだけ。
- 設定の互換: アプリの `SummaryProvider` の定義と、`key`・`default_model`・`api_key_account`・`from_key`・`is_vertex`・`cli_backend` を削除し、コアのものを使う。
  表示名と `known_models` は `SummaryProviderDisplay`（アプリ内の trait）に残る。`uses_cli()` は `cli_backend().is_some()` に置き換え（同値）。
- 組み立て: `summary_consumer.rs` は鍵・モデルの読み込みと `build_summarizer` を `spawn_blocking` の中で行い（キーリングの読み取りが async のワーカーを塞がなくなった）、
  `Some(cli_temp_base)` を渡す。エラーの写しは `MissingVertexCredentials` / `MissingApiKey` が従来と同じ日本語、`InvalidVertexCredentials` / `Client` は従来の文言から詳細を外したもの。
- 起動時の掃除: `main` の冒頭（`SummaryConsumer` と `RhaiEngine` の作成より前）で `create_dir_all` → `cleanup_stale_cli_dirs`。失敗は `kind` だけを `warn` に出して続ける。対象はアプリ専用の `cli-tmp` だけ。
- ログ: 新しいログは上の `warn` の 1 行だけ（本文・認証情報なし）。要約のエラー文は従来どおり `SummaryFailed` のイベントに載り、ログには出ない。
- スコープ: 変更は `Cargo.toml`・`Cargo.lock`・`apps/desktop/src/**` だけ。
- CI: `gh pr checks 13` の 3 ジョブが pass。macOS と Windows のジョブログで `summary_provider_keys_match_saved_settings`、`all_summary_providers_have_display_labels`、
  `cli_temp_base_is_under_app_data_dir` が ok（macOS 52 件・Windows 51 件、失敗 0）。手元（Linux）で `cargo fmt --all --check`、`desktop-cli` の clippy と test（5 件）が pass。
  `desktop` は GTK が無く手元でビルドできないので、変異は確かめていない（PR 本文はプローブで鍵の変異が落ちたと報告）。
- 実機確認が必要（PR 本文に記載）: 要約ボタンで CLI の要約が動くこと、Windows の npm 版 CLI で要約が動くこと。

## 判定
converged: yes（Blocker 0 / Major 0 / Minor 3 / nit 0）
