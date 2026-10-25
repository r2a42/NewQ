#!/usr/bin/env bash
# Re-pin the FFmpeg builds bundled with the Windows and Linux releases to what
# BtbN currently publishes, and show what changed. Check the new build first
# (its changelog, and a release dry run: package_release.sh tests the
# decoders and capture devices NewQ needs), then commit scripts/ffmpeg_pins.txt.
# Usage: scripts/update_ffmpeg_pins.sh
set -euo pipefail

cd "$(dirname "$0")"
branch="${FFMPEG_BRANCH:-n9.0}"
version="${FFMPEG_VERSION:-9.0}"
assets=(
  "ffmpeg-${branch}-latest-win64-lgpl-${version}.zip"
  "ffmpeg-${branch}-latest-linux64-lgpl-${version}.tar.xz"
)
sums="$(curl -fsSL "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/checksums.sha256")"
new="$(mktemp)"
grep '^#' ffmpeg_pins.txt > "$new"
for a in "${assets[@]}"; do
  line="$(awk -v a="$a" '$2 == a' <<< "$sums")"
  if [ -z "$line" ]; then
    echo "BtbN publishes no checksum for $a" >&2
    exit 1
  fi
  echo "$line" >> "$new"
done
diff -u ffmpeg_pins.txt "$new" || true
cp "$new" ffmpeg_pins.txt
rm -f "$new"
