#!/bin/bash
# Gets a Claude Code cloud session ready to build and test NewQ: the system
# packages CI installs, the crates, and the generated test media. Runs only in
# cloud sessions, and is safe to run again.
set -euo pipefail

if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

cd "${CLAUDE_PROJECT_DIR:-$(git rev-parse --show-toplevel)}"

SUDO=""
if [ "$(id -u)" -ne 0 ]; then
  SUDO="sudo"
fi

# ALSA, udev and xkbcommon headers for cpal/winit, and ffmpeg for the tests.
need=()
for pkg in libasound2-dev libudev-dev libxkbcommon-dev pkg-config ffmpeg; do
  if ! dpkg -s "$pkg" > /dev/null 2>&1; then
    need+=("$pkg")
  fi
done
if [ "${#need[@]}" -gt 0 ]; then
  $SUDO apt-get update -qq
  $SUDO env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq "${need[@]}"
fi

# Fetch the Linux crates now so the first build doesn't wait on the network.
cargo fetch --locked --target x86_64-unknown-linux-gnu

# The integration tests decode these (they're git-ignored). Generated once.
if [ ! -f test_media/testsrc_10s.mp4 ]; then
  sh scripts/make_test_media.sh
fi

# Make missing ffmpeg or test media fail the tests instead of skipping them.
if [ -n "${CLAUDE_ENV_FILE:-}" ]; then
  echo 'export NEWQ_REQUIRE_MEDIA=1' >> "$CLAUDE_ENV_FILE"
fi
