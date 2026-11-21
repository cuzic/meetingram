---
name: merge-pr
description: レビューが収束した meetingram / meetingram-core の PR を安全にマージする（head の固定、収束後のコミットの確認、CI 全緑の確認、コアならタグ付けとアプリの追従）。owner に「マージして」「収束したので入れて」と頼まれたときか、review-pr で converged: yes になったときに使う。
---

# 収束した PR のマージ

Claude（または owner）が行う。main の取り込みや衝突解消は自分でせず、Codex に戻す。

## 前提（1 つでも欠けたら止める）

- 最新ラウンドのレビュー（PR ブランチの `docs/reviews/prNN-roundK.md`）が converged: yes で、
  `対象: … @ <sha>` が書かれている。docs だけの PR は、レビューの代わりに Claude が差分を読めばよい。
- PR の base が main（`gh pr view NN -R cuzic/<repo> --json baseRefName --jq .baseRefName`）。
- アプリの PR なら、`Cargo.toml` の meetingram-core 参照がすべて `tag = "…"` で、ブランチ参照が無い。

## 手順

シェルの変数は Bash の呼び出しをまたいで残らない。各ブロックは 1 回の呼び出しで実行し、
`REPO` `PR` `R` `SHA` には**リテラルの値**を書く。

1. **head を固定し、レビュー後のコミットを確かめる**（`R` はレビューした SHA）:

   ```sh
   REPO=meetingram; PR=NN; R=<reviewed sha>
   SHA=$(gh pr view "$PR" -R "cuzic/$REPO" --json headRefOid --jq .headRefOid) || exit 1; echo "SHA=$SHA"
   cd ~/src/$REPO && git fetch -q origin || exit 1
   git merge-base --is-ancestor "$R" "$SHA" || { echo "レビューした SHA が head の祖先でない"; exit 1; }
   git log --no-merges --stat --format="%h %s" "$R..$SHA" --not origin/main
   for m in $(git log --merges --format=%H "$R..$SHA" --not origin/main); do git show --remerge-diff --stat "$m"; done
   ```

   許されるのは、レビューファイルのコミットと、指摘 ID を挙げた Minor/nit の修正だけ。
   マージコミットの `--remerge-diff` は空であること。それ以外があれば `$R..$SHA` だけを再レビューする。
2. **CI 全緑を確かめる**（`gh pr merge` は CI が赤でも通るので必ず通す）。待ち時間が長いので
   `run_in_background` で実行する:

   ```sh
   REPO=meetingram; PR=NN; MIN=3; : "${PR:?}"
   gh pr checks "$PR" -R "cuzic/$REPO" --watch --interval 30 >/dev/null 2>&1
   gh pr checks "$PR" -R "cuzic/$REPO" --json name,bucket --jq '.[] | "\(.bucket) \(.name)"'
   n=$(gh pr checks "$PR" -R "cuzic/$REPO" --json name,bucket --jq '[.[] | select(.name | test("e2e-best-effort") | not)] | length') || exit 1
   bad=$(gh pr checks "$PR" -R "cuzic/$REPO" --json name,bucket --jq '[.[] | select((.name | test("e2e-best-effort") | not) and .bucket != "pass")] | length') || exit 1
   [ "$n" -ge "$MIN" ] && [ "$bad" -eq 0 ] || { echo "CI が全緑でない（pass 以外 $bad / 必須 $n）"; exit 1; }
   [ "$(gh pr view "$PR" -R "cuzic/$REPO" --json mergeable --jq .mergeable)" = MERGEABLE ] || { echo "MERGEABLE でない"; exit 1; }
   ```

   - 必須ジョブ数 `MIN`: meetingram は 3（app-build の linux・Windows・macOS）、
     meetingram-core は 3（Linux・Windows・macOS の build）。
   - `e2e-best-effort`（実機に近いキャプチャのスモークテスト）は失敗を許すジョブ。落ちていたらログを読み、
     PR が原因なら修正タスクに回す。
   - checks が 0 件、取得失敗、pending は止まる側に倒れる。少し置いてからやり直す。
   - **CI が走らない PR**: コアの workflow は `paths` で絞っているので、docs・エージェント設定・workflow ファイル
     だけの PR では checks が 0 件のままになる（アプリの app-build は絞っていないので常に走る）。このときだけ、
     `gh pr diff NN -R cuzic/<repo> --name-only` で全ファイルが `crates/**` `plugins/**` `Cargo.*` `.cargo/**`
     の外にあることを確かめてから、手順 2 を飛ばしてよい。workflow ファイルを変えた PR は、マージ後に main の
     push で走る run（`gh run list -R cuzic/<repo> -b main -L 5`）を `gh run watch <id> --exit-status` で待ち、
     赤ならすぐ直す。
3. **固定した head でマージする**:

   ```sh
   REPO=meetingram; PR=NN; SHA=<手順 1 の SHA>; : "${PR:?}" "${SHA:?}"
   gh pr merge "$PR" -R "cuzic/$REPO" --merge --match-head-commit "$SHA"
   ```

4. **積み上げ PR を付け替えてから、ブランチを消す**:

   ```sh
   for n in $(gh pr list -R cuzic/<repo> --base <branch> --json number --jq '.[].number'); do gh pr edit "$n" -R cuzic/<repo> --base main; done
   git -C ~/src/<repo> push origin --delete <branch>
   ```

5. **コアをマージしたら**: アプリに届ける変更ならタグを打つ（0.x の間は、公開 API・イベント・設定・保存形式が
   変わるなら minor、それ以外は patch）。`~/src/meetingram-core` は他のセッションも使うので pull や switch をせず、
   マージコミットに直接打つ:

   ```sh
   M=$(gh pr view NN -R cuzic/meetingram-core --json mergeCommit --jq .mergeCommit.oid) || exit 1
   git -C ~/src/meetingram-core fetch -q origin && git -C ~/src/meetingram-core tag vX.Y.Z "$M" && git -C ~/src/meetingram-core push origin vX.Y.Z
   ```

   アプリに届ける変更が無い（整形、CI、docs だけ）ならタグは打たない。
   続けて、アプリの `[workspace.dependencies]` のタグ更新と追従を `.claude/skills/codex-task` で出す。

## 報告

マージした PR、打ったタグ、付け替えた PR、出した追従タスク、残った follow-up（Minor）を 1 行ずつ。
