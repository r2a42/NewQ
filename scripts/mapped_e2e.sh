#!/usr/bin/env bash
# A mapped projector's display from end to end, by the real app, headless:
# writes a show (a known picture on a canvas, two mapped projectors on one
# curved screen in the Proscenium sample, the left one's display and the
# canvas recorded through the stream path), runs NewQ on it under Xvfb for a
# while, then compares a late frame of each recording with what
# lensmap::reference says (stage_live_e2e_tests.rs). Linux, with xvfb-run,
# ffmpeg and a Vulkan driver (lavapipe will do).
#
#   scripts/mapped_e2e.sh [DIR] [SECONDS]
#
# DIR defaults to a new folder under /tmp; the numbers are in DIR/comparison.txt
# and a screenshot of the main window in DIR/shot.png.
set -euo pipefail
cd "$(dirname "$0")/.."
DIR=${1:-$(mktemp -d /tmp/newq-mapped-e2e.XXXXXX)}
SECS=${2:-20}
mkdir -p "$DIR"
DIR=$(cd "$DIR" && pwd)
export WGPU_BACKEND=${WGPU_BACKEND:-vulkan}
CARGO_ARGS=${CHECK_CARGO_ARGS:-}
export NEWQ_MAPPED_E2E="$DIR"

# shellcheck disable=SC2086
cargo test $CARGO_ARGS -- --ignored --exact stage_live::e2e_tests::write_a_mapped_recording_show
# shellcheck disable=SC2086
cargo build $CARGO_ARGS
BIN=${CARGO_TARGET_DIR:-target}/debug/newq

rm -rf "$DIR/rec-mapped"/* "$DIR/rec-canvas"/*
NEWQ_MUTE=1 NEWQ_AUTOSTART=2 NEWQ_NODE_DIR="$DIR/node" xvfb-run -a -s "-screen 0 1500x900x24" \
  sh -c "'$BIN' '$DIR/mapped.newq' > '$DIR/newq.out' 2>&1 & pid=\$!; sleep $SECS; \
         import -window root '$DIR/shot.png' || true; kill -INT \$pid; sleep 3; kill \$pid 2>/dev/null; wait \$pid || true"

ls -l "$DIR/rec-mapped" "$DIR/rec-canvas"
# shellcheck disable=SC2086
cargo test $CARGO_ARGS -- --ignored --exact --nocapture \
  stage_live::e2e_tests::a_recorded_mapped_display_shows_what_the_reference_says
cat "$DIR/comparison.txt"
