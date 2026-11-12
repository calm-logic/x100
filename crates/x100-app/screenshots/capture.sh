#!/usr/bin/env bash
# The real-window check: x100d on the fake clock with the Fernleaf script, x100-app on the X11
# display, driven through XTEST (xtest.py) and captured with xwd. Builds nothing: run
# `cargo build -p x100d -p x100-app --bins` first. Coordinates assume the app's 1400x900 window.
#
# Usage: capture.sh [OUT_DIR]   (default: this directory)
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
out=${1:-$here}
bin=$root/target/debug
work=$(mktemp -d)
daemon= app=

stop() {
  # Only the processes this script started.
  [[ -n $app ]] && kill "$app" 2>/dev/null || true
  [[ -n $daemon ]] && kill "$daemon" 2>/dev/null || true
  wait 2>/dev/null || true
}
trap stop EXIT

"$bin/x100d" --data-dir "$work/data" --scripts-dir "$root/crates/x100-orch/tests/scripts" \
  --fake-harness "$bin/x100d-fake-harness" --fake-clock 1794387600000 >"$work/daemon.log" 2>&1 &
daemon=$!
until [[ -S $work/data/x100d.sock ]]; do sleep 0.2; done

RUST_LOG=info "$bin/x100-app" --socket "$work/data/x100d.sock" >"$work/app.log" 2>&1 &
app=$!

window() { xwininfo -root -tree | grep '"x100' | head -1 | awk '{print $1}'; }
until [[ -n $(window) ]]; do sleep 0.2; done
sleep 6
id=$(window)
drive() { python3 -I "$here/xtest.py" "$id" "$@"; }
shot() {
  xwd -id "$id" -silent -out "$work/$1.xwd"
  ffmpeg -loglevel error -y -i "$work/$1.xwd" "$out/$1.png"
  echo "captured $out/$1.png"
}

# Files to attach: a picture and a note.
ffmpeg -loglevel error -f lavfi -i "color=c=0x2fd27a:s=160x120" -vf "drawbox=x=40:y=30:w=80:h=60:color=0x14532d:t=fill" \
  -frames:v 1 "$work/sketch.png"
printf '# Notes\nForest greens, maybe.\n' >"$work/notes.md"

# A project from the sidebar.
drive click 60 67 sleep 0.5 type "Fernleaf" click 80 110 type "$work/fernleaf" key Return sleep 2

# First run: the brief, with both files attached by path (WSL has no file chooser portal).
drive click 601 558 sleep 0.5 type "$work/sketch.png" key Return sleep 1.5
drive click 601 586 sleep 0.5 type "$work/notes.md" key Return sleep 1.5
drive click 785 440 type "Launch Fernleaf" key shift+Return \
  type "A landing page for Fernleaf, a plant shop, with a waitlist." sleep 0.5
shot first-run
drive key Return sleep 6
shot team

# Opening agents, down to one that runs none. Leads are listed Marketing, Product, Tech, under the
# switch between an agent's agents and its own output.
drive click 999 692 sleep 1
shot agent
drive click 999 383 sleep 0.7 click 999 383 sleep 1
shot output
drive click 635 79 sleep 0.7 click 1349 80 sleep 1
shot drivers

# The builder answers the CEO's question; Marketing gets its first task.
drive click 1287 80 sleep 0.5 click 380 815 type "Forest greens." key Return sleep 6
shot answered

# The other tabs, left to right. A tab's centre is where its label is on the 1400x900 window.
drive click 686 21 sleep 1
shot specs
drive click 740 21 sleep 1
shot tasks
# Raising a task: the form opens above the list.
drive click 1339 74 sleep 0.5 type "Add a pricing FAQ" click 700 175 type "Answer what people ask before they subscribe." sleep 0.5
shot tasks-raise
drive click 1339 74 sleep 0.5
drive click 792 21 sleep 1.5
shot files
# The cross asks before it removes.
drive click 757 119 sleep 0.7
shot files-confirm
drive click 848 21 sleep 1
shot budget
drive click 957 21 sleep 1
shot preview
drive click 1015 21 sleep 1
shot infra
# Settings: change Ask me about, which reaches the daemon and the Log; then the Credentials part.
drive click 1073 21 sleep 1 click 1331 78 sleep 1
shot settings
drive click 1000 851 sleep 0.7 scroll 1000 500 40 sleep 0.7
shot credentials
drive click 901 21 sleep 1.5
shot log

grep -m1 "Selected GPU adapter" "$work/app.log" || true
