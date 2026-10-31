"""Turns what drive.sh sent during a recording into footage/<name>.mov.events.json.

    events.py <video dir> <stop ms>

Times become footage seconds. screencapture starts writing a moment after it is launched; that
delay is the wall-clock length of the recording minus the file's length (it stops promptly on
SIGINT), and is taken off every event. Positions stay in window points, with the footage's pixels
per point beside them.
"""

import json
import re
import subprocess
import sys
from pathlib import Path

folder, stopped = Path(sys.argv[1]), int(sys.argv[2])
name, width, height = (folder / "record.meta").read_text().split()[:3]
start = int((folder / "record.start").read_text().strip())
footage = folder / "footage" / f"{name}.mov"
probe = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width:format=duration",
                        "-of", "json", str(footage)], capture_output=True, text=True)
info = json.loads(probe.stdout or "{}")
length = float(info.get("format", {}).get("duration", 0) or 0)
pixels = int(info.get("streams", [{}])[0].get("width", width))
lag = max(0.0, (stopped - start) / 1000 - length) if length else 0.0

events = []
for row in (folder / "record.events").read_text().splitlines():
    parts = row.split("\t")
    if len(parts) < 2:
        continue
    ms, cmd, arg = int(parts[0]), parts[1], parts[2] if len(parts) > 2 else ""
    named = parts[3].strip() if len(parts) > 3 else ""
    t = round((ms - start) / 1000 - lag, 3)
    if t < 0 or (length and t > length):
        continue
    e = {"t": t}
    xy = re.match(r"\s*(-?[\d.]+)\s+(-?[\d.]+)\s*(\w*)", arg)
    if cmd == "click" and xy:
        mod = xy.group(3)
        e.update(kind={"double": "double", "right": "right"}.get(mod, "click"), x=float(xy.group(1)), y=float(xy.group(2)))
        if mod and mod not in ("double", "right"):
            e["mod"] = mod
        if named:
            e["label"] = named
    elif cmd in ("hover", "drag-to") and xy:
        e.update(kind="hover" if cmd == "hover" else "drag", x=float(xy.group(1)), y=float(xy.group(2)))
    elif cmd == "mark":
        e.update(kind="mark", label=arg)
    elif cmd in ("type", "prompt", "ime"):
        e.update(kind="type", label=arg[:80])
    elif cmd in ("key", "keystroke"):
        e.update(kind="key", label=arg)
    else:
        e.update(kind="cmd", label=f"{cmd} {arg}".strip()[:80])
    events.append(e)

out = footage.with_name(footage.name + ".events.json")
out.write_text(json.dumps({"window": [int(width), int(height)], "pixels_per_point": round(pixels / int(width), 4),
                           "lag": round(lag, 3), "events": events}, indent=1))
print(f"{len(events)} event(s), start lag {lag:.2f}s -> {out}")
