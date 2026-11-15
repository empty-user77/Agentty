#!/bin/bash
# Idle check of a debug build: how often its window draws itself, how much CPU it takes and
# whether its memory grows while nothing happens. Run before every commit, push and hand-off
# (CLAUDE.md → Performance); it fails on a window that keeps drawing, a busy idle app or a leak.
#
#   scripts/perf-check.sh            # builds target/debug/agentty if needed, then measures
#   PERF_SECONDS=30 scripts/perf-check.sh
#
# The app starts with AGENTTY_BACKGROUND=1 (it takes no focus and no clicks from the app in use),
# its own data folder in a temp dir, and one shell workspace. macOS only: elsewhere it says so and
# passes.
set -euo pipefail

if [[ "$(uname)" != "Darwin" ]]; then
  echo "perf-check: macOS only, skipped"
  exit 0
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${PERF_BIN:-$ROOT/target/debug/agentty}"
SECONDS_IDLE="${PERF_SECONDS:-20}"
# Limits for an idle window in the background (debug build).
MAX_RENDERS_PER_SEC="${PERF_MAX_RENDERS_PER_SEC:-1}"
MAX_CPU_PERCENT="${PERF_MAX_CPU_PERCENT:-3}"
MAX_RSS_GROWTH_MB="${PERF_MAX_RSS_GROWTH_MB:-30}"

if [[ ! -x "$BIN" || -z "${PERF_BIN:-}" ]]; then
  (cd "$ROOT" && cargo build -q -p agentty-app)
fi

WORK="$(mktemp -d "${TMPDIR:-/tmp}/agentty-perf.XXXXXX")"
mkdir -p "$WORK/data" "$WORK/project"
printf '{"onboardingDone": true}' > "$WORK/data/settings.json"
LOG="$WORK/app.log"

cd "$WORK/project"
AGENTTY_DEBUG=1 AGENTTY_BACKGROUND=1 AGENTTY_DATA_DIR="$WORK/data" "$BIN" > "$LOG" 2>&1 &
PID=$!
cleanup() {
  kill "$PID" 2>/dev/null || true
  sleep 1
  kill -9 "$PID" 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT

SOCK=""
for _ in $(seq 1 60); do
  SOCK="$(ls -d "${TMPDIR%/}"/agentty-"$PID"-*/s.sock 2>/dev/null | head -1 || true)"
  [[ -n "$SOCK" ]] && break
  sleep 0.5
done
if [[ -z "$SOCK" ]]; then
  echo "perf-check: the app did not start (no socket)"; cat "$LOG"; exit 1
fi

send() { printf 'debug\t%s\t%s\n' "$1" "${2:-}" | nc -U -w 1 "$SOCK" >/dev/null 2>&1 || true; }
renders() {
  local before; before=$(wc -l < "$LOG")
  send perf
  sleep 0.5
  tail -n +$((before + 1)) "$LOG" | { grep -a -o 'renders=[0-9]*' || true; } | tail -1 | cut -d= -f2
}
# CPU time used so far, in seconds (`ps` prints [[dd-]hh:]mm:ss.cc).
cpu_seconds() {
  ps -o time= -p "$PID" | awk -F'[:-]' '{ s = 0; for (i = 1; i <= NF; i++) s = s * 60 + $i; print s }'
}
rss_mb() { ps -o rss= -p "$PID" | awk '{ printf "%d", $1 / 1024 }'; }

send workspace "shell $WORK/project"
# Let start-up work (shell start, agent detection, first reads) settle.
sleep "${PERF_SETTLE:-25}"

r0=$(renders); c0=$(cpu_seconds); m0=$(rss_mb)
sleep "$SECONDS_IDLE"
r1=$(renders); c1=$(cpu_seconds); m1=$(rss_mb)
send quit

renders_per_sec=$(awk -v a="$r0" -v b="$r1" -v t="$SECONDS_IDLE" 'BEGIN { printf "%.2f", (b - a) / t }')
cpu_percent=$(awk -v a="$c0" -v b="$c1" -v t="$SECONDS_IDLE" 'BEGIN { printf "%.1f", (b - a) * 100 / t }')
growth=$((m1 - m0))

fail=0
check() { # name value limit unit
  if awk -v v="$2" -v l="$3" 'BEGIN { exit !(v > l) }'; then
    printf '  ✗ %-22s %s %s (limit %s)\n' "$1" "$2" "$4" "$3"; fail=1
  else
    printf '  ✓ %-22s %s %s (limit %s)\n' "$1" "$2" "$4" "$3"
  fi
}
echo "perf-check: ${SECONDS_IDLE}s idle, one shell workspace, window in the background"
check "renders per second" "$renders_per_sec" "$MAX_RENDERS_PER_SEC" "/s"
check "CPU" "$cpu_percent" "$MAX_CPU_PERCENT" "%"
check "memory growth" "$growth" "$MAX_RSS_GROWTH_MB" "MB (RSS ${m1} MB)"
exit $fail
