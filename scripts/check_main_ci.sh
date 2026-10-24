#!/usr/bin/env bash
# Check that CI has passed on a commit of the default branch, for Cut release.
# Usage: scripts/check_main_ci.sh <sha>     (needs GH_TOKEN and REPO)
#
# CI skips changes that only touch documentation (see ci.yml), so the tip of
# the branch may have no run of its own. Then the newest commit before it that
# does have a passing run stands in for it, provided nothing after that commit
# touched more than documentation.
set -euo pipefail

sha="${1:?usage: check_main_ci.sh <sha>}"
doc_only='\.md$|^LICENSE-'

passed() {
  [ "$(gh api "repos/$REPO/actions/workflows/ci.yml/runs?head_sha=$1&status=success&per_page=1" --jq '.total_count')" -ge 1 ]
}

if passed "$sha"; then
  echo "CI passed on $sha"
  exit 0
fi
for base in $(git rev-list --first-parent --max-count=30 "$sha"); do
  if passed "$base"; then
    other="$(git diff --name-only "$base" "$sha" | grep -v -E "$doc_only" || true)"
    if [ -n "$other" ]; then
      echo "CI has not passed on $sha, and changes since $base (the last commit CI passed on) go beyond documentation:" >&2
      echo "$other" >&2
      exit 1
    fi
    echo "CI passed on $base; only documentation changed since"
    exit 0
  fi
done
echo "CI has not passed on $sha or any of the 30 commits before it" >&2
exit 1
