#!/usr/bin/env bash
# Feeds the spike's terminal: coloured log lines that scroll, plus a progress bar redrawn in
# place with carriage returns and cursor movement, at roughly 50 lines a second.
i=0
levels=("\e[32mINFO \e[0m" "\e[33mWARN \e[0m" "\e[31mERROR\e[0m" "\e[36mDEBUG\e[0m")
printf '\e[1;35mspike feed\e[0m: 16 colours, 256 colours, truecolor, bold, inverse\n'
for c in $(seq 0 15); do printf "\e[48;5;${c}m  "; done; printf '\e[0m\n'
for c in $(seq 16 6 231); do printf "\e[48;5;${c}m "; done; printf '\e[0m\n'
for r in $(seq 0 8 255); do printf "\e[48;2;${r};$((255-r));128m "; done; printf '\e[0m\n'
while true; do
  i=$((i+1))
  lvl=${levels[$((RANDOM % 4))]}
  printf "%s \e[90m%(%H:%M:%S)T\e[0m ${lvl} agent-%03d \e[1mtask\e[0m #%d \e[7m inv \e[0m \e[38;5;%dmcolour %d\e[0m\n" \
    "$i" -1 $((RANDOM % 100)) $((RANDOM % 900)) $((RANDOM % 216 + 16)) $((RANDOM % 1000))
  if (( i % 25 == 0 )); then
    # Draw a bar, then redraw it in place a few times: \r returns to column 0, \e[K clears.
    for p in 0 25 50 75 100; do
      n=$((p / 5))
      printf '\r\e[K\e[34mbuild\e[0m [\e[42m%*s\e[0m%*s] %3d%%' "$n" '' $((20 - n)) '' "$p"
      sleep 0.02
    done
    printf '\n'
    # Move up two lines, overwrite with a marker, then move back down.
    printf '\e[2A\r\e[K\e[45;97m rewritten by cursor-up \e[0m\e[2B\r'
  fi
  sleep 0.02
done
