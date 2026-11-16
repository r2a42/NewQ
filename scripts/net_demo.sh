#!/usr/bin/env bash
# A show network on one computer, for developers: a primary ("Booth") and
# display computers ("Left", "Right", ...), each NewQ with folders of its own,
# already joined to the primary's network, each on its own Xvfb screen (Linux).
# With --test the primary runs the network test by itself once they are in
# step (NEWQ_NET_TEST_AUTOSTART) and the script prints where the zip is.
#
#   scripts/net_demo.sh [--test] [displays] [path/to/newq]
#
# Needs: Xvfb, python3, a built NewQ (default target/debug/newq). Screens are
# :91 (the primary), :92, :93 ...; look with `DISPLAY=:92 import -window root
# left.png`. Ctrl-C ends every NewQ it started. Not for users: the show network
# is set up from Settings ▸ Show network, and the test from Help ▸ Test mode.
set -eu
TEST=0
if [ "${1:-}" = "--test" ]; then TEST=1; shift; fi
N="${1:-2}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${2:-$ROOT/target/debug/newq}"
BASE="${NEWQ_DEMO_DIR:-/tmp/newq-net-demo}"
PORT="${NEWQ_NET_PORT:-48500}"
[ -x "$BIN" ] || { echo "no NewQ at $BIN: cargo build first" >&2; exit 1; }
rm -rf "$BASE"
mkdir -p "$BASE"
NAMES=(Booth Left Right Centre Rear Side Back Spare)
NAMES=("${NAMES[@]:0:$((N + 1))}")
# The identities: the first runs the network, the others are its display computers.
python3 - "$BASE" "${NAMES[@]}" <<'EOF'
import json, os, random, sys
base, names = sys.argv[1], sys.argv[2:]
hexid = lambda: "%016x" % (random.getrandbits(63) | 1)
net, primary = hexid(), hexid()
for i, n in enumerate(names):
    d = os.path.join(base, n, "node")
    os.makedirs(d, exist_ok=True)
    me = {"id": primary if i == 0 else hexid(), "name": n, "on": True,
          "network": net, "primary": primary, "term": 1,
          "role": "Primary" if i == 0 else "Display"}
    if i:
        me["primary_name"] = names[0]
    json.dump(me, open(os.path.join(d, "node.json"), "w"), indent=2)
EOF
cat > "$BASE/show.newq" <<'SHOW'
{"version": 2, "name": "Demo", "outputs": [{"id": 1, "name": "Display 1", "enabled": true,
 "test_pattern": false, "corners": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]}],
 "cues": [], "next_id": 2, "panic_fade": 1.0, "own_layers": true}
SHOW
PIDS=()
cleanup() { kill "${PIDS[@]}" 2>/dev/null || true; }
trap cleanup EXIT INT TERM
for i in "${!NAMES[@]}"; do
  name="${NAMES[$i]}"; screen=":$((91 + i))"
  Xvfb "$screen" -screen 0 1280x720x24 >/dev/null 2>&1 &
  PIDS+=($!)
done
sleep 1
for i in $(seq $((${#NAMES[@]} - 1)) -1 0); do
  name="${NAMES[$i]}"; dir="$BASE/$name"; mkdir -p "$dir/state"
  args=(); envs=()
  if [ "$i" = 0 ]; then
    args=("$BASE/show.newq")
    [ "$TEST" = 1 ] && envs=(NEWQ_NET_TEST_AUTOSTART="$N")
  fi
  env "${envs[@]+"${envs[@]}"}" DISPLAY=":$((91 + i))" NEWQ_NODE_DIR="$dir/node" \
    XDG_STATE_HOME="$dir/state" NEWQ_NET_PORT="$PORT" NEWQ_NO_SPLASH=1 NEWQ_MUTE=1 \
    WGPU_BACKEND="${WGPU_BACKEND:-vulkan}" \
    "$BIN" "${args[@]+"${args[@]}"}" >"$dir/out.txt" 2>&1 &
  PIDS+=($!)
  echo "$name on :$((91 + i)), its log in $dir/state/newq/newq.log"
done
if [ "$TEST" = 1 ]; then
  runs="$BASE/Booth/state/newq/test-mode/runs"
  echo "waiting for the network test's zip in $runs ..."
  until ls "$runs"/NewQ-test-*.zip >/dev/null 2>&1; do sleep 2; done
  sleep 2
  echo "zip: $(ls "$runs"/NewQ-test-*.zip)"
  # Kept running a while for a look at the results (NEWQ_DEMO_HOLD seconds).
  sleep "${NEWQ_DEMO_HOLD:-0}"
else
  echo "running; Ctrl-C ends them"
  wait
fi
