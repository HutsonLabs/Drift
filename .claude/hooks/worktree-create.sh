#!/usr/bin/env bash
# WorktreeCreate hook. Shared by every repo in ~/code — keep the copies identical; per-repo
# settings come from the environment set in .claude/settings.json:
#   WT_INTEGRATION  branch agent work merges into (default: main)
#   WT_COPY         gitignored files to copy from the main checkout (default: .env)
#
# Creates <main checkout>/.worktrees/<name> on branch worktree-<name>. The base is the
# integration branch: local <int> or origin/<int>, whichever contains the other, so a new
# worktree sees both work merged locally but not yet pushed and work pushed from elsewhere.
# If the two have diverged it branches from local <int> (where the merge gate lands work).
# Records the creating session on the branch (branch.<b>.claudeSession) so
# worktree-merge-gate.sh knows which session must merge it.
#
# Reads the hook JSON on stdin and prints the worktree's absolute path as the last stdout
# line; everything else goes to stderr, since Claude Code reads the path from stdout.
set -euo pipefail

input=$(cat)
name=$(jq -r '.name // empty' <<<"$input")
session=$(jq -r '.session_id // empty' <<<"$input")
[ -n "$name" ] || { echo "worktree-create: no name in hook input" >&2; exit 1; }

# The main checkout, even when Claude was started inside another worktree.
common=$(git -C "${CLAUDE_PROJECT_DIR:-$PWD}" rev-parse --path-format=absolute --git-common-dir)
root=$(dirname "$common")
int=${WT_INTEGRATION:-main}
dir="$root/.worktrees/$name"
branch="worktree-$name"

if [ -d "$dir" ]; then
  # Resuming an existing worktree.
  echo "$dir"
  exit 0
fi

GIT_SSH_COMMAND="ssh -o BatchMode=yes -o ConnectTimeout=5" \
  git -C "$root" fetch --quiet origin "$int" >/dev/null 2>&1 \
  || echo "worktree-create: fetch of origin/$int failed, using the last fetched state" >&2

base=$int
if git -C "$root" rev-parse -q --verify "refs/remotes/origin/$int" >/dev/null; then
  if git -C "$root" merge-base --is-ancestor "$int" "origin/$int"; then
    base="origin/$int"
  elif ! git -C "$root" merge-base --is-ancestor "origin/$int" "$int"; then
    echo "worktree-create: $int and origin/$int have diverged, branching from local $int" >&2
  fi
fi

mkdir -p "$(dirname "$dir")"
if git -C "$root" show-ref --verify --quiet "refs/heads/$branch"; then
  git -C "$root" worktree add "$dir" "$branch" >&2
else
  git -C "$root" worktree add --no-track -b "$branch" "$dir" "$base" >&2
fi
[ -n "$session" ] && git -C "$root" config "branch.$branch.claudeSession" "$session"

for f in ${WT_COPY-.env}; do
  if [ -f "$root/$f" ]; then cp "$root/$f" "$dir/$f"; fi
done

echo "$dir"
