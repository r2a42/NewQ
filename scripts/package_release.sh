#!/usr/bin/env bash
# Package a release build into dist/.
# Usage: scripts/package_release.sh <windows-x64|linux-x64|macos-universal> <tag>
# Expects `cargo build --release` output in target/release/. Windows and Linux
# packages bundle an LGPL FFmpeg build from BtbN/FFmpeg-Builds
# (FFMPEG_BRANCH / FFMPEG_VERSION, default n9.0 / 9.0).
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

# Download an FFmpeg build and put ffmpeg/ffprobe plus licence info in $stage/ffmpeg.
bundle_ffmpeg() {
  local asset="$1" exe_suffix="$2"
  local url="https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/${asset}"
  local tmp
  tmp="$(mktemp -d)"
  echo "Downloading ${url}"
  curl -fsSL -o "${tmp}/${asset}" "$url"
  case "$asset" in
    *.zip) (cd "$tmp" && unzip -q "$asset") ;;
    *.tar.xz) (cd "$tmp" && tar xJf "$asset") ;;
  esac
  local bin
  bin="$(dirname "$(find "$tmp" -type f -name "ffmpeg${exe_suffix}" | head -1)")"
  mkdir -p "$stage/ffmpeg"
  cp "$bin/ffmpeg${exe_suffix}" "$bin/ffprobe${exe_suffix}" "$stage/ffmpeg/"
  cp "$(find "$tmp" -maxdepth 2 -name 'LICENSE*' | head -1)" "$stage/ffmpeg/LICENSE.txt"
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
    mkdir -p "$app/Contents/MacOS"
    cp target/release/newq "$app/Contents/MacOS/newq"
    chmod +x "$app/Contents/MacOS/newq"
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
</dict>
</plist>
EOF
    (cd dist/stage && ditto -c -k --keepParent "$name" "${root}/dist/${name}.zip")
    ;;
  *)
    echo "unknown platform: $platform" >&2
    exit 1
    ;;
esac

rm -rf dist/stage
ls -l dist
