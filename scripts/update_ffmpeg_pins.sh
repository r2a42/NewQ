#!/usr/bin/env bash
# Point the pins at one of BtbN's dated builds (https://github.com/BtbN/
# FFmpeg-Builds/releases, tags like autobuild-2026-10-25-13-00). This only
# rewrites scripts/ffmpeg_pins.txt from the checksums BtbN publishes for that
# build; then run the "Mirror FFmpeg" workflow to copy the files into this
# repository, and a release dry run to see that the build does what NewQ needs.
# Usage: scripts/update_ffmpeg_pins.sh <BtbN tag>
set -euo pipefail

tag="${1:?BtbN build tag, like autobuild-2026-10-25-13-00}"
if [[ ! "$tag" =~ ^autobuild-[0-9]{4}-[0-9]{2}-[0-9]{2}-[0-9]{2}-[0-9]{2}$ ]]; then
  echo "'$tag' is not a dated BtbN build tag like autobuild-2026-10-25-13-00" >&2
  exit 1
fi
cd "$(dirname "$0")"
branch="${FFMPEG_BRANCH:-n9.0}"
version="${FFMPEG_VERSION:-9.0}"
re_branch="${branch//./\\.}"
re_version="${version//./\\.}"
sums="$(curl -fsSL "https://github.com/BtbN/FFmpeg-Builds/releases/download/${tag}/checksums.sha256")"

pick() { # <platform> <extension regex>
  awk -v re="^ffmpeg-${re_branch}(\\\\.[0-9]+)?-[0-9]+-g[0-9a-f]+-$1-lgpl-${re_version}\\\\.$2\$" '$2 ~ re' <<< "$sums"
}
win="$(pick win64 zip)"
linux="$(pick linux64 'tar\.xz')"
if [ -z "$win" ] || [ -z "$linux" ] || [ "$(wc -l <<< "$win")" != 1 ] || [ "$(wc -l <<< "$linux")" != 1 ]; then
  echo "Expected exactly one Windows and one Linux LGPL $branch build in $tag" >&2
  exit 1
fi
name_of() { sed -E 's/^ffmpeg-(.*)-(win64|linux64)-lgpl-.*/\1/' <<< "$(awk '{print $2}' <<< "$1")"; }
ver="$(name_of "$win")"
if [ "$ver" != "$(name_of "$linux")" ]; then
  echo "The Windows build ($ver) and the Linux build ($(name_of "$linux")) are of different FFmpeg commits" >&2
  exit 1
fi

new="$(mktemp)"
sed -n '1,/^# then run the "Mirror FFmpeg"/p' ffmpeg_pins.txt > "$new"
{
  echo "btbn $tag"
  echo "release ffmpeg-$ver"
  echo "$win"
  echo "$linux"
} >> "$new"
diff -u ffmpeg_pins.txt "$new" || true
cp "$new" ffmpeg_pins.txt
rm -f "$new"
echo "Pinned FFmpeg $ver from $tag. Next: run the Mirror FFmpeg workflow."
