#!/usr/bin/env bash
# Check a downloaded FFmpeg archive against the checksum pinned for it in
# scripts/ffmpeg_pins.txt (PINS overrides the file).
# Usage: scripts/check_ffmpeg_pin.sh <file> <asset name>
set -euo pipefail

file="${1:?file}"
asset="${2:?asset name}"
pins="${PINS:-$(dirname "$0")/ffmpeg_pins.txt}"

want="$(awk -v a="$asset" '!/^#/ && $2 == a { print $1 }' "$pins")"
if [ -z "$want" ]; then
  echo "No checksum is pinned for $asset in $pins" >&2
  exit 1
fi
have="$(sha256sum "$file" | cut -d' ' -f1)"
if [ "$have" != "$want" ]; then
  cat >&2 <<MSG
$asset does not match its pinned checksum:
  pinned:     $want
  downloaded: $have
BtbN has published a new build (or the download was altered). Look at what
changed, then run scripts/update_ffmpeg_pins.sh and commit scripts/ffmpeg_pins.txt.
MSG
  exit 1
fi
echo "$asset matches its pinned checksum"
