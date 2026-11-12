#!/usr/bin/env bash
# The real-window check of an agent's view, as capture.sh does it: x100d on the fake clock with
# the Fernleaf script, x100-app on the X11 display, driven through the X server (xtest.py) and captured with xwd.
# Builds nothing: run `cargo build -p x100d -p x100-app --bins` and
# `cargo build -p x100-app --release --example replay` first.
#
# Captures Research's view, then the same view after an hour on the fake clock (a new driver
# carries on), Migrations' view three levels down, and real `claude` and `codex` recordings
# replayed into the widget at full size and in a small window.
#
# Usage: capture-output.sh [OUT_DIR]   (default: this directory)
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
out=${1:-$here}
bin=$root/target/debug
data=$here/../tests/data
work=$(mktemp -d)
daemon= app= replay=

stop() {
  # Only the processes this script started.
  for pid in $replay $app $daemon; do kill "$pid" 2>/dev/null || true; done
  wait 2>/dev/null || true
}
trap stop EXIT

"$bin/x100d" --data-dir "$work/data" --projects-root "$work/projects" --scripts-dir "$root/crates/x100-orch/tests/scripts" \
  --fake-harness "$bin/x100d-fake-harness" --fake-clock 1794387600000 >"$work/daemon.log" 2>&1 &
daemon=$!
until [[ -S $work/data/x100d.sock ]]; do sleep 0.2; done

RUST_LOG=info "$bin/x100-app" --socket "$work/data/x100d.sock" >"$work/app.log" 2>&1 &
app=$!

window() { xwininfo -root -tree | grep "\"$1" | head -1 | awk '{print $1}'; }
until [[ -n $(window x100) ]]; do sleep 0.2; done
sleep 3
id=$(window x100)
drive() { python3 -I "$here/xtest.py" "$id" "$@"; }
shot() {
  xwd -id "$1" -silent -out "$work/$2.xwd"
  ffmpeg -loglevel error -y -i "$work/$2.xwd" "$out/$2.png"
  echo "captured $out/$2.png"
}

# Moves the daemon's fake clock, over the protocol.
clock_to() {
  python3 -I - "$work/data/x100d.sock" "$1" <<'PY'
import json, socket, sys
s = socket.socket(socket.AF_UNIX)
s.connect(sys.argv[1])
f = s.makefile("rw")
for i, (m, p) in enumerate([("hello", {"client": "capture", "protocol": 1}),
                            ("daemon.set_clock", {"at": int(sys.argv[2])})]):
    f.write(json.dumps({"jsonrpc": "2.0", "id": i + 1, "method": m, "params": p}) + "\n")
    f.flush()
    reply = json.loads(f.readline())
    if "error" in reply:
        sys.exit(f"{m}: {reply['error']}")
PY
}

# The brief as the first message of a new project, which the CEO sets up from it at once.
drive click 785 470 sleep 0.5 type "Launch Fernleaf" key shift+Return \
  type "A landing page for Fernleaf, a plant shop, with a waitlist." sleep 0.5 key Return sleep 6

# Leads are listed Marketing, Product, Tech. Product runs Research.
drive click 999 592 sleep 1 click 999 331 sleep 2
shot "$id" research
# An hour on, Research's job wakes it on a new driver.
clock_to $((1794387600000 + 3600000))
sleep 4
shot "$id" research-handoff

# Back to the CEO, then Tech, Backend and Migrations.
drive click 635 79 sleep 1 click 999 664 sleep 1 click 999 331 sleep 1 click 999 331 sleep 2
shot "$id" migrations

grep -m1 "Selected GPU adapter" "$work/app.log" || true
kill "$app" "$daemon" 2>/dev/null || true
app= daemon=

# Real recordings in the widget's own window. Claude's is cut before it exits, which would leave
# its alternate screen and show only its farewell.
replay_shot() {
  "$root/target/release/examples/replay" "$data/$1.raw" "$2" 1 1 "$4" "$5" >"$work/replay.log" 2>&1 &
  replay=$!
  until [[ -n $(window "x100 replay") ]]; do sleep 0.2; done
  until grep -q 'loop 1 done' "$work/replay.log"; do sleep 0.2; done
  sleep 1
  shot "$(window "x100 replay")" "$3"
  kill "$replay"
  wait "$replay" 2>/dev/null || true
  replay=
}
head -26 "$data/claude.timing" >"$work/claude.timing"
replay_shot claude "$work/claude.timing" replay-claude 1000 700
replay_shot codex "$data/codex.timing" replay-codex 1000 700
replay_shot claude "$work/claude.timing" replay-scaled 560 340
