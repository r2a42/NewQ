#!/usr/bin/env bash
# Package a release build into dist/.
# Usage: scripts/package_release.sh <windows-x64|linux-x64|macos-universal> <tag>
# Expects `cargo build --release` output in target/release/. Every package
# bundles an LGPL FFmpeg: Windows and Linux use BtbN/FFmpeg-Builds
# (FFMPEG_BRANCH / FFMPEG_VERSION, default n9.0 / 9.0), checked against the
# checksums in scripts/ffmpeg_pins.txt; macOS uses the build
# from scripts/build_ffmpeg_macos.sh found in $FFMPEG_MACOS_DIR.
set -euo pipefail

platform="$1"
tag="$2"
name="NewQ-${tag}-${platform}"
ffbranch="${FFMPEG_BRANCH:-n9.0}"
ffversion="${FFMPEG_VERSION:-9.0}"

cd "$(dirname "$0")/.."
root="$(pwd)"
rm -rf dist
stage="dist/stage/${name}"
mkdir -p "$stage"
cp README.md LICENSE-MIT LICENSE-APACHE "$stage/"

# Fail the release if a bundled FFmpeg lacks what NewQ needs.
verify_ffmpeg() {
  local ff="$1" probe="$2"
  "$probe" -hide_banner -version > /dev/null
  local decoders
  decoders="$("$ff" -hide_banner -decoders)"
  for d in h264 hevc prores vp9 mpeg4 mpeg2video dnxhd mjpeg png aac mp3 flac pcm_s16le pcm_s24le; do
    if ! grep -qE "^ [A-Z.]{6} ${d} " <<< "$decoders"; then
      echo "bundled ffmpeg is missing the ${d} decoder" >&2
      exit 1
    fi
  done
  # Camera cues open the platform's capture device through ffmpeg.
  local indev
  case "$platform" in
    windows-x64) indev=dshow ;;
    linux-x64) indev=v4l2 ;;
    macos-universal) indev=avfoundation ;;
  esac
  if ! "$ff" -hide_banner -devices | grep -E "^ D" | grep -qw "$indev"; then
    echo "bundled ffmpeg cannot capture from ${indev} (camera cues need it)" >&2
    exit 1
  fi
  # Run the same kinds of commands NewQ does (see src/media/*.rs).
  local q=(-hide_banner -loglevel error -nostdin)
  "$ff" "${q[@]}" -f lavfi -i testsrc2=size=320x240:rate=25 -t 1 -map 0:v:0 -an     -vf "scale=160:120:flags=bicubic:out_range=tv,format=yuv420p" -fps_mode cfr -r 25     -f rawvideo -pix_fmt yuv420p pipe:1 > /dev/null
  "$ff" "${q[@]}" -f lavfi -i testsrc2=size=320x240 -frames:v 1 -vf scale=64:48:flags=bicubic     -f rawvideo -pix_fmt rgba pipe:1 > /dev/null
  "$ff" "${q[@]}" -f lavfi -i sine=frequency=440 -t 1 -map 0:a:0 -vn -ac 2 -ar 48000     -f f32le pipe:1 > /dev/null
  echo "bundled ffmpeg OK: $("$ff" -hide_banner -version | head -1)"
}

# Download an FFmpeg build and put ffmpeg/ffprobe plus licence info in $stage/ffmpeg.
bundle_ffmpeg() {
  local asset="$1" exe_suffix="$2"
  local url="https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/${asset}"
  local tmp
  tmp="$(mktemp -d)"
  echo "Downloading ${url}"
  curl -fsSL -o "${tmp}/${asset}" "$url"
  # The moving "latest" build is only used if it is the one that was looked at.
  "${root}/scripts/check_ffmpeg_pin.sh" "${tmp}/${asset}" "$asset"
  case "$asset" in
    *.zip) (cd "$tmp" && unzip -q "$asset") ;;
    *.tar.xz) (cd "$tmp" && tar xJf "$asset") ;;
  esac
  local bin
  bin="$(dirname "$(find "$tmp" -type f -name "ffmpeg${exe_suffix}" | head -1)")"
  mkdir -p "$stage/ffmpeg"
  cp "$bin/ffmpeg${exe_suffix}" "$bin/ffprobe${exe_suffix}" "$stage/ffmpeg/"
  cp "$(find "$tmp" -maxdepth 2 -name 'LICENSE*' | head -1)" "$stage/ffmpeg/LICENSE.txt"
  verify_ffmpeg "$stage/ffmpeg/ffmpeg${exe_suffix}" "$stage/ffmpeg/ffprobe${exe_suffix}"
  local version
  version="$("$stage/ffmpeg/ffmpeg${exe_suffix}" -hide_banner -version | head -1)"
  cat > "$stage/ffmpeg/SOURCE.txt" <<EOF
These ffmpeg and ffprobe executables are unmodified LGPL builds of FFmpeg from
https://github.com/BtbN/FFmpeg-Builds (asset: ${asset}).

${version}

FFmpeg is licensed under the GNU Lesser General Public License v2.1 or later;
see LICENSE.txt. NewQ runs these programs as separate processes and does not
link to them. Corresponding source code:
  FFmpeg:        https://github.com/FFmpeg/FFmpeg/tree/release/${ffversion}
  Build scripts: https://github.com/BtbN/FFmpeg-Builds
You may replace these files with any other FFmpeg build.
EOF
  rm -rf "$tmp"
}

case "$platform" in
  windows-x64)
    cp target/release/newq.exe "$stage/"
    bundle_ffmpeg "ffmpeg-${ffbranch}-latest-win64-lgpl-${ffversion}.zip" ".exe"
    (cd dist/stage && 7z a -tzip -bso0 "${root}/dist/${name}.zip" "$name")
    ;;
  linux-x64)
    cp target/release/newq "$stage/"
    chmod +x "$stage/newq"
    bundle_ffmpeg "ffmpeg-${ffbranch}-latest-linux64-lgpl-${ffversion}.tar.xz" ""
    tar -C dist/stage -czf "dist/${name}.tar.gz" "$name"
    ;;
  macos-universal)
    app="$stage/NewQ.app"
    ffdir="${FFMPEG_MACOS_DIR:?set FFMPEG_MACOS_DIR to the output of scripts/build_ffmpeg_macos.sh}"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/FFmpeg"
    cp target/release/newq "$app/Contents/MacOS/newq"
    # NewQ looks for ffmpeg/ffprobe next to its own executable.
    cp "$ffdir/ffmpeg" "$ffdir/ffprobe" "$app/Contents/MacOS/"
    chmod +x "$app/Contents/MacOS/"*
    verify_ffmpeg "$app/Contents/MacOS/ffmpeg" "$app/Contents/MacOS/ffprobe"
    cp "$ffdir/LICENSE.txt" "$app/Contents/Resources/FFmpeg/LICENSE.txt"
    cat > "$app/Contents/Resources/FFmpeg/SOURCE.txt" <<EOF
The ffmpeg and ffprobe executables in NewQ.app/Contents/MacOS are LGPL builds
of FFmpeg, compiled from the unmodified release source by
scripts/build_ffmpeg_macos.sh in the NewQ repository (no external libraries).

$(cat "$ffdir/VERSION.txt")

FFmpeg is licensed under the GNU Lesser General Public License v2.1 or later;
see LICENSE.txt. NewQ runs these programs as separate processes and does not
link to them. Corresponding source code:
  https://ffmpeg.org/releases/ffmpeg-$(cut -d' ' -f3 "$ffdir/VERSION.txt").tar.xz
EOF
    mkdir -p "$stage/FFmpeg"
    cp "$app/Contents/Resources/FFmpeg/"* "$stage/FFmpeg/"
    cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>NewQ</string>
  <key>CFBundleDisplayName</key><string>NewQ</string>
  <key>CFBundleIdentifier</key><string>io.github.r2a42.newq</string>
  <key>CFBundleExecutable</key><string>newq</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${tag#v}</string>
  <key>CFBundleVersion</key><string>${tag#v}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSCameraUsageDescription</key><string>NewQ shows live camera pictures in camera cues.</string>
</dict>
</plist>
EOF
    # Apple Silicon only runs signed code: ad-hoc sign the helpers and the app.
    codesign --force --sign - "$app/Contents/MacOS/ffmpeg" "$app/Contents/MacOS/ffprobe"
    codesign --force --sign - "$app"
    codesign --verify --deep --strict "$app"
    (cd dist/stage && ditto -c -k --keepParent "$name" "${root}/dist/${name}.zip")
    ;;
  *)
    echo "unknown platform: $platform" >&2
    exit 1
    ;;
esac

rm -rf dist/stage
ls -l dist
