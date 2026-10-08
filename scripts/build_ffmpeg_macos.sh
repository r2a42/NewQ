#!/usr/bin/env bash
# Build a self-contained, LGPL, universal (arm64 + x86_64) ffmpeg and ffprobe
# for bundling inside NewQ.app. Run on macOS (Apple Silicon or Intel) with
# Xcode command line tools and nasm (`brew install nasm`).
#
# Usage: scripts/build_ffmpeg_macos.sh <output-dir>
# Output: <output-dir>/{ffmpeg,ffprobe,LICENSE.txt,VERSION.txt}
set -euo pipefail

VERSION="9.0.2"
# sha256 of https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz
# (its .asc verified against the FFmpeg release key FCF986EA15E6E293A5644F10B4322F04D67658D8).
SHA256="8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e"
MIN_MACOS="11.0"

out="$(mkdir -p "$1" && cd "$1" && pwd)"
work="$(mktemp -d)"
# Keep the exit status: macOS's bash 3.2 otherwise reports the trap's status.
trap 'rc=$?; rm -rf "$work"; exit $rc' EXIT
cd "$work"

curl -fsSLO "https://ffmpeg.org/releases/ffmpeg-${VERSION}.tar.xz"
echo "${SHA256}  ffmpeg-${VERSION}.tar.xz" | shasum -a 256 -c -
tar xJf "ffmpeg-${VERSION}.tar.xz"
src="$work/ffmpeg-${VERSION}"
jobs="$(sysctl -n hw.ncpu)"

build_arch() {
  local arch="$1"
  local dir="$work/build-$arch"
  mkdir -p "$dir"
  cd "$dir"
  # A plain string, not an array: bash 3.2 with `set -u` rejects empty arrays.
  local cross=""
  if [ "$arch" != "$(uname -m)" ]; then
    cross="--enable-cross-compile --target-os=darwin"
  fi
  # No external libraries and no GPL/nonfree parts: FFmpeg's own decoders,
  # demuxers and filters cover everything NewQ uses. --disable-autodetect
  # stops it picking up Homebrew libraries that wouldn't exist on users' Macs.
  if ! "$src/configure" \
    --arch="$arch" $cross \
    --cc="clang -arch $arch" \
    --extra-cflags="-mmacosx-version-min=${MIN_MACOS}" \
    --extra-ldflags="-arch $arch -mmacosx-version-min=${MIN_MACOS}" \
    --disable-autodetect --enable-zlib --enable-videotoolbox \
    --enable-static --disable-shared \
    --disable-doc --disable-debug --disable-ffplay --disable-network \
    --disable-encoders --enable-encoder=rawvideo,pcm_f32le,pcm_s16le,png,mpeg4,aac \
    --disable-muxers --enable-muxer=rawvideo,f32le,s16le,null,image2,mp4,mov,wav \
    > configure.log 2>&1; then
    tail -40 configure.log ffbuild/config.log
    exit 1
  fi
  grep -E "^(threading support|ARCH)" configure.log || true
  if ! make -j"$jobs" > make.log 2>&1; then
    tail -60 make.log
    exit 1
  fi
  cd "$work"
}

build_arch arm64
build_arch x86_64

for tool in ffmpeg ffprobe; do
  lipo -create -output "$out/$tool" "$work/build-arm64/$tool" "$work/build-x86_64/$tool"
  chmod +x "$out/$tool"
done
cp "$src/COPYING.LGPLv2.1" "$out/LICENSE.txt"
"$out/ffmpeg" -hide_banner -version | head -1 > "$out/VERSION.txt"
lipo -info "$out/ffmpeg"
cat "$out/VERSION.txt"
