#!/usr/bin/env bash
# Writes the release page's text to stdout: this version's CHANGELOG section,
# then .github/release_notes.md with the tag filled in and the macOS paragraph
# that matches how the Mac download was built.
#   release_notes.sh vX.Y.Z [mac_unsigned]   (mac_unsigned: "true" or anything else)
set -euo pipefail
tag="${1:?tag}"
unsigned="${2:-false}"
if [ "$unsigned" = "true" ]; then drop="mac-signed"; else drop="mac-adhoc"; fi
echo "## What's new"
echo
"$(dirname "$0")/changelog_section.sh" "$tag"
echo
sed -e "s/{{TAG}}/$tag/g" \
    -e "/^<!--$drop-->$/,/^<!--\/$drop-->$/d" \
    -e '/^<!--\/\{0,1\}mac-[a-z]*-->$/d' \
    .github/release_notes.md
