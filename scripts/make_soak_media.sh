#!/usr/bin/env sh
# Make the media the soak show plays (see src/soak.rs and the README).
# Usage: scripts/make_soak_media.sh DIR [path/to/ffmpeg]
set -e
DIR="${1:?usage: make_soak_media.sh DIR [ffmpeg]}"
FF="${2:-ffmpeg}"
mkdir -p "$DIR"
cd "$DIR"
q="-hide_banner -loglevel error -y"
# 30 s of 1080p H.264 with a running clock and a tone.
"$FF" $q -f lavfi -i testsrc2=size=1920x1080:rate=30 \
  -f lavfi -i sine=frequency=440:sample_rate=48000 -t 30 \
  -c:v libx264 -preset veryfast -pix_fmt yuv420p -c:a aac -shortest h264_1080.mp4
# 10 s of 1080p Hap.
"$FF" $q -f lavfi -i smptehdbars=size=1920x1080:rate=30 -t 10 \
  -c:v hap -format hap hap_1080.mov
# A 6 s clip that loops.
"$FF" $q -f lavfi -i testsrc2=size=1280x720:rate=30 -t 6 \
  -c:v libx264 -preset veryfast -pix_fmt yuv420p loop_720.mp4
# A minute of music-like sound, and ten seconds of ambience.
"$FF" $q -f lavfi -i "aevalsrc=0.3*sin(2*PI*(220+40*sin(2*PI*t/7))*t)|0.3*sin(2*PI*(330+30*sin(2*PI*t/5))*t):s=48000:d=60" \
  -c:a pcm_s16le music.wav
"$FF" $q -f lavfi -i "anoisesrc=color=pink:amplitude=0.05:sample_rate=48000:duration=10" -ac 2 \
  -c:a pcm_s16le ambience.wav
# Three pictures for the slide show.
for n in 1 2 3; do
  "$FF" $q -f lavfi -i "gradients=size=1920x1080:seed=$n:duration=1:speed=0.00001" -frames:v 1 "slide$n.png"
done
echo "soak media written to $(pwd)"
