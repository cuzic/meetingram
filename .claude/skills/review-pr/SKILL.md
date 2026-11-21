---
name: review-pr
description: meetingram / meetingram-core の PR か ADR を敵対的にレビューし、docs/reviews/prNN-roundK.md を PR ブランチにコミットする。Codex の PR が戻ってきたとき、PR のレビュー、レビューラウンド、収束判定を頼まれたときに使う。
---

# PR / ADR の敵対的レビュー

目的は **Blocker と Major を見つけること**。体裁や好みの指摘を増やさない。
Codex の PR 本文は自己申告として読み、主張はすべて差分と実行結果で確かめる。

## 入力

- repo、PR 番号、ラウンド番号 K。前ラウンドのレビューファイルがあれば必ず読む。
- 根拠の ADR（Accepted）とタスク仕様（Issue か PR コメント）。

## 手順

1. **独立したコピーで作業する**: スクラッチ領域に
   `git clone --reference ~/src/<repo> https://github.com/cuzic/<repo> <dir>` して `gh pr checkout NN`。
   レビューする head の SHA（`git rev-parse HEAD`）を控える。共有の checkout や Codex の clone を使わない。
2. **CI を読む**: `gh pr checks NN`。赤なら、ログの失敗箇所を根拠に指摘にする（推測で直さない）。
   Linux で通るもの（コアのテスト、`capture-windows` の Windows 向け型チェック、`desktop-cli`）は手元でも回す。
3. **前ラウンドの解消確認**: 指摘ごとに、説明文ではなく挙動やテストで確かめる。
4. **仕様との照合**: タスク仕様の「やること」がすべてあり、「やらないこと」と触ってよいファイルの外に変更が無いか。
5. **壊しにいく**。重点:
   - **録音を止めない**: 文字起こし・要約・ヒント・図の失敗や遅延が、録音やセグメントの保存を止めたり遅らせたりしないか。
     async の中のブロッキング、`broadcast` の lag、Rhai の `call_async` の同期待ち、ロックの持ちすぎ。
   - **コールバックスレッドでの panic**: COM / GCD のコールバック、`spawn` したタスクの `unwrap` / `expect`。
   - **個人データ**: API キー・文字起こし・要約がログ、エラー文、テスト、コミットに出ないか
     （meetingram-core の `docs/logging-policy.md`）。新しい外部送信が ADR と owner の承認どおりか。
   - **保存**: SQLite のスキーマ変更に移行があるか、データディレクトリやキーリングの名前を変えていないか。
   - **プラットフォーム**: `cfg(windows)` / `cfg(target_os = "macos")` の側だけ壊れていないか（CI のどのジョブが通したか）。
   - **2 つの repo**: アプリがタグ参照か（ブランチ参照のままでないか）、`rhai` の版がそろっているか、
     コアの公開 API の変更にアプリが追従しているか。
   - **Rhai**: hook から呼んだヘルパー関数がモジュールの変数を読んでいないか（`plugins/default/hint.rhai` 冒頭の注記）。
6. **テストの効き目**: 新しいテストが、壊した実装で落ちるか。手書きの変異を数個入れて確かめる。
   プローブや変異はコミットしない。

## 重大度

| 等級 | 基準 |
| --- | --- |
| Blocker | 録音が止まる・失われる、データを黙って失う、キーや会話内容の漏えい |
| Major | 特定の条件で落ちる・固まる、エラーを黙って握りつぶす、仕様や ADR の違反、重要な経路にテストが無い、片方の OS でしか確かめていない変更を「動く」としている |
| Minor | 過剰なエラー、文書の不一致、テストの穴（挙動は正しい） |
| nit | 表現・コメント |

## 出力

`docs/reviews/prNN-roundK.md`（ADR は `docs/reviews/adr-NNNN-roundK.md`）:

```markdown
# PR #NN レビュー（round K）

対象: <repo> <branch> @ <レビューした head の SHA>

## 結論（先に）
<2〜4 文。converged: yes/no と理由>

## 前ラウンドの解消状況
<指摘ごとに 1 行>

## Findings
### M1（Major）: <一文>
- 根拠: <再現手順・ファイル:行・CI のログ>
- 修正案: <一文>

## 判定
converged: yes/no（Blocker n / Major n / Minor n / nit n）
```

**converged: yes** は Blocker と Major が 0 のとき。Minor は follow-up に回してよい。

## 届け方

レビューファイルだけを PR ブランチにコミットして push する（メッセージ `docs: PR #NN review round K`）。
push が拒否されたら `git pull --no-rebase origin <branch>` してから push し直す（force-push しない）。
converged: no なら、続けて `.claude/skills/codex-task` で修正の仕様を PR コメントに書き、Codex を起動する。
最後の返答は、結論・判定・指摘を 1 行ずつと、push したコミットの SHA。
