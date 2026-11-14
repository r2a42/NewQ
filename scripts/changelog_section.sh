#!/usr/bin/env bash
# Print the section of CHANGELOG.md for a release tag (v1.2.3 or v1.2.3-rc1 both
# print the section "## 1.2.3"), without its heading. Fails if there is no such
# section or it says nothing: a release must tell people what changed.
# Usage: scripts/changelog_section.sh <tag>      (CHANGELOG overrides the file)
set -euo pipefail

tag="${1:-}"
if [[ ! "$tag" =~ ^v([0-9]+\.[0-9]+\.[0-9]+)(-[0-9A-Za-z.]+)?$ ]]; then
  echo "'$tag' is not a release tag like v1.2.3 or v1.2.3-rc1" >&2
  exit 1
fi
version="${BASH_REMATCH[1]}"
file="${CHANGELOG:-CHANGELOG.md}"

# The lines after "## <version>" up to the next "## " heading.
section="$(awk -v want="## $version" '
  $0 == want { on = 1; next }
  on && /^## / { exit }
  on { print }
' "$file")"

if ! grep -q '[^[:space:]]' <<< "$section"; then
  echo "$file has no section '## $version' (or it is empty): say what changed before releasing $tag" >&2
  exit 1
fi
# Without the blank lines around it.
sed -e '/./,$!d' <<< "$section" | sed -e ':a' -e '/^\n*$/{$d;N;ba' -e '}'
