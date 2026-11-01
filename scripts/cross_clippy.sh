#!/usr/bin/env bash
# Clippy for the Windows and Mac targets from a Linux machine (a cloud session).
#
# `ring` runs a C compiler for the target in its build script, which a Linux
# machine doesn't have. For `cargo check` and `clippy` nothing is linked, so
# the compiler can be a stand-in that succeeds and writes nothing: the Rust of
# the whole program is then checked for the target, cfg(windows) and
# cfg(target_os = "macos") code included. (Not for building or testing.)
#
#   scripts/cross_clippy.sh            both targets
#   scripts/cross_clippy.sh mac        one of them (mac | windows)
#
# Uses its own target folders under $TMPDIR (each is about a gigabyte).
set -euo pipefail
cd "$(dirname "$0")/.."
dir="${TMPDIR:-/tmp}/newq-cross"
mkdir -p "$dir/fake"
cat > "$dir/fake/cc" <<'SH'
#!/bin/sh
exit 0
SH
cat > "$dir/fake/ar" <<'SH'
#!/bin/sh
for a in "$@"; do
  case "$a" in
    -out:*|-OUT:*|/OUT:*|/out:*) f="${a#*:}"; : > "$f";;
  esac
done
case "$1" in cq|crs|cqs|rcs|cr) : > "$2";; esac
exit 0
SH
chmod +x "$dir/fake/cc" "$dir/fake/ar"

run() { # target, env-suffix, folder
  echo "== clippy for $1"
  env "CC_$2=$dir/fake/cc" "AR_$2=$dir/fake/ar" CARGO_TARGET_DIR="$dir/$3" \
    cargo clippy --target "$1" --all-targets -- -D warnings
}

want="${1:-all}"
if [ "$want" = all ] || [ "$want" = mac ]; then
  run aarch64-apple-darwin aarch64_apple_darwin mac
fi
if [ "$want" = all ] || [ "$want" = windows ]; then
  run x86_64-pc-windows-msvc x86_64_pc_windows_msvc windows
fi
