#!/usr/bin/env bash
# Copy the FFmpeg build named in scripts/ffmpeg_pins.txt from BtbN's release
# into a release of this repository, which is where package_release.sh gets
# it. Every file is checked against the pinned SHA-256 first. Safe to run
# again: a release that is already there is checked, not replaced.
# Usage: scripts/mirror_ffmpeg.sh [--check]   (--check downloads and verifies only)
# Needs GH_TOKEN and GITHUB_REPOSITORY (and GITHUB_SHA) unless --check.
set -euo pipefail

cd "$(dirname "$0")/.."
pins=scripts/ffmpeg_pins.txt
btbn="$(awk '$1 == "btbn" { print $2 }' "$pins")"
release="$(awk '$1 == "release" { print $2 }' "$pins")"
mapfile -t assets < <(awk '!/^#/ && NF == 2 && $1 ~ /^[0-9a-f]{64}$/ { print $2 }' "$pins")
if [ -z "$btbn" ] || [ -z "$release" ] || [ "${#assets[@]}" -lt 1 ]; then
  echo "$pins names no build" >&2
  exit 1
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
for a in "${assets[@]}"; do
  echo "Downloading $a from BtbN ($btbn)"
  curl -fsSL -o "$work/$a" "https://github.com/BtbN/FFmpeg-Builds/releases/download/${btbn}/${a}"
  scripts/check_ffmpeg_pin.sh "$work/$a" "$a"
done
(cd "$work" && sha256sum "${assets[@]}" > SHA256SUMS.txt)

if [ "${1:-}" = "--check" ]; then
  echo "Every pinned file matches (nothing was uploaded)."
  exit 0
fi

repo="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY}"
if gh release view "$release" --repo "$repo" > /dev/null 2>&1; then
  echo "$release already exists: checking what it holds"
  have="$(mktemp -d)"
  gh release download "$release" --repo "$repo" --dir "$have" --pattern 'SHA256SUMS.txt'
  if ! diff -q "$have/SHA256SUMS.txt" "$work/SHA256SUMS.txt" > /dev/null; then
    echo "The existing $release holds different files from the pins. It is not replaced; pin a new build instead." >&2
    exit 1
  fi
  rm -rf "$have"
  echo "It holds exactly the pinned files."
  exit 0
fi

ver="${release#ffmpeg-}"
commit="${ver##*-g}"
cat > "$work/notes.md" <<NOTES
A copy of the LGPL FFmpeg build bundled with NewQ's Windows and Linux downloads,
kept here so the build NewQ ships can't change or disappear upstream.

- Build: ${ver}, from https://github.com/BtbN/FFmpeg-Builds/releases/tag/${btbn}
- FFmpeg source: https://github.com/FFmpeg/FFmpeg/commit/${commit}
- Build scripts: https://github.com/BtbN/FFmpeg-Builds/tree/${btbn}

FFmpeg is licensed under the GNU Lesser General Public License v2.1 or later.
The files are unmodified. This release is not a NewQ release.
NOTES
# Not the "latest" release, and not a tag the Release workflow builds (v*).
gh release create "$release" --repo "$repo" --title "FFmpeg ${ver} (mirror)" \
  --notes-file "$work/notes.md" --prerelease --latest=false --target "${GITHUB_SHA:-main}"
gh release upload "$release" --repo "$repo" "$work/SHA256SUMS.txt" "${assets[@]/#/$work/}"
echo "Mirrored ${#assets[@]} files into release $release"
