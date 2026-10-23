#!/usr/bin/env bash
# WorktreeRemove hook. Shared by every repo in ~/code — keep the copies identical; per-repo
# settings come from the environment set in .claude/settings.json:
#   WT_INTEGRATION  branch agent work merges into (default: main)
#
# Removes a worktree made by worktree-create.sh, and its branch once the branch's content is
# in the integration branch (a plain or squash merge both count). Keeps the worktree (exit 1)
# when it has uncommitted or untracked changes, and keeps an unmerged branch, so work is
# never lost. Reads the hook JSON ({"worktree_path": "<abs path>", ...}) on stdin.
set -uo pipefail

path=$(jq -r '.worktree_path // empty')
[ -n "$path" ] || { echo "worktree-remove: no worktree_path in hook input" >&2; exit 1; }
[ -d "$path" ] || exit 0

common=$(git -C "$path" rev-parse --path-format=absolute --git-common-dir) || exit 1
root=$(dirname "$common")
int=${WT_INTEGRATION:-main}
branch=$(git -C "$path" branch --show-current 2>/dev/null)

# Merged = merging the branch into <int> would change nothing.
merged() {
  [ "$(git -C "$root" rev-list --count "$int..$1" 2>/dev/null)" = 0 ] && return 0
  local tree
  tree=$(git -C "$root" merge-tree --write-tree "$int" "$1" 2>/dev/null | head -1) || return 1
  [ "$tree" = "$(git -C "$root" rev-parse "$int^{tree}")" ]
}

if [ -n "$(git -C "$path" status --porcelain 2>/dev/null)" ]; then
  echo "worktree-remove: $path has uncommitted changes, kept" >&2
  exit 1
fi

git -C "$root" worktree remove "$path" >&2 || exit 1
# Names may contain "/"; drop the parent directories they leave empty.
find "$root/.worktrees" -mindepth 1 -type d -empty -delete 2>/dev/null
if [ -n "$branch" ]; then
  if merged "$branch"; then
    git -C "$root" branch -D "$branch" >&2
  else
    echo "worktree-remove: branch $branch is not merged into $int, kept" >&2
  fi
fi
exit 0
