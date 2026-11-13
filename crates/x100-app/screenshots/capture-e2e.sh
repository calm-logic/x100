#!/usr/bin/env bash
# The real-window check of the end-to-end run, as capture.sh does it but on the builder's own
# logins: x100d on the real clock with a new project's default settings, x100-app on an X11
# display, driven through the X server (xtest.py), captured with xwd and recorded as a video.
#
# Part one says "hi" to a new project, then sends the todo brief that tests/real_e2e.rs sends, and
# waits for index.html on main. Part two connects Homebox ("Use local") and GitHub (the local gh)
# in Settings, asks the CEO for a small change and its deploy, approves the held deploy, and
# waits until the project's Homebox address serves the change.
#
# The real run spends the builder's subscription and leaves the machine: it mints an edit key
# named x100 in the local Homebox, creates a private GitHub repository under the account gh is
# logged in as, and deploys it publicly on Homebox. Run it only when asked to. It ends by printing
# what it created, kept in WORK_DIR/created.txt for cleanup-e2e.sh.
#
# --dry-run plays the same script at no cost and touching nothing real: the CEO is the fake
# harness playing e2e-dry-run.toml, Homebox is the tests' stand-in (x100d's example
# homebox_standin), and docker, gh and GitHub are its stand-ins.
#
# Builds nothing. First run:
#   cargo build -p x100d -p x100-app -p x100-tui --bins && cargo build -p x100d --example homebox_standin
# Coordinates assume the app's 1400x900 window.
#
# Usage: capture-e2e.sh [--dry-run] WORK_DIR [OUT_DIR] [DISPLAY]
#   WORK_DIR  keeps the project's code (under WORK_DIR/projects), the daemon's and the app's
#             output, the videos e2e.mp4 and e2e-fast.mp4, and created.txt. They can hold the
#             builder's account id, so it belongs outside the repository.
#   OUT_DIR   where the screenshots go (default: ./e2e beside this script). Check each, and the
#             videos, for an account id, an email or a home path before keeping them anywhere:
#             the program's terminal shows its working directory, which is under WORK_DIR, and
#             Settings shows the code directory, the GitHub account and Homebox's projects.
#   DISPLAY   an X display; with none given a private Xvfb is started on a free display number.
set -euo pipefail

dry=
if [[ ${1:-} == --dry-run ]]; then
  dry=1
  shift
fi
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
work=${1:?a work directory outside the repository}
out=${2:-$here/e2e}
bin=$root/target/debug
mkdir -p "$work" "$out"
work=$(cd "$work" && pwd)
# Codex will not set up its sandbox in a home under /tmp, so the daemon's data is in the target
# directory, which git ignores.
mkdir -p "$root/target/tmp"
data=$(mktemp -d "$root/target/tmp/e2e-window.XXXXXX")
daemon= app= xvfb= recorder= standin=

stop() {
  # Only the processes this script started. The recorder first, and by SIGINT, so ffmpeg writes
  # the end of the file and the video plays.
  if [[ -n $recorder ]]; then
    kill -INT "$recorder" 2>/dev/null || true
    wait "$recorder" 2>/dev/null || true
  fi
  for pid in $app $daemon; do kill "$pid" 2>/dev/null || true; done
  wait $app $daemon 2>/dev/null || true
  [[ -n $standin ]] && { kill "$standin" 2>/dev/null || true; wait "$standin" 2>/dev/null || true; }
  [[ -n $xvfb ]] && { kill "$xvfb" 2>/dev/null || true; wait "$xvfb" 2>/dev/null || true; }
  rm -rf "$data"
}
trap stop EXIT

for need in "$bin/x100d" "$bin/x100-app" "$bin/x100"; do
  [[ -x $need ]] || { echo "missing $need: build first, see the top of this script" >&2; exit 2; }
done

if [[ -n ${3:-} ]]; then
  export DISPLAY=$3
else
  n=97
  while [[ -e /tmp/.X11-unix/X$n || -e /tmp/.X$n-lock ]]; do n=$((n + 1)); done
  Xvfb ":$n" -screen 0 1400x900x24 -nolisten tcp >"$work/xvfb.log" 2>&1 &
  xvfb=$!
  export DISPLAY=:$n
  until xwininfo -root >/dev/null 2>&1; do sleep 0.2; done
fi

brief="Create a todo app as a proof of concept: one index.html with inline CSS and JavaScript, no build step and no dependencies. I can add a todo, tick it done, and delete it, and the list survives a reload."
followup="Add a 'Clear completed' button, then deploy it with Homebox."

# Anything that would point the daemon at other programs or another place goes; the dry run then
# names its stand-ins.
clean=(-u X100_CLAUDE_LOGIN -u X100_CODEX_LOGIN -u ANTHROPIC_BASE_URL -u OPENAI_BASE_URL
  -u X100_DATA_DIR -u X100_FAKE_SCRIPTS -u X100_FAKE_HARNESS -u X100_DOCKER -u X100_GH
  -u X100_GITHUB_GIT_BASE -u X100_GITHUB_CLIENT_ID -u X100_PROJECT)
if [[ -n $dry ]]; then
  [[ -x $bin/examples/homebox_standin ]] || { echo "missing the stand-in: build first, see the top of this script" >&2; exit 2; }
  stand=$work/standin
  rm -rf "$stand"
  mkdir -p "$stand/scripts"
  cp "$here/e2e-dry-run.toml" "$stand/scripts/"
  "$bin/examples/homebox_standin" --remotes "$stand/github" --bin "$stand/bin" \
    --key hbk_dry_run_minted_by_the_standin_2f7c9a41 >"$stand/ports" 2>"$stand/standin.log" &
  standin=$!
  until [[ -s $stand/ports ]]; do sleep 0.2; done
  read -r _ _ _ site_port <"$stand/ports"
  env "${clean[@]}" X100_DOCKER="$stand/bin/docker" X100_GH="$stand/bin/gh" \
    X100_GITHUB_GIT_BASE="file://$stand/github" \
    "$bin/x100d" --data-dir "$data" --projects-root "$work/projects" \
    --scripts-dir "$stand/scripts" --fake-harness "$bin/x100d-fake-harness" >"$work/daemon.log" 2>&1 &
else
  env "${clean[@]}" "$bin/x100d" --data-dir "$data" --projects-root "$work/projects" >"$work/daemon.log" 2>&1 &
fi
daemon=$!
until [[ -S $data/x100d.sock ]]; do sleep 0.2; done
x100() { "$bin/x100" --socket "$data/x100d.sock" "$@"; }

RUST_LOG=info "$bin/x100-app" --socket "$data/x100d.sock" >"$work/app.log" 2>&1 &
app=$!

window() { xwininfo -root -tree | grep '"x100' | head -1 | awk '{print $1}'; }
until [[ -n $(window) ]]; do sleep 0.2; done
id=$(window)
drive() { python3 -I "$here/xtest.py" "$id" "$@"; }
shot() {
  xwd -id "$id" -silent -out "$work/$1.xwd"
  ffmpeg -loglevel error -y -i "$work/$1.xwd" "$out/$1.png"
  rm -f "$work/$1.xwd"
  echo "captured $out/$1.png"
}

# The video, of the window's place on the display, from the first frame the app draws.
# x11grab takes only what is on the screen, so the area is moved onto it where the window's
# border puts it a pixel or two past the edge.
at() { awk -v k="$2" -F: '$1 ~ k {gsub(/ /, "", $2); print $2; exit}' <<<"$1"; }
win=$(xwininfo -id "$id") screen=$(xwininfo -root)
w=$(at "$win" Width) h=$(at "$win" Height)
x=$(at "$win" 'Absolute upper-left X') y=$(at "$win" 'Absolute upper-left Y')
x=$((x + w > $(at "$screen" Width) ? $(at "$screen" Width) - w : x))
y=$((y + h > $(at "$screen" Height) ? $(at "$screen" Height) - h : y))
ffmpeg -loglevel error -nostdin -y -f x11grab -framerate 10 \
  -video_size "${w}x${h}" -i "$DISPLAY+$((x < 0 ? 0 : x)),$((y < 0 ? 0 : y))" \
  -c:v libx264 -preset veryfast -crf 26 -pix_fmt yuv420p "$work/e2e.mp4" 2>"$work/ffmpeg.log" &
recorder=$!
sleep 6

# Stops the run on any sign that the logins should not be used further.
guard() {
  if grep -qE "usage window is|asks for a login|waiting on its terminal" "$work/daemon.log"; then
    echo "stopping: the daemon reports a login or usage problem; see $work/daemon.log" >&2
    exit 1
  fi
}
# Waits up to $2 seconds for the command in $3 to succeed; on timeout says what it waited for,
# with the chat's last lines, and stops.
until_true() {
  local what=$1 deadline=$((SECONDS + $2))
  until eval "$3"; do
    guard
    if ((SECONDS >= deadline)); then
      echo "timed out after $2 seconds waiting for: $what" >&2
      echo "the chat's last lines:" >&2
      x100 chat 2>/dev/null | tail -5 >&2 || true
      echo "the daemon's output: $work/daemon.log" >&2
      exit 1
    fi
    sleep 2
  done
}
ceo_lines() { x100 chat 2>/dev/null | awk -F'\t' '$2 == "ceo"' | wc -l; }
# The CEO's lines in the chat after the first $before.
ceo_since() { x100 chat 2>/dev/null | awk -F'\t' '$2 == "ceo"' | tail -n +$((before + 1)); }

# Part one.
#
# No project: the box for the first message. "hi" creates the project, which the CEO answers on
# a driver of its own, without setting it up.
shot new-project
drive click 785 470 sleep 0.5 type "hi" key Return
until_true "the CEO to answer hi" 600 '(( $(ceo_lines) >= 1 ))'
sleep 3
shot new-project-hi

# The brief, in the chat's box. The CEO sets the project up from it, which makes its code
# directory, and goes on in its worktree on a new driver.
drive click 380 815 sleep 0.5 type "$brief" sleep 0.5
shot brief
drive key Return
until_true "the CEO to set the project up" 600 'compgen -G "$work/projects/*/.git" >/dev/null'
project=$(compgen -G "$work/projects/*" | head -1)
shot named

# The CEO at work, once a driver is on the brief's task: the Team tab opened on it, then its live
# terminal a little later. A small brief can be done in under half a minute.
until_true "a driver for the brief" 300 'x100 log --all 2>/dev/null | grep -q "Chose "'
sleep 6
shot team
sleep 6
shot terminal

# The final report: index.html reaches main when the CEO reports the brief.
until_true "index.html on main" 2400 'git -C "$project" cat-file -e main:index.html 2>/dev/null'
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

# Part two.
#
# Settings, scrolled to its end, where Integrations is. Homebox's "Use local" asks first; the
# daemon then mints its key in Homebox's container. GitHub connects through the local gh.
to_end() { drive scroll 1000 500 60 sleep 1; }
drive click 1073 21 sleep 1
to_end
until_true "Settings to say what it found" 60 'x100 integrations | grep -q "^homebox"'
sleep 2
shot integrations
drive click 998 601 sleep 1.5
shot homebox-confirm
drive click 690 620
until_true "Homebox to connect" 120 'x100 integrations | grep -qP "^homebox\\tconnected"'
sleep 3
to_end
shot homebox-connected
drive click 998 695
until_true "GitHub to connect" 120 'x100 integrations | grep -qP "^github\\tconnected"'
sleep 3
to_end
shot integrations-connected

# The follow-up, in the chat's box. The CEO makes the change, reports it so it is on main, and
# asks for the deploy, which is held for the builder: High risk only is a new project's default.
before=$(ceo_lines)
drive click 380 815 sleep 0.5 type "$followup" sleep 0.5 key Return
until_true "the CEO to ask for the deploy" 1800 'x100 actions | grep -qi deploy'
action=$(x100 actions | grep -i deploy | head -1 | cut -f1)
sleep 3
shot held-deploy
drive click 234 753
until_true "the Approve click to reach the daemon" 30 '! x100 actions | grep -q "^$action"'
sleep 2
shot approved

# The deploy: x100 publishes the project to GitHub, adds it to Homebox, waits for the deployment,
# and the CEO tells the builder the address, which must then serve the change.
until_true "the CEO to tell the address" 1800 'ceo_since | grep -q "https://"'
url=$(ceo_since | grep -o 'https://[^][ ()<>"`*]*' | tail -1 | sed -E 's/[.,;:!?]+$//')
host=${url#https://}
host=${host%%/*}
page() {
  if [[ -n $dry ]]; then
    curl -fsS --max-time 10 -H "Host: $host" "http://127.0.0.1:$site_port/"
  else
    curl -fsS --max-time 15 "$url"
  fi
}
until_true "$url to serve the change" 600 'page 2>/dev/null | grep -qi "clear completed"'
sleep 5
shot deployed
drive click 1073 21 sleep 2
to_end
shot settings-deployed
drive click 901 21 sleep 2
shot log-deployed

# What the run made outside this machine, for cleanup-e2e.sh.
logged=$(x100 log --all)
repository=$(grep -o 'Published the project to the GitHub repository [^[:space:]]*' <<<"$logged" | tail -1 | awk '{print $NF}')
homebox_project=$(grep -o 'Linked the project to the Homebox project [^[:space:]]*' <<<"$logged" | tail -1 | awk '{print $NF}')
{
  echo "# What capture-e2e.sh made$([[ -n $dry ]] && echo ' on its stand-ins'), for cleanup-e2e.sh"
  echo "REPOSITORY=$repository"
  echo "HOMEBOX_PROJECT=$homebox_project"
  echo "HOMEBOX_URL=$url"
  echo "HOMEBOX_KEY_NAME=x100"
} >"$work/created.txt"
echo
echo "Created$([[ -n $dry ]] && echo ' on the stand-ins, not on any real service'):"
echo "  the private GitHub repository $repository"
echo "  the Homebox project $homebox_project, public at $url (and its dev environment)"
echo "  the Homebox API key named x100, minted by \"Use local\""
echo "Recorded in $work/created.txt; cleanup-e2e.sh $work/created.txt releases and removes them."

# The videos: the recording as it was, and a cut of about 75 seconds with the frames that did not
# change dropped and the rest sped up to fit.
kill -INT "$recorder"
wait "$recorder" || true
recorder=
# A still stretch keeps a frame about every second, so waits shrink tenfold and stay readable.
ffmpeg -loglevel error -y -i "$work/e2e.mp4" -vf "mpdecimate=hi=64*24:lo=64*8:frac=0.2:max=10,setpts=N/10/TB" \
  -an -c:v libx264 -preset veryfast -crf 26 -pix_fmt yuv420p "$work/e2e-changes.mp4"
long=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$work/e2e-changes.mp4")
speed=$(awk -v d="$long" 'BEGIN { print (d > 90 ? d / 75 : 1) }')
ffmpeg -loglevel error -y -i "$work/e2e-changes.mp4" -vf "setpts=PTS/$speed,fps=30" \
  -an -c:v libx264 -preset veryfast -crf 26 -pix_fmt yuv420p "$work/e2e-fast.mp4"
rm -f "$work/e2e-changes.mp4"
if [[ -n $dry ]]; then
  X100_DOCKER=$stand/bin/docker "$here/cleanup-e2e.sh" "$work/created.txt"
fi
echo "recorded $work/e2e.mp4 and $work/e2e-fast.mp4 (${long%.*} seconds of change, sped up ${speed}x)"
