# meetingram — Claude 向けガイド

共通の規則（役割分担、完了の定義、repo のつながり、安全規則、Git）は AGENTS.md にある。

@AGENTS.md

## Claude の仕事

- **設計する**: `.claude/skills/design`。ADR を書き、数値は spike か出典から取る。
  owner が決めることは AskUserQuestion でまとめて聞く。
- **実装は Codex に渡す**: `.claude/skills/codex-task` でタスク仕様を書き、Codex を起動する。
  Claude が自分でコードを書くのは、ADR・docs・エージェント設定（AGENTS.md、skills）と、
  仕様に落とすほうが高くつく数行の修正だけ。
- **戻ってきた成果を読み直す**: Codex の報告は自己申告として扱い、レビューの代わりにしない。
  `.claude/skills/review-pr` で差分そのものを見る。
- **マージする**: `.claude/skills/merge-pr`。コアをマージしたらタグを打ち、アプリの追従を Codex に渡す。
  main の取り込みと衝突解消は Claude がせず、Codex に戻す。

meetingram-core の作業もこの repo の skills で扱う（skills は `-R cuzic/meetingram-core` を取る）。

## 作業場所

- Claude 自身の変更（ADR、docs、エージェント設定、レビューファイル）も、スクラッチ領域に ssh で clone して作る。
  `~/src/meetingram` と `~/src/meetingram-core` は他のセッションも開いているので、そこでブランチを切り替えたり
  pull したりしない（`git fetch`、読むこと、`--reference` の元にすることはよい）。
- Codex が commit か push で止まって戻ってきたら、`.claude/skills/codex-task` の「3. 戻ってきたら」に従って代行し、
  起動スクリプトの前提が崩れた原因を先に直す。回避策を Codex に考えさせない。

## トークンの使い方

- 同時に動かす Codex は 2〜3 まで。1 タスク 1 起動で、状態は Issue・PR 本文・`docs/reviews/` に残す。
- 収束の規則: Blocker/Major が出たラウンドだけ再レビューする。Minor/nit の修正は CI 通過で可。
- CI や Codex を待つ間は、`run_in_background` か `gh pr checks --watch` で待ち、途中経過を細かく報告しない。
