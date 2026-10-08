#!/usr/bin/env sh
# Generate the small test clips used by the demo show and the integration tests.
# Usage: scripts/make_test_media.sh [path/to/ffmpeg]
set -e
FF="${1:-ffmpeg}"
cd "$(dirname "$0")/../test_media"
"$FF" -hide_banner -loglevel error -y -f lavfi -i testsrc2=size=1280x720:rate=30 \
  -f lavfi -i sine=frequency=440:sample_rate=48000 -t 10 \
  -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest testsrc_10s.mp4
"$FF" -hide_banner -loglevel error -y -f lavfi -i smptehdbars=size=1920x1080:rate=25 \
  -f lavfi -i sine=frequency=660:sample_rate=48000 -t 8 \
  -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest smpte_8s.mp4
"$FF" -hide_banner -loglevel error -y -f lavfi -i color=c=red:s=640x360 -frames:v 1 color_red.png
"$FF" -hide_banner -loglevel error -y -f lavfi -i sine=frequency=330:sample_rate=48000 \
  -t 5 -ac 2 tone_5s.wav
echo "test media written to $(pwd)"
