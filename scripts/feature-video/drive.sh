#!/bin/bash
# Drives a demo Agentty (debug build, its own data folder) for a feature video: start it, send it debug
# commands, wait for what a pane shows, and record its window.
#
#   drive.sh start [--remote] [--release]   debug (or release) build with AGENTTY_DEBUG=1 and a throwaway data folder, window 1920x1080
#                                   (or the largest 16:9 that fits a smaller screen)
#   drive.sh cmd <command> [arg]    one debug command (see `debug_command` in workbench/mod.rs)
#   drive.sh wait <regex> [secs]    until the active pane's screen matches
#   drive.sh prompt <text>          type into the active pane, then Enter (sent apart, so it is not a paste)
#   drive.sh agent claude|codex <folder>   open an agent tab there and wait until it is ready
#   drive.sh record <name> [secs]   record the window to <dir>/footage/<name>.mov in the background (the pointer
#                                   is parked outside the window first)
#   drive.sh mark <label>           a named moment in the recording (storyboard times like "@label+0.5")
#   drive.sh tap "<x y [mod]>" <label>   a click that names what it lands on (checked by `video.sh verify`)
#   drive.sh stop                   stop the recording; every click, key and mark sent meanwhile is written to
#                                   footage/<name>.mov.events.json in footage seconds (the film's cursor and zooms)
#   drive.sh snap <name>            still of the window (no screen-recording permission needed)
#   drive.sh demo-project           create the demo project ~/agentty-demo/taskboard (once)
#   drive.sh quit                   quit the app (also stops a recording)
#
# State lives in $VIDEO_DIR (default: the scratchpad, else /tmp): app.pid, app.log, data/, footage/.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
DIR="${VIDEO_DIR:-${CLAUDE_SCRATCHPAD:-${TMPDIR:-/tmp}}/feature-video}"
mkdir -p "$DIR/footage"
LOG="$DIR/app.log"
DEMO="$HOME/agentty-demo"

pid() { cat "$DIR/app.pid" 2>/dev/null; }
socket() {
  local p; p="$(pid)"
  for base in "${TMPDIR%/}" /tmp; do
    for s in "$base"/agentty-"$p"-*/s.sock; do [ -S "$s" ] && { echo "$s"; return; }; done
  done
}
send() { printf 'debug\t%s\t%s\n' "$1" "${2:-}" | nc -U -w 1 "$(socket)"; }
now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time * 1000'; }
# While recording, what is sent and when (wall-clock ms), for the events file `stop` writes.
log_event() { [ -f "$DIR/record.pid" ] && printf '%s\t%s\t%s\t%s\n' "$(now_ms)" "$1" "${2:-}" "${3:-}" >>"$DIR/record.events"; return 0; }
say() { echo "[$(date +%H:%M:%S)] $*"; }
lines() { local n; n=$(grep -c "$1" "$LOG" 2>/dev/null); echo "${n:-0}"; }
# The active pane's visible lines, freshly reported.
screen() {
  local before; before=$(lines 'screen-lines:')
  send screen-lines
  for _ in $(seq 1 30); do [ "$(lines 'screen-lines:')" -gt "$before" ] && break; sleep 0.1; done
  grep 'screen-lines:' "$LOG" | tail -1
}
# The window's frame in screen points, top-left origin, as screencapture -R wants it.
window_rect() {
  local before; before=$(lines '^frame:')
  send frame
  for _ in $(seq 1 30); do [ "$(lines '^frame:')" -gt "$before" ] && break; sleep 0.1; done
  grep '^frame:' "$LOG" | tail -1 | python3 -c '
import re, sys
line = sys.stdin.read()
nums = [float(n) for n in re.findall(r"(?:x|y|width|height): (-?[\d.]+)", line)]
(x, y, w, h), (_, sy, _, sh) = nums[0:4], nums[4:8]
# Cocoa y grows upwards from the bottom of the main screen; the visible frame tells its height.
from subprocess import check_output
# Newer macOS no longer prints "UI Looks like"; a Retina panel at its default scale is half its pixels.
info = check_output(["system_profiler", "SPDisplaysDataType"], text=True)
looks = re.search(r"UI Looks like: (\d+) x (\d+)", info)
pixels = re.search(r"Resolution: (\d+) x (\d+)( Retina)?", info)
screen_h = float(looks.group(2)) if looks else float(pixels.group(2)) / (2 if pixels.group(3) else 1)
print(f"{int(x)},{int(screen_h - y - h)},{int(w)},{int(h)}")'
}

# 1920x1080 when the screen's visible area holds it from where the window is, else the largest 16:9 size that does.
window_fit() {
  local before; before=$(lines '^frame:')
  send frame
  for _ in $(seq 1 30); do [ "$(lines '^frame:')" -gt "$before" ] && break; sleep 0.1; done
  grep '^frame:' "$LOG" | tail -1 | python3 -c '
import re, sys
nums = [float(n) for n in re.findall(r"(?:x|y|width|height): (-?[\d.]+)", sys.stdin.read())]
if len(nums) == 8:
    # Resizing keeps the top-left corner of the window, so the room is from there to the right and bottom edges.
    (x, y, _, h), (sx, sy, sw, _) = nums[0:4], nums[4:8]
    sw, sh = sx + sw - x, y + h - sy
    if sw < 1920 or sh < 1080:
        w = int(min(sw, sh * 16 / 9)) // 16 * 16
        print(f"{w}x{w * 9 // 16}")'
}

case "${1:-}" in
  start)
    [ -n "$(pid)" ] && kill -0 "$(pid)" 2>/dev/null && { say "already running (pid $(pid))"; exit 0; }
    # --release: the optimized build. Plugins (WebAssembly) run many times slower in a debug build:
    # a load test there manages about 0.4 requests a second. The remote dev mode needs a debug build.
    profile=debug
    env_extra=()
    for flag in "${@:2}"; do
      case "$flag" in
        --remote) env_extra=(AGENTTY_REMOTE_DEV=1) ;;
        --release) profile=release ;;
      esac
    done
    if [ "$profile" = release ]; then
      (cd "$REPO" && cargo build --release -p agentty-app 2>&1 | tail -1)
    else
      (cd "$REPO" && cargo build -p agentty-app 2>&1 | tail -1)
    fi
    mkdir -p "$DIR/data"
    # `${a[@]+…}`: macOS's bash 3.2 calls an empty array unbound under `set -u`.
    # -AppleLanguages: pages in the in-app browser (the remote page among them) read the app's
    # language list, which is the Mac's unless given; the footage is in English.
    env ${env_extra[@]+"${env_extra[@]}"} AGENTTY_DATA_DIR="$DIR/data" AGENTTY_DEBUG=1 "$REPO/target/$profile/agentty" -AppleLanguages '(en)' >"$LOG" 2>&1 &
    echo $! >"$DIR/app.pid"
    for _ in $(seq 1 60); do [ -n "$(socket)" ] && break; sleep 0.5; done
    [ -n "$(socket)" ] || { say "app did not come up, see $LOG"; exit 1; }
    sleep 2
    send resize 1920x1080
    # A smaller screen (a laptop's 1512x982) can't hold 1920x1080: the largest 16:9 window that fits.
    fit="$(window_fit)"
    [ -n "$fit" ] && send resize "$fit"
    send activate
    say "running (pid $(pid)), data in $DIR/data"
    ;;
  cmd) log_event "$2" "${3:-}"; send "$2" "${3:-}" ;;
  mark) log_event mark "$2"; say "mark $2" ;;
  # tap "<x y [double|right]>" "<label>": a click that says what it lands on ("Format", "Hot");
  # `verify` reads that text under the film's cursor.
  tap) log_event click "$2" "${3:-}"; send click "$2" ;;
  screen) screen ;;
  wait)
    for _ in $(seq 1 "${3:-60}"); do screen | grep -qE "$2" && exit 0; sleep 1; done
    say "timed out waiting for /$2/"; screen; exit 1
    ;;
  prompt) log_event prompt "$2"; send type "$2"; sleep 0.8; send type '\n' ;;
  agent)
    send launch-agent "$2 $3"
    if [ "$2" = claude ]; then ready='shift\+tab to cycle|for shortcuts'; else ready='context left|% left|esc to interrupt'; fi
    for _ in $(seq 1 45); do
      s="$(screen)"
      if echo "$s" | grep -qiE 'trust the files|trust this folder|Do you trust|allow Codex to work'; then send type '\n'; sleep 3; continue; fi
      echo "$s" | grep -qE "$ready" && { sleep 1; exit 0; }
      sleep 1
    done
    say "$2 did not come up"; screen; exit 1
    ;;
  record)
    rect="$(window_rect)"
    out="$DIR/footage/$2.mov"
    rm -f "$out"
    # The real pointer would sit in the footage wherever it was left (the driver's clicks never move
    # it): park it just under the window. The film draws its own cursor from the logged clicks.
    IFS=, read -r rx ry rw rh <<<"$rect"
    swift "$HERE/park.swift" "$((rx + rw / 2))" "$((ry + rh + 30))" 2>/dev/null || true
    printf '%s\n%s\n%s\n' "$2" "$rw" "$rh" >"$DIR/record.meta"
    : >"$DIR/record.events"
    # -V stops by itself after that many seconds; `drive.sh stop` ends it earlier.
    screencapture -x -v ${3:+-V "$3"} -R"$rect" "$out" >/dev/null 2>&1 &
    echo $! >"$DIR/record.pid"
    now_ms >"$DIR/record.start"
    sleep 1
    say "recording $rect to $out"
    ;;
  stop)
    p="$(cat "$DIR/record.pid" 2>/dev/null)"
    stopped="$(now_ms)"
    [ -n "$p" ] && kill -INT "$p" 2>/dev/null
    for _ in $(seq 1 20); do kill -0 "$p" 2>/dev/null || break; sleep 0.5; done
    rm -f "$DIR/record.pid"
    if [ -f "$DIR/record.meta" ]; then
      python3 "$HERE/events.py" "$DIR" "$stopped"
      rm -f "$DIR/record.meta" "$DIR/record.events" "$DIR/record.start"
    fi
    ls -la "$DIR"/footage/*.mov 2>/dev/null | tail -3
    ;;
  snap) send snapshot "$DIR/footage/$2.png"; sleep 1.5; echo "$DIR/footage/$2.png" ;;
  demo-project)
    P="$DEMO/taskboard"
    [ -d "$P/.git" ] && { echo "$P"; exit 0; }
    mkdir -p "$P"
    cat >"$P/index.html" <<'HTML'
<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Taskboard</title>
  <link rel="stylesheet" href="style.css">
</head>
<body>
  <main class="board">
    <h1>Taskboard</h1>
    <form id="new-task"><input id="title" placeholder="What needs doing?" autocomplete="off"><button>Add</button></form>
    <ul id="tasks"></ul>
  </main>
  <script type="module" src="app.js"></script>
</body>
</html>
HTML
    cat >"$P/style.css" <<'CSS'
body { font-family: system-ui, sans-serif; margin: 0; background: #f4f4f5; color: #18181b; }
.board { max-width: 480px; margin: 48px auto; padding: 24px; background: #fff; border-radius: 12px; }
form { display: flex; gap: 8px; }
input { flex: 1; padding: 8px; font-size: 16px; }
ul { list-style: none; padding: 0; }
li { display: flex; gap: 8px; padding: 8px 0; border-bottom: 1px solid #e4e4e7; }
li.done span { text-decoration: line-through; color: #a1a1aa; }
CSS
    cat >"$P/tasks.js" <<'JS'
export function addTask(tasks, title) {
  return [...tasks, { id: Date.now(), title: title.trim(), done: false }];
}

export function toggleTask(tasks, id) {
  return tasks.map((t) => (t.id === id ? { ...t, done: !t.done } : t));
}
JS
    cat >"$P/app.js" <<'JS'
import { addTask, toggleTask } from "./tasks.js";

let tasks = JSON.parse(localStorage.getItem("tasks") || "[]");
const list = document.getElementById("tasks");

function render() {
  localStorage.setItem("tasks", JSON.stringify(tasks));
  list.innerHTML = "";
  for (const t of tasks) {
    const li = document.createElement("li");
    li.className = t.done ? "done" : "";
    li.innerHTML = `<input type="checkbox" ${t.done ? "checked" : ""}><span></span>`;
    li.querySelector("span").textContent = t.title;
    li.querySelector("input").onchange = () => { tasks = toggleTask(tasks, t.id); render(); };
    list.append(li);
  }
}

document.getElementById("new-task").onsubmit = (e) => {
  e.preventDefault();
  const input = document.getElementById("title");
  if (input.value.trim()) tasks = addTask(tasks, input.value);
  input.value = "";
  render();
};
render();
JS
    printf '# Taskboard\n\nA tiny to-do board.\n' >"$P/README.md"
    git -C "$P" init -q -b main && git -C "$P" add -A && git -C "$P" -c user.name=Demo -c user.email=demo@example.com commit -qm "Taskboard"
    echo "$P"
    ;;
  quit)
    [ -f "$DIR/record.pid" ] && "$0" stop >/dev/null
    send quit 2>/dev/null; sleep 3
    p="$(pid)"; [ -n "$p" ] && kill "$p" 2>/dev/null
    rm -f "$DIR/app.pid"
    say "quit"
    ;;
  *) sed -n 2,16p "$0"; exit 1 ;;
esac
