#!/bin/sh
# Codex を 1 タスク分、専用の clone で非対話に動かす。
#
# usage: run-codex.sh <repo> <branch> <label> <prompt-file> [base]
#   repo         meetingram | meetingram-core
#   branch       作業ブランチ。origin にあればそれを checkout し、無ければ origin/<base> から作る
#   label        ログと clone の名前（英数字とハイフン）
#   prompt-file  Codex への指示（数行。本体は Issue / PR コメントに書く）
#   base         新しいブランチの起点（既定 main）
#
# env:
#   CODEX_BIN      codex の実行ファイル（既定 codex）
#   CODEX_WORKDIR  clone とログの置き場（既定 ${TMPDIR:-/tmp}/meetingram-codex）。repo の外にする
#
# Codex の sandbox は書き込み可能な場所の中の .git を読み取り専用にする。そこでタスクごとに clone し、
# git のディレクトリを作業ツリーの外（$w/git-<label>）に分けて --add-dir で書けるようにする。
# clone は ssh で取る（https だと gh の OAuth トークンになり、workflow スコープが無いと
# .github/workflows を含む push が拒否される）。
# 出力: 最後に exit コード、clone・ログ・最終メッセージの場所、最終メッセージの末尾を表示する。
set -u
[ $# -ge 4 ] || { echo "usage: $0 <repo> <branch> <label> <prompt-file> [base]" >&2; exit 2; }
repo=$1; br=$2; label=$3; prompt=$4; base=${5:-main}
case $repo in meetingram|meetingram-core) ;; *) echo "unknown repo: $repo" >&2; exit 2 ;; esac
[ -f "$prompt" ] || { echo "prompt file not found: $prompt" >&2; exit 2; }

w=${CODEX_WORKDIR:-${TMPDIR:-/tmp}/meetingram-codex}
d="$w/cx-$label"
mkdir -p "$w" || exit 1
g="$w/git-$label"
rm -rf "$d" "$g"

if [ -d "$HOME/src/$repo/.git" ]; then
  git clone -q --reference "$HOME/src/$repo" --separate-git-dir "$g" "git@github.com:cuzic/$repo.git" "$d" || exit 1
else
  git clone -q --separate-git-dir "$g" "git@github.com:cuzic/$repo.git" "$d" || exit 1
fi
if git -C "$d" ls-remote --exit-code --heads origin "$br" >/dev/null 2>&1; then
  git -C "$d" switch -q "$br" || exit 1
else
  git -C "$d" switch -q --no-track -c "$br" "origin/$base" || exit 1
fi

"${CODEX_BIN:-codex}" exec -s workspace-write \
  -c sandbox_workspace_write.network_access=true \
  --add-dir "$HOME/.cargo" --add-dir "$g" \
  -C "$d" -o "$w/codex-$label.last.md" - < "$prompt" > "$w/codex-$label.log" 2>&1
rc=$?

echo "exit=$rc label=$label"
echo "clone=$d"
echo "log=$w/codex-$label.log"
echo "last=$w/codex-$label.last.md"
tail -c 3000 "$w/codex-$label.last.md" 2>/dev/null
exit $rc
