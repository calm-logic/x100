#!/usr/bin/env bash
# The real-window check of the end-to-end run, as capture.sh does it but on the builder's own
# logins: x100d on the real clock with a new project's default settings, x100-app on an X11
# display, driven through the X server (xtest.py) and captured with xwd. It says "hi" to a new
# project, then sends the todo brief that tests/real_e2e.rs sends, so it spends the builder's
# subscription: run it only when asked to. Builds nothing: run
# `cargo build -p x100d -p x100-app --bins` first. Coordinates assume the app's 1400x900 window.
#
# Usage: capture-e2e.sh WORK_DIR [OUT_DIR] [DISPLAY]
#   WORK_DIR  keeps the project's code (under WORK_DIR/projects) and the daemon's and the app's
#             output. They can hold the builder's account id, so it belongs outside the repository.
#   OUT_DIR   where the screenshots go (default: ./e2e beside this script). Check each for an
#             account id, an email or a home path before keeping it in the repository: the
#             program's terminal shows its working directory, which is under WORK_DIR.
#   DISPLAY   an X display; with none given a private Xvfb is started on :97.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
work=${1:?a work directory outside the repository}
out=${2:-$here/e2e}
bin=$root/target/debug
mkdir -p "$work" "$out"
# Codex will not set up its sandbox in a home under /tmp, so the daemon's data is in the target
# directory, which git ignores.
data=$(mktemp -d "$root/target/tmp/e2e-window.XXXXXX")
daemon= app= xvfb=

stop() {
  # Only the processes this script started.
  for pid in $app $daemon; do kill "$pid" 2>/dev/null || true; done
  wait $app $daemon 2>/dev/null || true
  [[ -n $xvfb ]] && kill "$xvfb" 2>/dev/null || true
  rm -rf "$data"
}
trap stop EXIT

if [[ -n ${3:-} ]]; then
  export DISPLAY=$3
else
  Xvfb :97 -screen 0 1400x900x24 -nolisten tcp >"$work/xvfb.log" 2>&1 &
  xvfb=$!
  export DISPLAY=:97
  until xwininfo -root >/dev/null 2>&1; do sleep 0.2; done
fi

brief="Create a todo app as a proof of concept: one index.html with inline CSS and JavaScript, no build step and no dependencies. I can add a todo, tick it done, and delete it, and the list survives a reload."

env -u X100_CLAUDE_LOGIN -u X100_CODEX_LOGIN -u ANTHROPIC_BASE_URL -u OPENAI_BASE_URL \
  "$bin/x100d" --data-dir "$data" --projects-root "$work/projects" >"$work/daemon.log" 2>&1 &
daemon=$!
until [[ -S $data/x100d.sock ]]; do sleep 0.2; done

RUST_LOG=info "$bin/x100-app" --socket "$data/x100d.sock" >"$work/app.log" 2>&1 &
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
# Stops the run on any sign that the logins should not be used further.
guard() {
  if grep -qE "usage window is|asks for a login|waiting on its terminal" "$work/daemon.log"; then
    echo "stopping: the daemon reports a login or usage problem; see $work/daemon.log" >&2
    exit 1
  fi
}
# Waits up to $2 seconds for the command in $1 to succeed.
until_true() {
  local deadline=$((SECONDS + $2))
  until eval "$1"; do
    guard
    ((SECONDS < deadline)) || { echo "timed out: $1" >&2; return 1; }
    sleep 2
  done
}

# No project: the box for the first message. "hi" creates the project, which the CEO answers on
# a driver of its own, without setting it up.
shot new-project
drive click 785 470 sleep 0.5 type "hi" key Return
until_true 'compgen -G "$data/harnesses/*-terminal/drv_*" >/dev/null' 120
sleep 30
shot new-project-hi

# The brief, in the chat's box. The CEO sets the project up from it, which makes its code
# directory, and goes on in its worktree on a new driver.
drive click 380 815 sleep 0.5 type "$brief" sleep 0.5
shot brief
drive key Return
until_true 'compgen -G "$work/projects/*/.git" >/dev/null' 600
project=$(compgen -G "$work/projects/*" | head -1)
shot named

# The CEO at work, once its driver's program is up: the Team tab opened on it, then its live
# terminal a little later. A small brief can be done in under half a minute.
until_true 'compgen -G "$data/harnesses/*-terminal/drv_*" >/dev/null' 120
sleep 6
shot team
sleep 6
shot terminal

# The final report: index.html reaches main when the CEO reports the brief.
until_true 'git -C "$project" cat-file -e main:index.html 2>/dev/null' 2400
sleep 20
shot chat

# The tabs afterwards. A tab's centre is where its label is on the 1400x900 window.
drive click 792 21 sleep 2
shot files
drive click 740 21 sleep 2
shot tasks
drive click 848 21 sleep 2
shot budget
drive click 901 21 sleep 2
shot log
drive click 635 21 sleep 2
shot team-after
