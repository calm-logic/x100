#!/usr/bin/env bash
# The real-window check: x100d on the fake clock with the Fernleaf script, x100-app on the X11
# display, driven through the X server (xtest.py) and captured with xwd. Builds nothing: run
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

"$bin/x100d" --data-dir "$work/data" --projects-root "$work/projects" --scripts-dir "$root/crates/x100-orch/tests/scripts" \
  --fake-harness "$bin/x100d-fake-harness" --fake-clock 1794387600000 --seed 7 >"$work/daemon.log" 2>&1 &
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

# No project: the window opens on the box for the first message, with no form and no face, once its
# opening has played.
shot new-project
# "hi" creates the project, listed as "Untitled" until its CEO names it; the CEO only answers,
# and with no leads the Team tab is its drilldown.
drive click 785 470 sleep 0.5 type "hi" key Return sleep 5
shot new-project-hi

# The brief, in the chat's box, with both files added through "+". WSL has no file chooser portal,
# so "+" opens a field for a path in its place, and Enter adds the file and closes it.
drive click 211 858 sleep 0.5 type "$work/sketch.png" key Return sleep 1.5
drive click 211 858 sleep 0.5 type "$work/notes.md" key Return sleep 1.5
drive click 211 858 sleep 0.7
shot composer
drive click 211 858 sleep 0.5 click 380 759 type "Launch Fernleaf" key shift+Return \
  type "A landing page for Fernleaf, a plant shop, with a waitlist." sleep 0.5
shot brief
# The CEO sets the project up as Fernleaf, and its team forms.
drive key Return sleep 6
shot named
shot team

# Opening agents, down to one that runs none. Leads are listed Marketing, Product, Tech. An opened
# agent is a drilldown: its head and name in the top row's corner, with the switch between its
# agents and its own output beside the listing's.
drive click 999 694 sleep 1
shot agent
drive click 1205 76 sleep 1.5
shot drilldown
drive click 1132 76 sleep 1 click 999 156 sleep 1 click 999 156 sleep 1.5
shot output
# The CEO in Migrations' trail, then the drivers.
drive click 752 76 sleep 0.7 click 1349 76 sleep 1
shot drivers

# The builder answers the CEO's question; Marketing gets its first task.
drive click 1287 76 sleep 0.5 click 380 815 type "Forest greens." key Return sleep 6
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
# Settings: the project's name, intent and code directory first, then Ask me about, changed here,
# which reaches the daemon and the Log; then the Credentials part at the foot of the tab.
drive click 1073 21 sleep 1 click 1331 236 sleep 1
shot settings
drive scroll 1000 500 40 sleep 0.7 click 1000 876 sleep 0.7 scroll 1000 500 40 sleep 0.7
shot credentials
drive click 901 21 sleep 1.5
shot log

grep -m1 "Selected GPU adapter" "$work/app.log" || true
