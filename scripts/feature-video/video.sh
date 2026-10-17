#!/bin/bash
# Runs video.py with Pillow and edge-tts from a virtualenv kept in ~/.cache/agentty-video (made on first use).
set -euo pipefail
CACHE="${AGENTTY_VIDEO_CACHE:-$HOME/.cache/agentty-video}"
VENV="$CACHE/venv"
if ! "$VENV/bin/python" -c 'import PIL, edge_tts' 2>/dev/null; then
  mkdir -p "$CACHE"
  python3 -m venv "$VENV"
  "$VENV/bin/pip" install -q --upgrade pip
  "$VENV/bin/pip" install -q 'pillow>=11,<12' 'edge-tts>=7,<8'
fi
command -v ffmpeg >/dev/null || { echo "ffmpeg is missing: brew install ffmpeg" >&2; exit 1; }
exec "$VENV/bin/python" "$(dirname "$0")/video.py" "$@"
