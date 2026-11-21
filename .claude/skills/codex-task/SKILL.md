---
name: codex-task
description: meetingram / meetingram-core の実装・レビュー指摘の修正・main の取り込みを Codex に渡す。タスク仕様（Issue か PR コメント）を書き、run-codex.sh で Codex を起動し、戻ってきた PR を確かめる。「実装して」「Codex に渡して」「指摘を直させて」と言われたときに使う。
---

# Codex へのタスクの渡し方

Codex は AGENTS.md と `.agents/skills/implement-task` を読むが、この会話の文脈は知らない。
仕様だけで作業が閉じるように書く。判断が要る点は先に決めて書くか、
「決めずに Open questions に書いて止める」と明示する。

## 1. 仕様を書く

- 新しい作業: その repo に Issue（タイトル `[codex] <短い要約>`）。
  `gh issue create -R cuzic/<repo> --title … --body-file …`
- 既存 PR への修正・main の取り込み: その PR にコメント（見出し `## [codex] タスク仕様`）。
  根拠のレビューファイルは PR ブランチにコミット済みのもの（`docs/reviews/prNN-roundK.md`）を指す。

```markdown
## [codex] タスク仕様

### 目的
<1〜2 文。何ができるようになるか>

### 根拠
- ADR: <repo> の docs/adr/NNNN-….md の §x（Accepted）
- レビュー: docs/reviews/prNN-roundK.md の M1, m2（修正の場合）

### ブランチ
- 新規: `<branch>`（起点 `origin/<base>`）。PR の base は `<base>`。積み上げなら PR タイトルに "stacked on #NN"
- 既存 PR: `<branch>` に追加コミット。merge で追従し、force-push しない

### スコープ
- やること: <箇条書き。指摘 ID ごとに 1 行>
- やらないこと: <範囲外>
- 触ってよいファイル: <crate / ディレクトリ>

### 受け入れ条件
- AGENTS.md の完了の定義（CI 全緑を含む）
- 追加するテスト: <再現テスト、境界、失敗時に録音を止めないこと など>
- 変異: <落ちるべき変更の例>
- コアに触る場合: <アプリ側で必要な追従（タグ更新は Claude がマージ後に別タスクにする）>

### 報告
PR 本文に「変更 / 確認したコマンドと結果 / CI でしか確かめられないこと / Open questions / Status」を書く。
コミットメッセージに指摘 ID を入れる。マージはしない。
```

粒度は 1 タスク = 1 PR = レビュー 1 回で見られる量（数百〜千行）。コアとアプリにまたがるなら 2 タスクに分ける。

仕様を出す前の確認（過去に漏れたもの）:

- **CI の workflow を足す・変える**: `paths` の絞り込みに、テストが読むディレクトリをすべて入れる
  （コアは `crates/**` だけでなく `plugins/**`。`rhai-engine` のテストが `plugins/default/*.rhai` を読む）。
  「既存の workflow と同じ」と書くだけにしない。既存のほうが漏れていることがある。
- **機械的な変更（`cargo fmt`、一括置換、生成物）**: それだけのコミットに分けさせ、手での変更を混ぜさせない。
  レビューで同じコマンドを親コミットにかけ直し、一致を確かめられるようにする。
- **判断の逃げ道**: 公開 API・挙動が変わりそうな箇所の扱い（`allow` で残す / 止めて聞く）を先に書く。

## 2. Codex を起動する

プロンプトは短くし、本体は Issue / PR コメントに置く。スクラッチ領域に書く:

```text
AGENTS.md と .agents/skills/implement-task/SKILL.md に従って、次をやって:
https://github.com/cuzic/<repo>/issues/NN
```

Bash の `run_in_background: true` で起動する（数十分かかる。完了は通知で受け取り、ポーリングしない）:

```sh
CODEX_WORKDIR=<スクラッチ領域>/codex CODEX_BIN=<codex のパス> \
  .claude/skills/codex-task/run-codex.sh <repo> <branch> <label> <prompt-file> [base]
```

- 同時に動かすのは 2〜3 まで。同じブランチに 2 つの Codex を走らせない。
- `codex` が PATH に無ければ、owner に場所を聞くか、メモリにある場所を使う。

## 3. 戻ってきたら

1. 出力の `exit=`、`unpushed=`、`dirty=` と最終メッセージ（`last=` のファイル）を読む。0 以外、利用上限、
   途中終了なら、ログの末尾で原因を確かめ、同じ仕様で再起動するか owner に伝える。
   **exit=0 でも PR が無い・`unpushed` か `dirty` が 0 でない**なら、Codex が commit か push で止まっている。
   - ログで失敗したコマンドとエラー文を確かめる（`grep -n -iE "read-only|denied|rejected|scope" <log>`）。
     過去の例: sandbox が `.git` を読み取り専用にした、https の push が `workflow` スコープ不足で拒否された。
     どちらも run-codex.sh で対処済みなので、再発したらスクリプトの前提が崩れている。原因を直してから次を起動する。
   - 成果は捨てない。Claude が代行する: `dirty` なら差分を読んでから clone でコミットし（Codex の
     `Co-Authored-By` を付ける）、`git -C <clone> push -u origin <branch>`（clone は ssh）。PR が無ければ
     `last=` の報告から implement-task の形の本文を作って `gh pr create`。Status に「push と PR 作成は Claude が代行」と書く。
   - Codex が別の場所に clone し直して作業していたら、最終メッセージかログからその場所を探し、そこから push する。
     終わったらその場所と `/tmp` に残した一時ファイルを消す。
2. PR ができているか、Open questions が無いかを `gh pr view` で確かめる。Open questions があれば、
   owner の判断が要るものは AskUserQuestion、設計で決められるものは仕様コメントに答えを書いて再起動。
3. 報告をそのまま信じない。`.claude/skills/review-pr` で差分そのものをレビューする
   （CI を待ってから。CI が赤なら、ログを根拠に修正タスクを PR コメントで出す）。
4. PR がマージされたら `clone=` と、同じ置き場の `git-<label>`・`tmp-<label>` を消す。
