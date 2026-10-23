#!/usr/bin/env bash
# Check that a release tag is well formed and matches the version in
# Cargo.toml: v1.2.3 or v1.2.3-rc1 for version 1.2.3.
# Usage: scripts/check_release_tag.sh <tag>      (CARGO_TOML overrides the file)
set -euo pipefail

tag="${1:-}"
if [[ ! "$tag" =~ ^v([0-9]+\.[0-9]+\.[0-9]+)(-[0-9A-Za-z.]+)?$ ]]; then
  echo "'$tag' is not a release tag like v1.2.3 or v1.2.3-rc1" >&2
  exit 1
fi
want="${BASH_REMATCH[1]}"
have="$(sed -n 's/^version = "\(.*\)"$/\1/p' "${CARGO_TOML:-Cargo.toml}" | head -n1)"
if [ "$have" != "$want" ]; then
  echo "Cargo.toml is version '$have', but $tag is for $want" >&2
  exit 1
fi
echo "$tag matches Cargo.toml ($have)"
