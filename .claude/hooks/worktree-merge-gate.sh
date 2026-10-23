#!/usr/bin/env bash
# Stop hook. Shared by every repo in ~/code — keep the copies identical; per-repo settings
# come from the environment set in .claude/settings.json:
#   WT_INTEGRATION  branch agent work merges into (default: main)
#   WT_MERGE_HINT   how to merge, shown to the agent (default: merge into local <int>)
#
# A session can't finish while a worktree it owns has work missing from local <int>: commits
# whose content isn't in <int> (plain or squash merges both count), or uncommitted changes.
# A session owns the worktree it runs in and every worktree it created (worktree-create.sh
# records branch.<b>.claudeSession), so parallel agents don't block on each other's work.
# Blocks once per stop; stop_hook_active lets the retry through as a notice, so it can't loop.
input=$(cat)
session=$(jq -r '.session_id // ""' <<<"$input")
cwd=$(jq -r '.cwd // ""' <<<"$input")
active=$(jq -r '.stop_hook_active // false' <<<"$input")

common=$(git -C "${cwd:-${CLAUDE_PROJECT_DIR:-$PWD}}" rev-parse --path-format=absolute --git-common-dir 2>/dev/null) || exit 0
root=$(dirname "$common")
int=${WT_INTEGRATION:-main}
hint=${WT_MERGE_HINT:-"merge it into local $int (in $root)"}

merged() {
  [ "$(git -C "$root" rev-list --count "$int..$1" 2>/dev/null)" = 0 ] && return 0
  local tree
  tree=$(git -C "$root" merge-tree --write-tree "$int" "$1" 2>/dev/null | head -1) || return 1
  [ "$tree" = "$(git -C "$root" rev-parse "$int^{tree}")" ]
}

pending=()
while read -r path branch; do
  # .claude/worktrees is the pre-.worktrees location, kept while old worktrees drain.
  [[ "$path" == "$root"/.worktrees/* || "$path" == "$root"/.claude/worktrees/* ]] || continue
  owner=$(git -C "$root" config "branch.$branch.claudeSession" 2>/dev/null)
  [[ "$cwd/" == "$path"/* || ( -n "$session" && "$owner" == "$session" ) ]] || continue
  why=()
  [ -n "$(git -C "$path" status --porcelain 2>/dev/null)" ] && why+=("uncommitted changes")
  merged "$branch" || why+=("$(git -C "$root" rev-list --count "$int..$branch") commit(s) not in $int")
  [ ${#why[@]} -gt 0 ] && pending+=("$branch ($(IFS=,; echo "${why[*]}" | sed 's/,/, /g')) at $path")
done < <(git -C "$root" worktree list --porcelain | awk '
  /^worktree /{p=substr($0,10)} /^branch /{sub("refs/heads/","",$2); print p, $2}')

[ ${#pending[@]} -eq 0 ] && exit 0
list=$(printf '  %s\n' "${pending[@]}")
if [ "$active" = true ]; then
  jq -n --arg m "Worktree work not yet in $int:
$list" '{systemMessage: $m}'
else
  jq -n --arg r ".claude/hooks/worktree-merge-gate.sh: your worktree work isn't in $int yet. Commit, then $hint before finishing — or, if the work isn't finished (waiting on the user, or its agents are still running), say so in one line and stop:
$list" '{decision: "block", reason: $r}'
fi
exit 0
