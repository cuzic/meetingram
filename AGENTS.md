# meetingram — エージェント共通ガイド（Codex・Claude 共通）

会議を録音・文字起こしし、議論をマインドマップや TOC の図（CRT、対立解消図）にして、
次に何を聞くべきかを支援するデスクトップアプリ。この repo はアプリ本体で、
録音・文字起こし・イベント配信・Rhai プラグインは
[meetingram-core](https://github.com/cuzic/meetingram-core)（`~/src/meetingram-core`）にある。
設計は `docs/adr/`（アプリ）と meetingram-core の `docs/adr/`（0015 以降）にある。
実装の前に、担当する ADR（Accepted のもの）を読むこと。

## 役割分担

| 担当 | 仕事 |
| --- | --- |
| Claude | 設計（ADR・spike）、owner への質問の整理、Codex へのタスク仕様、PR の敵対的レビュー、マージとタグ付け |
| Codex | タスク仕様に沿った実装、レビュー指摘の修正、main の取り込みと衝突解消、テストの追加 |
| CI | Windows と macOS でのビルド・clippy・テスト（この 2 つの OS でしか通せないものの最終判定） |
| owner | 仕様の判断、実機での確認（録音・文字起こし・画面） |

- 仕事は「タスク仕様」（目的・スコープ・受け入れ条件・触ってよいファイル）で渡す。新しい作業は
  `[codex] …` の Issue、既存 PR への修正や main の取り込みはその PR の `## [codex] タスク仕様` コメント。
- 仕様に無い判断が要るときは、実装で決めずに PR 本文の「Open questions」に書いて止める。
- レビュー指摘の根拠は、PR ブランチにコミットされた `docs/reviews/prNN-roundK.md`。
- Codex の手順は `.agents/skills/implement-task/`、Claude の手順は `CLAUDE.md` と `.claude/skills/`。

## 完了の定義

この repo のアプリ（`apps/desktop`）は Linux ではビルドできない（GTK/WebKitGTK が無い）。
Linux で通せるものは手元で、それ以外は PR の CI（`.github/workflows/app-build.yml`）で確かめる。

```sh
cargo fmt --all --check
cargo test -p desktop-cli                     # Linux で通るテストはこれだけ
gh pr checks <PR> --watch                     # Linux（fmt・desktop-cli）と Windows・macOS の clippy -D warnings とテスト
```

- CI の 3 ジョブ（Linux・Windows・macOS）がすべて通ること。CI を通す前に「動く」と報告しない。
- Windows の CI は、実際の音声デバイスが要るテスト `control_server::tests::status_start_status_stop_round_trip`
  を飛ばしている。録音の開始・停止に触れたら、PR 本文に「実機確認が必要」と書く。
- 新しい挙動にはテストを付け、そのテストが落ちる変更（手書きの変異でよい）を 1 つ確かめる。
- `cargo fmt --all` をかけてからコミットする。触った crate で clippy の警告を増やさない（CI が `-D warnings`）。

## 2 つの repo のつながり

- アプリは meetingram-core を `Cargo.toml` の `[workspace.dependencies]` で **タグ固定**で参照する
  （`tag = "vX.Y.Z"`、全 crate を同じタグに）。
- コアとアプリの両方に触る変更は、コア PR → マージ → タグ → アプリ PR の順。アプリ PR の途中では
  コアのブランチを参照してよい（`branch = "…"`）が、**ブランチ参照のままアプリをマージしない**。
- `rhai` は `=` で固定しており、コアとアプリで同じ版にする（`rhai::Dynamic` などの型を受け渡すため）。
- `.github/workflows/desktop-bundle.yml` の `DX_VERSION` は、Cargo.lock の `dioxus` と同じ版にする。

## 安全規則

- API キーは OS のキーリング（サービス名 `meetingram`）にだけ置く。コード・ログ・テスト・コミットに書かない。
- 録音と文字起こしは個人の会話。実在の会議の音声やテキストを fixture やログに入れない。
  テストには合成した音声・文章を使う。
- プローブや一時ファイルは repo の外（スクラッチ領域）に置く。
- コミットやコメントにマシン固有の情報（ユーザー名、絶対パス）を入れない。
- 人の承認が要る操作: リリース（`v*.*.*` タグ、`desktop-bundle.yml`）、ライセンスの変更、
  新しい外部サービスへの送信（STT・LLM のプロバイダ追加など）。

## Git の約束

- ブランチは `feat/… fix/… docs/… chore/… ci/… test/… spike/… deps/…`。1 つの作業に 1 つの clone か `git worktree`。
  共有の checkout でブランチを切り替えない。
- **merge で追従し、rebase / force-push はしない**。既存 PR で作業を始める前と push が拒否されたときは
  `git pull --no-rebase origin <branch>`（ブランチ名を必ず書く）。最初の push は `git push -u origin <branch>`。
- main の取り込みで手書きの衝突解消をしたら、マージコミットのメッセージに何をどう解いたかを書く。
- Codex は PR を作って更新するが、**マージしない**。マージは Claude（`.claude/skills/merge-pr`）か owner。
- コミット末尾に使っているツールの `Co-Authored-By:` を付ける。レビュー指摘を直すコミットには指摘 ID（`M1` など）を書く。

## repo の地図

| 場所 | 中身 |
| --- | --- |
| `apps/desktop` | Dioxus のデスクトップアプリ。録音の操作、文字起こしの表示、要約（`summary_consumer.rs`）、Rhai のヒント（`hint_consumer.rs`）、制御ソケット（`control_server.rs`） |
| `apps/desktop-cli` | `meetingramctl`。起動中のアプリを制御ソケット越しに start / stop / status |
| `docs/adr/` | アプリの設計（0001 から。形式は `.claude/skills/design/adr-template.md`） |
| `docs/reviews/` | PR のレビュー記録（`prNN-roundK.md`） |

コア側の地図は meetingram-core の `AGENTS.md` にある。
