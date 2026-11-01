"""Builds a feature video as a HyperFrames project (HTML + GSAP, rendered by headless Chrome).

    video.sh film <storyboard.json> [--out DIR] [--langs en,ko] [--project DIR] [--no-render]

What it adds over the plain ffmpeg build: a camera that eases into what is happening (from the
clicks the driver logged, or a storyboard rect), an oversized cursor that walks to every click and
causes it, headlines that perform their entrance, word-by-word captions timed by the voice itself,
transitions with a cause between scenes, more scene layouts (title, stat, steps, quote, split,
picture-in-picture) and vertical or square cuts of the same storyboard.

Every project is plain files under the work folder (`--project`): open one in
`npx hyperframes preview` to look or edit by hand, then build again.
"""

import asyncio
import hashlib
import html
import json
import math
import os
import re
import shutil
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import motion
import video as v

HF_VERSION = "0.8.207"
GSAP = "https://cdn.jsdelivr.net/npm/gsap@3.14.2/dist/gsap.min.js"
ASPECTS = {"16:9": (1920, 1080), "9:16": (1080, 1920), "1:1": (1080, 1080)}
# Opening and end card lengths (seconds), the same in every video.
INTRO, OUTRO = 4.2, 4.6
# A scene's host outlives its slot by this much: the outgoing half of the seam after it.
HOST_TAIL = 0.6
# The house transition: a velocity-matched cut to the left (HyperFrames' "cut the curve").
DEFAULT_SEAM = "curve-left"
SEAMS = {"curve-left", "curve-right", "curve-up", "zoom-through", "inverse-zoom", "blur-swap", "dip", "whip", "iris", "cut"} | motion.FX_SEAMS
LAYOUTS = {"window", "full", "split", "pip", "title", "stat", "steps", "quote", "device", "hero3d", "mosaic", "bento", "kinetic", "lower3rd"}
# Faces by language, by PostScript name (Chrome finds installed fonts by it).
FACES = {
    "en": ("AvenirNext-DemiBold", "AvenirNext-Regular"),
    "ko": ("AppleSDGothicNeo-Bold", "AppleSDGothicNeo-Regular"),
    "ja": ("HiraginoSans-W6", "HiraginoSans-W3"),
    "zh": ("PingFangSC-Semibold", "PingFangSC-Regular"),
}
CJK = {"ja", "zh"}


def esc(text):
    return html.escape(text or "", quote=True)


def hex_rgb(rgb):
    return "#" + "".join(f"{c:02x}" for c in rgb)


def js(value):
    return json.dumps(value, ensure_ascii=False)


# ---------------------------------------------------------------- narration with word timings


def speak(text, voice, rate):
    """The line spoken (mp3, cached) and when each word starts and ends, from the voice service itself."""
    key = hashlib.sha1(f"{voice}|{rate}|{text}|words".encode()).hexdigest()[:16]
    mp3, meta = v.CACHE / "tts" / f"{key}.mp3", v.CACHE / "tts" / f"{key}.words.json"
    if not (mp3.exists() and meta.exists()):
        import edge_tts

        mp3.parent.mkdir(parents=True, exist_ok=True)

        async def go():
            words, audio = [], bytearray()
            async for chunk in edge_tts.Communicate(text, voice, rate=rate, boundary="WordBoundary").stream():
                if chunk["type"] == "audio":
                    audio += chunk["data"]
                elif chunk["type"] == "WordBoundary":
                    a = chunk["offset"] / 1e7
                    words.append({"w": chunk["text"], "a": round(a, 3), "b": round(a + chunk["duration"] / 1e7, 3)})
            return bytes(audio), words

        audio, words = asyncio.run(go())
        part = mp3.with_suffix(".part.mp3")
        part.write_bytes(audio)
        part.rename(mp3)
        meta.write_text(json.dumps(words, ensure_ascii=False))
    return mp3, v.duration(mp3), json.loads(meta.read_text())


# ---------------------------------------------------------------- footage, events and marks


def events_of(src):
    """What the driver did while recording (`<footage>.events.json`): clicks, keys and marks in
    footage seconds, positions in footage pixels."""
    path = src.with_name(src.name + ".events.json")
    if not path.exists():
        return []
    data = json.loads(path.read_text())
    k = data.get("pixels_per_point", 1)
    out = []
    for e in data.get("events", []):
        e = dict(e)
        if "x" in e:
            e["x"], e["y"] = e["x"] * k, e["y"] * k
        out.append(e)
    return out


def resolve_time(value, events):
    """A time in footage seconds: a number, or `"@label"` / `"@label+1.5"` for a mark the driver logged."""
    if isinstance(value, (int, float)):
        return float(value)
    # Mark names are letters, digits, `_` and `.`; a `-` starts the offset ("@result-1").
    m = re.fullmatch(r"@([A-Za-z_][\w.]*)\s*([+-]\s*[\d.]+)?", str(value).strip())
    if not m:
        sys.exit(f"not a time: {value!r} (a number, or @mark, or @mark+1.5)")
    hits = [e["t"] for e in events if e.get("kind") == "mark" and e.get("label") == m.group(1)]
    if not hits:
        sys.exit(f"no mark {m.group(1)!r} in the footage's events")
    return hits[0] + float((m.group(2) or "0").replace(" ", ""))


def probe_size(path):
    out = v.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "csv=p=0", str(path)])
    w, h = out.strip().split(",")[:2]
    return int(w), int(h)


def pieces_of(scene, events):
    """The footage stretches a scene plays: `[[from, to, speed], …]` or one `from`/`to`/`speed`."""
    if "pieces" in scene:
        return [(resolve_time(p[0], events), resolve_time(p[1], events), float(p[2]) if len(p) > 2 else 1.0) for p in scene["pieces"]]
    a = resolve_time(scene.get("from", 0), events)
    b = resolve_time(scene["to"], events) if "to" in scene else None
    return [(a, b, float(scene.get("speed", 1)))]


def prep_clip(src, pieces, regions, out, max_w):
    """One H.264 file of the stretches, sped up, private text blurred in, no wider than `max_w`."""
    sw, sh = probe_size(src)
    k = min(1.0, max_w / sw)
    tw, th = round(sw * k / 2) * 2, round(sh * k / 2) * 2
    graph, labels, mapping, t = [], [], [], 0.0
    graph.append(f"[0:v]fps={v.FPS},split={len(pieces)}" + "".join(f"[s{i}]" for i in range(len(pieces))))
    total = v.duration(src)
    for i, (a, b, speed) in enumerate(pieces):
        b = total if b is None else b
        graph.append(f"[s{i}]trim=start={a}:end={b},setpts=(PTS-STARTPTS)/{speed}[p{i}]")
        labels.append(f"[p{i}]")
        mapping.append((a, b, speed, t))
        t += (b - a) / speed
    graph.append(f"{''.join(labels)}concat=n={len(pieces)}:v=1:a=0,fps={v.FPS}[cat]")
    # Blur regions are in source seconds: move them to clip seconds, piece by piece.
    moved = []
    for r in regions:
        lo, hi = float(r.get("from", 0)), float(r.get("to", 1e9))
        for a, b, speed, at in mapping:
            x, y = max(lo, a), min(hi, b)
            if x < y:
                moved.append({"rect": r["rect"], "from": at + (x - a) / speed, "to": at + (y - a) / speed})
    blur = v.blur_graph(moved, 0.0, 1.0, 1.0, "[cat]")
    tail = "[src]" if blur else "[cat]"
    graph = ";".join(graph) + (";" + blur if blur else "") + f";{tail}scale={tw}:{th}:flags=lanczos,format=yuv420p[v]"
    v.run(["ffmpeg", "-v", "error", "-y", "-i", str(src), "-filter_complex", graph, "-map", "[v]", "-an",
           "-c:v", "libx264", "-preset", "medium", "-crf", "15", "-r", str(v.FPS), str(out)])
    return t, (tw, th), k, mapping


def scrub(clip):
    """A second privacy pass on a prepared clip: text recognition every 0.25 s (the footage scan
    reads every 0.5 s and can miss a line that scrolls past), blurring whatever it finds. Returns
    how many places it blurred."""
    patterns = v.private_patterns()
    regions = []
    for name, t, (x, y, w, h), _, _ in v.private_hits(v.read_words(str(clip), 0.25), patterns):
        box = [x - 12, y - 10, w + 24, h + 20]
        for r in regions:
            rx, ry, rw, rh = r["rect"]
            if abs(rx - box[0]) < 40 and abs(ry - box[1]) < 24 and t - r["to"] <= 0.5:
                r["rect"] = [min(rx, box[0]), min(ry, box[1]), max(rx + rw, box[0] + box[2]) - min(rx, box[0]), max(ry + rh, box[1] + box[3]) - min(ry, box[1])]
                r["to"] = t + 0.4
                break
        else:
            regions.append({"rect": box, "from": max(0.0, t - 0.4), "to": t + 0.4})
    if regions:
        tmp = clip.with_suffix(".scrub.mp4")
        graph = v.blur_graph(regions, 0.0, 1.0, 1.0, "[0:v]") + ";[src]format=yuv420p[v]"
        v.run(["ffmpeg", "-v", "error", "-y", "-i", str(clip), "-filter_complex", graph, "-map", "[v]", "-an",
               "-c:v", "libx264", "-preset", "medium", "-crf", "15", "-r", str(v.FPS), str(tmp)])
        tmp.replace(clip)
    return len(regions)


def to_clip_time(t, mapping):
    for a, b, speed, at in mapping:
        if a <= t <= b:
            return at + (t - a) / speed
    return None


# ---------------------------------------------------------------- the camera and the cursor


def fit_aspect(rect, aspect, bounds):
    """`rect` grown to `aspect` (w/h) around its centre, kept inside `bounds` (w, h)."""
    x, y, w, h = rect
    bw, bh = bounds
    if w / h < aspect:
        w2, h2 = h * aspect, h
    else:
        w2, h2 = w, w / aspect
    if w2 > bw:
        w2, h2 = bw, bw / aspect
    if h2 > bh:
        w2, h2 = bh * aspect, bh
    cx, cy = x + w / 2, y + h / 2
    x2 = min(max(cx - w2 / 2, 0), bw - w2)
    y2 = min(max(cy - h2 / 2, 0), bh - h2)
    return [round(x2, 1), round(y2, 1), round(w2, 1), round(h2, 1)]


def camera_moves(scene, events, clip_len, size, aspect, k):
    """Camera keyframes: (start, duration, view rect in clip pixels, ease).

    `zoom` is a rect `{rect, at, back}` (or a list of them) in source pixels and clip seconds, or
    "auto": into each burst of clicks the driver logged, held while the UI acts, then back out."""
    cw, ch = size
    whole = fit_aspect([0, 0, cw, ch], aspect, size)
    if scene.get("crop"):
        whole = fit_aspect([c * k for c in scene["crop"]], aspect, size)
    moves = [(0.0, 0.0, whole, "none")]
    if scene.get("reveal"):
        # Open tight on a detail, hold it, then one decelerating pull-back to the whole window.
        detail = fit_aspect([c * k for c in scene["reveal"]], aspect, size)
        t0 = float(scene.get("reveal_at", 1.0))
        moves = [(0.0, 0.0, detail, "none"), (t0, 1.3, whole, "expo.inOut")]
    zoom = scene.get("zoom")
    targets = []
    if zoom == "auto":
        clicks = [e for e in events if e["kind"] in ("click", "double", "right", "drag")]
        bursts = []
        for e in clicks:
            if bursts and e["t"] - bursts[-1][-1]["t"] < 2.2:
                bursts[-1].append(e)
            else:
                bursts.append([e])
        depth = float(scene.get("zoom_depth", 1.6))
        for burst in bursts:
            xs, ys = [e["x"] for e in burst], [e["y"] for e in burst]
            w = max(max(xs) - min(xs) + cw / depth * 0.6, cw / depth)
            h = max(max(ys) - min(ys) + ch / depth * 0.6, ch / depth)
            rect = [(min(xs) + max(xs)) / 2 - w / 2, (min(ys) + max(ys)) / 2 - h / 2, w, h]
            t0 = max(0.0, burst[0]["t"] - 0.75)
            t1 = min(clip_len - 0.8, burst[-1]["t"] + 2.4)
            if targets and t0 < targets[-1]["back"][0] + 1.2:
                # The next burst comes soon: travel there instead of pulling out and pushing in again.
                prev = targets[-1]
                prev["back"] = None
                targets.append({"rect": rect, "at": [t0, t0 + 0.6], "back": [t1, t1 + 0.6]})
                continue
            targets.append({"rect": rect, "at": [t0, t0 + 0.6], "back": [t1, t1 + 0.6]})
        if targets and targets[-1]["back"] and targets[-1]["back"][0] > clip_len - 0.9:
            # Out again before the cut, so the next scene starts from the whole window.
            targets[-1]["back"] = [max(targets[-1]["at"][1] + 0.4, clip_len - 1.0), max(targets[-1]["at"][1] + 1.0, clip_len - 0.4)]
    elif zoom:
        for z in zoom if isinstance(zoom, list) else [zoom]:
            targets.append({"rect": [c * k for c in z["rect"]], "at": z.get("at", [0.5, 1.1]), "back": z.get("back")})
    for z in targets:
        view = fit_aspect(z["rect"], aspect, size)
        a0, a1 = z["at"]
        moves.append((a0, max(0.2, a1 - a0), view, "power2.inOut"))
        if z.get("back"):
            b0, b1 = z["back"]
            moves.append((b0, max(0.2, b1 - b0), whole, "power2.inOut"))
    return moves


def cursor_path(events, clip_len):
    """Where the oversized cursor goes: in from below to the first target, on to each click
    (arriving before it), a tap on every click, then out of the frame."""
    points = [e for e in events if e["kind"] in ("click", "double", "right", "hover", "drag")]
    if not points:
        return []
    steps = []
    for i, e in enumerate(points):
        prev = steps[-1]["t"] if steps else max(0.0, e["t"] - 1.2)
        arrive = e["t"] - 0.05
        travel = min(0.85 if not steps else 0.6, max(0.25, arrive - prev - 0.1))
        steps.append({"t": round(e["t"], 3), "x": e["x"], "y": e["y"], "go": round(max(0.0, arrive - travel), 3),
                      "dur": round(travel, 3), "tap": e["kind"] in ("click", "double", "right"),
                      "double": e["kind"] == "double", "first": i == 0})
    leave = min(clip_len - 0.1, steps[-1]["t"] + 1.6)
    return {"steps": steps, "leave": round(leave, 3)}


# ---------------------------------------------------------------- captions


def chunks(words, lang, limit_words=6, limit_chars=16):
    """A spoken line in caption groups of a few words, broken at pauses and punctuation."""
    groups, cur = [], []
    for i, w in enumerate(words):
        cur.append(w)
        nxt = words[i + 1] if i + 1 < len(words) else None
        size = sum(len(x["w"]) for x in cur) if lang in CJK else len(cur)
        limit = limit_chars if lang in CJK else limit_words
        pause = nxt is not None and nxt["a"] - w["b"] >= 0.15
        if nxt is None or size >= limit or pause or re.search(r"[.,!?;:。、，！？]$", w["w"]):
            groups.append(cur)
            cur = []
    return groups


def caption_groups(text, lang, words, start, length):
    """Caption groups for one line: word by word when the voice speaks this language (its timings
    are exact), else the line itself split evenly across the time it is spoken."""
    if words:
        # The voice service drops punctuation from its words; put it back from the text so the
        # caption reads as written.
        out, pos = [], 0
        for w in words:
            j = text.find(w["w"], pos)
            if j < 0:
                out.append(dict(w))
                continue
            end = j + len(w["w"])
            while end < len(text) and not text[end].isspace() and not text[end].isalnum():
                end += 1
            out.append({**w, "w": text[j:end]})
            pos = end
        # Characters the words skip (CJK punctuation, quotes) stay where they were.
        groups = chunks(out, lang)
        return [{"a": start + g[0]["a"] - 0.05, "b": start + g[-1]["b"] + 0.12,
                 "words": [{"w": w["w"], "a": start + w["a"]} for w in g]} for g in groups]
    parts = re.split(r"(?<=[.,!?。、，！？])\s*", text) if len(text) > (24 if lang in CJK else 60) else [text]
    parts = [p for p in parts if p.strip()]
    # A scrap of a few characters ("テスト。") would flash by: it joins the part before it.
    merged = []
    for p in parts:
        if merged and len(p.strip()) < (6 if lang in CJK else 12):
            merged[-1] += ("" if lang in CJK else " ") + p
        else:
            merged.append(p)
    parts = merged
    total = sum(len(p) for p in parts) or 1
    out, t = [], start
    for p in parts:
        d = length * len(p) / total
        out.append({"a": t, "b": t + d, "words": None, "text": p})
        t += d
    return out


# ---------------------------------------------------------------- planning


def plan(board, base, langs, work):
    """Every scene's length, narration and prepared footage, shared by all languages."""
    voice_mode = board.get("narration", "en")
    rate = board.get("rate", "+0%")
    voices = {**v.VOICES, **board.get("voices", {})}
    spoken = langs if voice_mode == "each" else ["en"]
    aspect_name = board.get("aspect", "16:9")
    if aspect_name not in ASPECTS:
        sys.exit(f"aspect must be one of {', '.join(ASPECTS)}")
    W, H = ASPECTS[aspect_name]
    assets = work / "shared"
    assets.mkdir(parents=True, exist_ok=True)
    scenes = []
    t = INTRO
    for i, scene in enumerate(board["scenes"]):
        sid = f"s{i + 1:02}"
        layout = scene.get("layout", "full" if scene.get("full") else "window")
        if layout not in LAYOUTS:
            sys.exit(f"scene {i + 1}: layout must be one of {', '.join(sorted(LAYOUTS))}")
        seam = scene.get("transition", DEFAULT_SEAM if i else "blur-swap")
        if seam not in SEAMS:
            sys.exit(f"scene {i + 1}: transition must be one of {', '.join(sorted(SEAMS))}")
        lines, cursor = [], 0.35
        for say in scene.get("say", []):
            takes = {lang: speak(v.text_of(say, lang), voices[lang], rate) for lang in spoken}
            slot = max(d for _, d, _ in takes.values())
            lines.append({"start": cursor, "len": slot, "say": say, "takes": takes})
            cursor += slot + 0.25
        spoken_len = cursor - 0.25 + 0.3 if lines else 0
        clips = []
        for n, ref in enumerate(scene.get("clips") or ([scene] if scene.get("clip") or scene.get("image") else [])):
            src = base / (ref.get("clip") or ref.get("image"))
            if not src.exists():
                sys.exit(f"missing footage: {src}")
            events = events_of(src)
            if ref.get("image"):
                sw, sh = v.Image.open(src).size
                dst = assets / f"{sid}-{n}{src.suffix}"
                shutil.copy(src, dst)
                clips.append({"file": dst.name, "kind": "image", "len": float(ref.get("seconds", 3)), "size": (sw, sh), "k": 1.0,
                              "events": [], "scene": ref})
                continue
            pieces = pieces_of(ref, events)
            regions = v.blur_regions(src, ref)
            dst = assets / f"{sid}-{n}.mp4"
            max_w = 2 * W if layout in ("window", "full", "pip") else W
            key = hashlib.sha1(json.dumps(["scrub", str(src), src.stat().st_mtime, pieces, regions, max_w]).encode()).hexdigest()[:12]
            stamp = dst.with_suffix(".key")
            if stamp.exists() and stamp.read_text() == key and dst.exists():
                length, size, k, mapping = json.loads(dst.with_suffix(".meta").read_text())
            else:
                length, size, k, mapping = prep_clip(src, pieces, regions, dst, max_w)
                found = scrub(dst)
                if found:
                    print(f"  scene {i + 1}: blurred {found} more private place(s) the footage scan missed")
                stamp.write_text(key)
                dst.with_suffix(".meta").write_text(json.dumps([length, size, k, mapping]))
            moved = []
            for e in events:
                at = to_clip_time(e["t"], mapping)
                if at is not None and e["kind"] != "mark":
                    moved.append({**e, "t": round(at, 3), **({"x": e["x"] * k, "y": e["y"] * k} if "x" in e else {})})
            clips.append({"file": dst.name, "kind": "video", "len": length, "size": tuple(size), "k": k, "events": moved, "scene": ref})
        footage = max((c["len"] for c in clips), default=0)
        if scene.get("fit") and spoken_len and clips and clips[0]["kind"] == "video":
            sys.exit(f"scene {i + 1}: `fit` is not supported by `film`; set `speed` or `pieces` instead")
        # A card holds long enough to be read: 0.5 s plus 0.3 s a word, in its longest language.
        def read_time(*keys):
            longest = 0
            for lang in langs:
                text = " ".join(v.text_of(scene.get(k), lang) or "" for k in keys)
                n = len(re.sub(r"\s", "", text)) / 3 if lang in CJK else len(text.split())
                longest = max(longest, n)
            return 0.5 + 0.3 * longest + 0.6
        def kinetic_time():
            # Each word slams in (about 0.4 s), then the whole line holds to be read.
            def words(lang):
                text = motion.plain(v.text_of(scene.get("headline"), lang) or "")
                return len(re.findall(r"\S{1,4}", text)) if lang in CJK else len(text.split())
            return 0.4 * max(words(lang) for lang in langs) + read_time("headline")
        body = {"title": read_time("headline", "subline"), "stat": max(2.8, read_time("label", "subline")),
                "quote": read_time("quote", "by"), "steps": 0.8 + 0.9 * len(scene.get("items", [])),
                "kinetic": kinetic_time(), "bento": max(3.0, read_time("headline", "label"))}.get(layout, 0)
        seconds = round(max(footage, spoken_len, body, float(scene.get("min", 0))), 3)
        scenes.append({"id": sid, "i": i, "scene": scene, "layout": layout, "seam": seam, "start": round(t, 3),
                       "len": seconds, "lines": lines, "clips": clips})
        print(f"scene {i + 1}: {seconds:.1f}s ({layout}, {seam}; footage {footage:.1f}s, narration {spoken_len:.1f}s)")
        t += seconds
    return {"W": W, "H": H, "aspect": aspect_name, "scenes": scenes, "body_end": round(t, 3),
            "total": round(t + OUTRO, 3), "spoken": spoken, "voice_mode": voice_mode,
            "theme": motion.theme_of(board)[0], "chapters": bool(board.get("chapters"))}


# ---------------------------------------------------------------- HTML


CURSOR_SVG = ('<svg viewBox="0 0 24 24" class="cur-svg"><path d="M4 2 L4 20 L8.6 15.6 L11.6 22 L14.6 20.7 L11.7 14.4 L18 14.4 Z" '
              'fill="#ffffff" stroke="#111318" stroke-width="1.3" stroke-linejoin="round"/></svg>')


def font_faces(lang):
    bold, regular = FACES[lang]
    latin_b, latin_r = FACES["en"]
    return (f'@font-face {{ font-family: "Display"; src: local("{latin_b}"); font-weight: 600; unicode-range: U+0000-024F, U+2000-206F, U+20AC, U+2190-21FF, U+2318; }}\n'
            f'@font-face {{ font-family: "Display"; src: local("{bold}"); font-weight: 600; }}\n'
            f'@font-face {{ font-family: "Body"; src: local("{latin_r}"); font-weight: 400; unicode-range: U+0000-024F, U+2000-206F, U+20AC, U+2190-21FF, U+2318; }}\n'
            f'@font-face {{ font-family: "Body"; src: local("{regular}"); font-weight: 400; }}\n')


def words_html(text, lang):
    """A headline as spans to animate (see motion.words_html: `*word*` is the accent)."""
    return motion.words_html(text, lang, CJK)


def stage_box(layout, W, H, has_head):
    """Where the footage sits: (x, y, w, h) on the canvas."""
    if layout == "full":
        return (0, 0, W, H)
    if W > H:  # landscape
        top = 186 if has_head else 90
        h = H - top - 46
        w = round(h * 16 / 9)
        if w > W - 200:
            w = W - 200
            h = round(w * 9 / 16)
        return ((W - w) // 2, top, w, h)
    if W < H:  # portrait: a tall window under the headline
        top = 330 if has_head else 160
        return (48, top, W - 96, H - top - 330)
    top = 210 if has_head else 90
    return (60, top, W - 120, H - top - 170)


ENTRANCES = ["words-blur", "words-ink", "line-blur", "letters"]


def headline_tweens(prefix, style, at, ink="var(--ink)"):
    return motion.title_fx(f"#{prefix}-h", style, at, ink)


def clip_layer(prefix, clip, box, scene_len, cam_moves=None, cursor=None, spotlight=None, cls="stage", lang="en", opts=None):
    """A video or image in a stage box with a camera wrapper and, optionally, the cursor and
    spotlights. `opts`: `enter` (how the window arrives), `sweep` (a light pass as it lands),
    `brackets` (corner brackets close in), `ripple` (a ring on each click)."""
    opts = opts or {}
    x, y, w, h = box
    cw, ch = clip["size"]
    aspect = w / h
    view0 = cam_moves[0][2] if cam_moves else fit_aspect([0, 0, cw, ch], aspect, (cw, ch))
    s0 = w / view0[2]
    # The picture lasts as long as the scene's host (its length plus the seam after it), so the
    # outgoing side of a transition still shows its last frame.
    slot = scene_len + HOST_TAIL
    media = (f'<video id="{prefix}-v" class="clip" data-start="0" data-duration="{slot:.3f}" muted playsinline src="assets/{clip["file"]}"></video>'
             if clip["kind"] == "video" else
             f'<img id="{prefix}-v" class="clip" data-start="0" data-duration="{slot:.3f}" src="assets/{clip["file"]}" alt="">')
    cur_px = round(max(64, min(118, w * 0.058)) / s0, 1)  # big: about 6% of the stage, in clip pixels
    cur = f'<div id="{prefix}-cur" class="cur" style="width:{cur_px}px;height:{cur_px}px">{CURSOR_SVG}</div>' if cursor else ""
    spots, tl = "", ""
    for n, sp in enumerate(spotlight or []):
        # For a held framing: the rect in stage pixels at the scene's opening view.
        rx, ry, rw, rh = sp["rect"]
        sx, sy, sw, sh = (rx - view0[0]) * s0, (ry - view0[1]) * s0, rw * s0, rh * s0
        a0, a1 = sp["at"]
        shape = motion.callout_shape(prefix, n, sp.get("style"), (sx, sy, sw, sh), a0, a1)
        if shape:
            spots += shape[0]
            tl += shape[1]
        else:
            spots += f'<div id="{prefix}-sp{n}" class="spot" style="left:{sx - 8:.0f}px;top:{sy - 8:.0f}px;width:{sw + 16:.0f}px;height:{sh + 16:.0f}px"></div>'
            tl += (f'tl.fromTo("#{prefix}-sp{n}", {{opacity: 0}}, {{opacity: 1, duration: 0.35, ease: "power2.out"}}, {a0:.3f});\n'
                   f'tl.to("#{prefix}-sp{n}", {{opacity: 0, duration: 0.22, ease: "power2.in"}}, {max(a0 + 0.4, a1 - 0.22):.3f});\n')
        label = v.text_of(sp.get("label"), lang)
        if label:
            below = sy + sh + 90 < h
            ly = sy + sh + 22 if below else sy - 70
            spots += f'<div id="{prefix}-sl{n}" class="pill" data-layout-allow-overlap style="left:{min(max(sx + sw / 2, 160), w - 160):.0f}px;top:{ly:.0f}px">{esc(label)}</div>'
            tl += (f'tl.fromTo("#{prefix}-sl{n}", {{opacity: 0, y: {12 if below else -12}, filter: "blur(6px)"}}, {{opacity: 1, y: 0, filter: "blur(0px)", duration: 0.3, ease: "expo.out"}}, {a0 + 0.15:.3f});\n'
                   f'tl.to("#{prefix}-sl{n}", {{opacity: 0, filter: "blur(6px)", duration: 0.18, ease: "power2.in"}}, {max(a0 + 0.5, a1 - 0.18):.3f});\n')
    ripples = ""
    if cursor and opts.get("ripple"):
        for n, st in enumerate(s for s in cursor["steps"] if s["tap"]):
            m, j = motion.ripple_fx(prefix, n, st["x"], st["y"], st["t"], 150 / s0)
            ripples += m
            tl += j
    extra = ""
    if opts.get("sweep"):
        m, j = motion.sweep_fx(prefix, w, h, 0.25)
        extra += m
        tl += j
    if opts.get("brackets"):
        m, j = motion.brackets_fx(prefix, w, h, 0.1)
        extra += m
        tl += j
    markup = (f'<div id="{prefix}-stage" class="{cls}" style="left:{x}px;top:{y}px;width:{w}px;height:{h}px">'
              f'<div id="{prefix}-cam" class="cam" style="width:{cw}px;height:{ch}px">{media}{ripples}{cur}</div>{spots}{extra}</div>')
    tl = f'tl.set("#{prefix}-cam", {{x: {-view0[0] * s0:.2f}, y: {-view0[1] * s0:.2f}, scale: {s0:.5f}}}, 0);\n' + tl
    tl += motion.enter_fx(f"#{prefix}-stage", opts.get("enter"), 0.0)
    for start, dur, view, ease_ in (cam_moves or [])[1:]:
        s = w / view[2]
        tl += (f'tl.to("#{prefix}-cam", {{x: {-view[0] * s:.2f}, y: {-view[1] * s:.2f}, scale: {s:.5f}, '
               f'duration: {dur:.3f}, ease: "{ease_}"}}, {start:.3f});\n')
    if cursor:
        steps = cursor["steps"]
        first = steps[0]
        tl += f'tl.set("#{prefix}-cur", {{x: {first["x"] + cw * 0.08:.1f}, y: {ch * 1.25:.1f}}}, 0);\n'
        for st in steps:
            tl += (f'tl.to("#{prefix}-cur", {{x: {st["x"]:.1f}, y: {st["y"]:.1f}, duration: {st["dur"]}, '
                   f'ease: "{"power3.out" if st["first"] else "power2.inOut"}"}}, {st["go"]});\n')
            if st["tap"]:
                for rep in range(2 if st["double"] else 1):
                    t = st["t"] + rep * 0.28
                    tl += (f'tl.to("#{prefix}-cur", {{scale: 0.84, duration: 0.08, ease: "power2.in", transformOrigin: "21% 14%"}}, {max(0, t - 0.08):.3f});\n'
                           f'tl.to("#{prefix}-cur", {{scale: 1, duration: 0.18, ease: "power2.out", transformOrigin: "21% 14%"}}, {t:.3f});\n')
        tl += f'tl.to("#{prefix}-cur", {{y: {ch * 1.3:.1f}, duration: 0.6, ease: "power2.in"}}, {cursor["leave"]});\n'
    return markup, tl


def view_at(moves, t):
    """The camera's view rect at scene time `t` (the eases taken as smoothsteps)."""
    view = list(moves[0][2])
    for start, dur, target, _ in moves[1:]:
        if t >= start + dur:
            view = list(target)
        elif t > start:
            p = (t - start) / dur
            p = p * p * (3 - 2 * p)
            view = [a + (b - a) * p for a, b in zip(view, target)]
            break
        else:
            break
    return view


def to_canvas(box, view, x, y):
    s = box[2] / view[2]
    return box[0] + (x - view[0]) * s, box[1] + (y - view[1]) * s


def clip_checks(checks, sc, box, clip, moves, spots, lang):
    """What `verify` reads back for a footage clip: each click's target under the cursor, each
    spotlight's label and the text it points at."""
    start, L = sc["start"], sc["len"]
    for e in clip["events"]:
        if e["kind"] not in ("click", "double", "right") or not e.get("label") or e["t"] > L:
            continue
        x, y = to_canvas(box, view_at(moves, e["t"]), e["x"], e["y"])
        checks.append({"kind": "click", "scene": sc["i"] + 1, "t": round(start + max(0.0, e["t"] - 0.12), 3), "x": round(x), "y": round(y),
                       "text": e["label"], "stage": list(box)})
    for sp in spots or []:
        a0, a1 = sp["at"]
        t = (a0 + a1) / 2
        view = view_at(moves, a0)
        x0, y0 = to_canvas(box, view, sp["rect"][0], sp["rect"][1])
        s = box[2] / view[2]
        rect = [round(x0), round(y0), round(sp["rect"][2] * s), round(sp["rect"][3] * s)]
        label = v.text_of(sp.get("label"), lang)
        if label:
            checks.append({"kind": "spot-label", "scene": sc["i"] + 1, "t": round(start + t, 3), "near": rect, "text": label})
        if sp.get("expect"):
            checks.append({"kind": "spot-target", "scene": sc["i"] + 1, "t": round(start + t, 3), "box": rect, "text": v.text_of(sp["expect"], lang)})
    z = sc["scene"].get("zoom")
    for zz in ([z] if isinstance(z, dict) else z if isinstance(z, list) else []):
        if zz.get("expect") and zz.get("at"):
            t = zz["at"][1] + 0.2
            checks.append({"kind": "zoom-target", "scene": sc["i"] + 1, "t": round(start + t, 3), "box": list(box), "text": v.text_of(zz["expect"], lang)})


def scene_html(sc, plan_, lang, k_style, checks=None):
    """One scene as a HyperFrames sub-composition. `checks` collects what `verify` reads back."""
    checks = [] if checks is None else checks
    W, H = plan_["W"], plan_["H"]
    scene, sid, layout, L = sc["scene"], sc["id"], sc["layout"], sc["len"]
    at = lambda t: round(sc["start"] + min(t, L - 0.35), 3)  # a moment inside the scene, before its exit
    n_ = sc["i"] + 1
    text_area = [0, 0, W, H - (420 if W < H else 210)]
    head = v.text_of(scene.get("headline"), lang)
    sub = v.text_of(scene.get("subline"), lang)
    style = scene.get("title_style") or ENTRANCES[k_style % len(ENTRANCES)]
    parts, tl = [], ""
    theme = motion.THEMES[plan_["theme"]]
    has_head = bool(head) and layout not in ("title", "stat", "quote", "kinetic", "bento", "lower3rd")
    portrait = W < H
    if has_head:
        size = 56 if portrait else 62
        parts.append(f'<div id="{sid}-bar" class="bar" style="{"left:48px;top:150px" if portrait else ""}"></div>'
                     f'<div id="{sid}-head" class="head" style="{"left:72px;top:140px;right:48px" if portrait else ""}">'
                     f'<h2 id="{sid}-h" style="font-size:{size}px">{words_html(head, lang)}</h2>'
                     + (f'<p id="{sid}-p">{esc(motion.plain(sub))}</p>' if sub else "") + "</div>")
        # Entrances start on the cut: a frame just after a seam is never empty.
        tl += headline_tweens(sid, style, 0.02)
        tl += f'tl.fromTo("#{sid}-bar", {{scaleY: 0.2}}, {{scaleY: 1, duration: 0.35, ease: "expo.out", transformOrigin: "50% 0%"}}, 0);\n'
        if sub:
            tl += f'tl.fromTo("#{sid}-p", {{opacity: 0, filter: "blur(8px)"}}, {{opacity: 1, filter: "blur(0px)", duration: 0.2, ease: "power4.out"}}, 0.32);\n'
        head_box = [40, 110, W - 80, 290] if portrait else [100, 20, W - 200, 170]
        checks.append({"kind": "headline", "scene": n_, "t": at(1.4), "box": head_box, "text": motion.plain(head)})
        if sub:
            checks.append({"kind": "subline", "scene": n_, "t": at(1.4), "box": head_box, "text": motion.plain(sub)})
    if plan_.get("chapters") and layout not in ("title", "kinetic") and not portrait:
        # "02 / 08": where the viewer is in the film.
        total_n = len(plan_["scenes"])
        parts.append(f'<div id="{sid}-chip" class="chip"><b>{n_:02}</b>/ {total_n:02}</div>')
        tl += f'tl.fromTo("#{sid}-chip", {{opacity: 0, x: 20}}, {{opacity: 1, x: 0, duration: 0.5, ease: "expo.out"}}, 0.15);\n'

    clips = sc["clips"]
    # What the theme and the scene add to a footage window.
    opts = {"enter": scene.get("enter"), "sweep": scene.get("sweep", theme.get("sweep", False)),
            "brackets": scene.get("brackets", theme.get("brackets", False)), "ripple": scene.get("ripple", True)}
    if layout in ("window", "full", "device", "hero3d", "lower3rd"):
        if layout == "device":
            # A laptop: the footage is its screen, a bezel around it and a base under it.
            top = 190 if has_head else 110
            sh_ = H - top - 120
            sw_ = round(sh_ * 16 / 9)
            box = ((W - sw_) // 2, top, sw_, sh_)
            pad = 20
            # One wrapper holds the bezel, the base and the screen, so they rise as one object.
            parts.append(f'<div id="{sid}-dev" class="device" style="left:0;top:0;width:{W}px;height:{H}px">'
                         f'<div class="bezel" style="left:{box[0] - pad}px;top:{box[1] - pad}px;width:{box[2] + 2 * pad}px;height:{box[3] + 2 * pad}px"></div>'
                         f'<div class="base" style="left:{box[0] - pad - box[2] * 0.07:.0f}px;width:{box[2] * 1.14 + 2 * pad:.0f}px;top:{box[1] + box[3] + pad - 2}px;right:auto"></div>')
            tl += (f'tl.fromTo("#{sid}-dev", {{y: 140, rotationX: 22, opacity: 0.2, transformPerspective: 1800, transformOrigin: "50% {(box[1] + box[3] + pad) / H * 100:.1f}%"}}, '
                   f'{{y: 0, rotationX: 0, opacity: 1, duration: 1.0, ease: "expo.out"}}, 0);\n')
            opts["enter"] = None
            opts["close_device"] = True
        elif layout == "lower3rd":
            box = stage_box("full", W, H, False)
        else:
            box = stage_box("window" if layout == "hero3d" else layout, W, H, has_head)
        if layout == "hero3d":
            opts["enter"] = None
        c = clips[0] if clips else None
        if c:
            aspect = box[2] / box[3]
            moves = camera_moves(scene, c["events"], c["len"], c["size"], aspect, c["k"])
            cur = None
            if scene.get("cursor", "auto") and c["events"]:
                cur = cursor_path(c["events"], L)
            # Callouts with a rect (footage pixels) are spotlights with a label.
            srcs = [*scene.get("spotlight", []), *[x for x in scene.get("callouts", []) if "rect" in x]]
            spots = [{"rect": [q * c["k"] for q in sp["rect"]], "at": sp["at"], "label": sp.get("label"),
                      "style": sp.get("style"), "expect": sp.get("expect")} for sp in srcs]
            m, t = clip_layer(sid, c, box, L, moves, cur or None, spots or None, lang=lang, opts=opts)
            parts.append(m)
            tl += t
            if layout == "hero3d":
                tl += (f'tl.fromTo("#{sid}-stage", {{rotationX: 26, rotationY: -24, rotationZ: 3, scale: 0.78, y: 60, transformPerspective: 2200, transformOrigin: "50% 50%"}}, '
                       f'{{rotationX: 0, rotationY: 0, rotationZ: 0, scale: 1, y: 0, duration: 1.5, ease: "expo.out"}}, 0);\n')
            clip_checks(checks, sc, box, c, moves, spots, lang)
        if opts.get("close_device"):
            parts.append("</div>")
        if layout == "lower3rd" and head:
            # The headline as a lower third over full-bleed footage, above the caption band.
            ly = H - (64 + 170 + 170 if not portrait else 300 + 170 + 220)
            parts.append(f'<div id="{sid}-l3" class="l3" data-layout-allow-overlap style="left:0;top:{ly}px">'
                         f'<h2 id="{sid}-h">{words_html(head, lang)}</h2>' + (f'<p id="{sid}-p">{esc(motion.plain(sub))}</p>' if sub else "") + "</div>")
            tl += (f'tl.fromTo("#{sid}-l3", {{clipPath: "inset(0% 100% 0% 0%)"}}, {{clipPath: "inset(0% 0% 0% 0%)", duration: 0.55, ease: "power3.inOut"}}, 0.2);\n'
                   + headline_tweens(sid, style, 0.4)
                   + (f'tl.fromTo("#{sid}-p", {{opacity: 0, y: 10}}, {{opacity: 1, y: 0, duration: 0.4, ease: "expo.out"}}, 0.75);\n' if sub else "")
                   + f'tl.to("#{sid}-l3", {{clipPath: "inset(0% 0% 0% 100%)", duration: 0.4, ease: "power3.in"}}, {max(1.6, L - 0.6):.2f});\n')
            checks.append({"kind": "headline", "scene": n_, "t": at(1.4), "box": [0, ly - 10, W * 0.7, 190], "text": motion.plain(head)})
    elif layout == "mosaic":
        # Up to four clips as tiles that assemble, then the camera pushes into one of them.
        top = 186 if has_head else 90
        n = min(4, len(clips))
        gap = 26
        cols = 2 if n == 4 or portrait else n
        rows = (n + cols - 1) // cols
        aw, ah = W - 120, H - top - 60
        tw = (aw - gap * (cols - 1)) / cols
        th = min((ah - gap * (rows - 1)) / rows, tw * 9 / 16)
        tw = th * 16 / 9
        x0 = (W - (tw * cols + gap * (cols - 1))) / 2
        boxes = [(round(x0 + (k % cols) * (tw + gap)), round(top + 20 + (k // cols) * (th + gap)), round(tw), round(th)) for k in range(n)]
        labels = scene.get("labels") or []
        parts.append(f'<div id="{sid}-mos" class="mos" style="position:absolute;inset:0">')
        for k, (c, box) in enumerate(zip(clips, boxes)):
            pre = f"{sid}m{k}"
            moves = camera_moves(c["scene"], c["events"], c["len"], c["size"], box[2] / box[3], c["k"])
            m, t = clip_layer(pre, c, box, L, moves, None, None, "stage", lang)
            parts.append(m)
            tl += t + (f'tl.fromTo("#{pre}-stage", {{opacity: 0, scale: 0.7, rotation: {(-1) ** k * 4}}}, '
                       f'{{opacity: 1, scale: 1, rotation: 0, duration: 0.7, ease: "expo.out"}}, {0.05 + k * 0.12:.2f});\n')
            if k < len(labels):
                lab = v.text_of(labels[k], lang)
                parts.append(f'<div id="{pre}-lab" class="label" data-layout-allow-overlap style="left:{box[0] + 16}px;top:{box[1] + 14}px">{esc(lab)}</div>')
                tl += f'tl.fromTo("#{pre}-lab", {{opacity: 0, x: -14}}, {{opacity: 1, x: 0, duration: 0.35, ease: "expo.out"}}, {0.5 + k * 0.12:.2f});\n'
                checks.append({"kind": "label", "scene": n_, "t": at(1.3), "box": [box[0], box[1], min(box[2], 600), 80], "text": lab})
        parts.append("</div>")
        focus = scene.get("focus")
        if focus is not None and focus < n:
            fx, fy, fw, fh = boxes[focus]
            s = min((W - 120) / fw, (H - top - 60) / fh)
            at_ = float(scene.get("focus_at", 2.2))
            tl += (f'tl.to("#{sid}-mos", {{x: {W / 2 - (fx + fw / 2) * s:.0f}, y: {(top + (H - top) / 2) - (fy + fh / 2) * s:.0f}, scale: {s:.4f}, '
                   f'transformOrigin: "0px 0px", duration: 0.9, ease: "power3.inOut"}}, {at_:.2f});\n')
    elif layout == "bento":
        # A grid: the footage in a big tile, the headline, a number and a line in tiles beside it.
        gap = 26
        top = 70
        aw, ah = W - 140, H - top - 230
        lw = round(aw * 0.64)
        rw = aw - lw - gap
        box = (70, top, lw, round(lw * 9 / 16))
        c = clips[0] if clips else None
        if c:
            moves = camera_moves(scene, c["events"], c["len"], c["size"], box[2] / box[3], c["k"])
            cur = cursor_path(c["events"], L) if scene.get("cursor", "auto") and c["events"] else None
            m, t = clip_layer(sid, c, box, L, moves, cur or None, None, lang=lang, opts={**opts, "enter": "scale"})
            parts.append(m)
            tl += t
            clip_checks(checks, sc, box, c, moves, None, lang)
        rx = 70 + lw + gap
        h1 = round(box[3] * 0.55)
        h2 = box[3] - h1 - gap
        tiles = [(f"{sid}-tA", rx, top, rw, h1), (f"{sid}-tB", rx, top + h1 + gap, rw, h2)]
        value = str(scene.get("value", ""))
        label = v.text_of(scene.get("label"), lang) or ""
        parts.append(f'<div id="{sid}-tA" class="tile" style="left:{rx}px;top:{top}px;width:{rw}px;height:{h1}px">'
                     f'<div id="{sid}-h" class="t-big">{words_html(head, lang)}</div>'
                     + (f'<div id="{sid}-p" class="t-lab">{esc(motion.plain(sub))}</div>' if sub else "") + "</div>")
        parts.append(f'<div id="{sid}-tB" class="tile" style="left:{rx}px;top:{top + h1 + gap}px;width:{rw}px;height:{h2}px">'
                     f'<div id="{sid}-num" class="t-num">{esc(value)}</div><div id="{sid}-lab" class="t-lab">{esc(label)}</div></div>')
        for k, (tid, *_r) in enumerate(tiles):
            tl += f'tl.fromTo("#{tid}", {{opacity: 0, y: 50, scale: 0.94}}, {{opacity: 1, y: 0, scale: 1, duration: 0.7, ease: "expo.out"}}, {0.12 + k * 0.12:.2f});\n'
        tl += headline_tweens(sid, style, 0.3)
        if head:
            checks.append({"kind": "headline", "scene": n_, "t": at(1.6), "box": [rx, top, rw, h1], "text": motion.plain(head)})
        if value:
            checks.append({"kind": "stat", "scene": n_, "t": at(1.8), "box": [rx, top + h1 + gap, rw, h2], "text": value})
        if label:
            checks.append({"kind": "stat-label", "scene": n_, "t": at(1.8), "box": [rx, top + h1 + gap, rw, h2], "text": label})
    elif layout == "kinetic":
        # Kinetic type: the words slam in one at a time across the frame, then the whole line
        # locks in the middle. A clip, if any, plays dimmed behind.
        words = [w for w in (motion.plain(head) or "").split()] if lang not in CJK else re.findall(r"\S{1,4}", motion.plain(head) or "")
        if clips:
            c = clips[0]
            box = stage_box("full", W, H, False)
            moves = camera_moves(scene, c["events"], c["len"], c["size"], box[2] / box[3], c["k"])
            m, t = clip_layer(sid, c, box, L, moves, None, None, lang=lang)
            parts.append(m + '<div class="kin-dim"></div>')
            tl += t
        spots_ = [(0.5, 0.42, 1.0), (0.32, 0.3, 0.8), (0.66, 0.6, 0.9), (0.4, 0.66, 1.2), (0.6, 0.36, 0.85), (0.5, 0.5, 1.1)]
        beat = max(0.28, min(0.5, (L - 1.6) / max(1, len(words))))
        base = 200 if not portrait else 150
        kin = []
        for k, w in enumerate(words):
            px, py, sc_ = spots_[k % len(spots_)]
            kin.append(f'<div id="{sid}-k{k}" class="kin-word" style="left:{px * W:.0f}px;top:{py * H:.0f}px;font-size:{base * sc_:.0f}px">{esc(w)}</div>')
            t0 = 0.05 + k * beat
            tl += (f'tl.fromTo("#{sid}-k{k}", {{xPercent: -50, yPercent: -50, opacity: 0, scale: 1.8, filter: "blur(10px)"}}, '
                   f'{{xPercent: -50, yPercent: -50, opacity: 1, scale: 1, filter: "blur(0px)", duration: 0.22, ease: "power4.in"}}, {t0:.2f});\n'
                   f'tl.to("#{sid}-k{k}", {{opacity: 0, scale: 0.9, filter: "blur(8px)", duration: 0.18, ease: "power2.in"}}, {t0 + beat * 0.95:.2f});\n')
        lock = 0.05 + len(words) * beat
        parts.append(f'<div class="kin">{"".join(kin)}</div><div id="{sid}-h" class="kin-final" style="font-size:{(110 if not portrait else 96)}px" data-hold>{words_html(head, lang)}</div>')
        tl += (f'tl.fromTo("#{sid}-h", {{opacity: 0, scale: 0.9}}, {{opacity: 1, scale: 1, duration: 0.5, ease: "expo.out"}}, {lock:.2f});\n'
               f'tl.fromTo("#{sid}-h .em", {{"--mark": 0}}, {{"--mark": 1, duration: 0.45, ease: "power2.inOut"}}, {lock + 0.3:.2f});\n')
        checks.append({"kind": "title", "scene": n_, "t": at(max(lock + 0.8, L - 0.4)), "box": text_area, "text": motion.plain(head)})
    elif layout in ("split", "pip"):
        top = 186 if has_head else 90
        if layout == "split" and not portrait:
            gap = 36
            w = (W - 120 - gap) // 2
            h = round(w * 9 / 16)
            boxes = [(60, top + 40, w, h), (60 + w + gap, top + 40, w, h)]
        elif layout == "split":
            h = (H - top - 380) // 2
            boxes = [(48, top, W - 96, h), (48, top + h + 30, W - 96, h)]
        else:
            main = stage_box("window", W, H, has_head)
            pw = round(main[2] * 0.36)
            boxes = [main, (main[0] + main[2] - pw - 28, main[1] + main[3] - round(pw * 9 / 16) - 28, pw, round(pw * 9 / 16))]
        labels = scene.get("labels") or []
        for n, (c, box) in enumerate(zip(clips, boxes)):
            pre = f"{sid}c{n}"
            moves = camera_moves(c["scene"], c["events"], c["len"], c["size"], box[2] / box[3], c["k"])
            m, t = clip_layer(pre, c, box, L, moves, None, None, "stage pip" if layout == "pip" and n == 1 else "stage", lang)
            parts.append(m)
            tl += t
            clip_checks(checks, sc, box, c, moves, None, lang)
            if n < len(labels):
                lab = v.text_of(labels[n], lang)
                parts.append(f'<div id="{pre}-lab" class="label" data-layout-allow-overlap style="left:{box[0] + 18}px;top:{box[1] + 16}px">{esc(lab)}</div>')
                checks.append({"kind": "label", "scene": n_, "t": at(1.4), "box": [box[0], box[1], min(box[2], 700), 90], "text": lab})
                tl += f'tl.fromTo("#{pre}-lab", {{opacity: 0, x: -14}}, {{opacity: 1, x: 0, duration: 0.35, ease: "expo.out"}}, {0.3 + n * 0.12:.2f});\n'
            tl += (f'tl.fromTo("#{pre}-stage", {{opacity: 0.4, {"x: 60" if layout == "split" and n else "y: 30"}}}, '
                   f'{{opacity: 1, {"x: 0" if layout == "split" and n else "y: 0"}, duration: 0.5, ease: "expo.out"}}, {n * 0.1:.2f});\n')
    elif layout == "title":
        big = v.text_of(scene.get("headline"), lang) or ""
        size = 92 if not portrait else 84
        parts.append(f'<div id="{sid}-title" class="title-card"><h1 id="{sid}-h" data-hold style="font-size:{size}px">{words_html(big, lang)}</h1>'
                     + (f'<p id="{sid}-p">{esc(motion.plain(sub))}</p>' if sub else "") + "</div>")
        tl += headline_tweens(sid, style if style != "words-ink" else "words-blur", 0.02)
        if sub:
            # A staged reveal: the second line lands while the first is being read.
            tl += f'tl.fromTo("#{sid}-p", {{opacity: 0, filter: "blur(8px)"}}, {{opacity: 1, filter: "blur(0px)", duration: 0.22, ease: "power4.out"}}, {max(0.7, min(L * 0.4, 2.2)):.2f});\n'
        checks.append({"kind": "title", "scene": n_, "t": at(L - 0.5), "box": text_area, "text": motion.plain(big)})
        if sub:
            checks.append({"kind": "subline", "scene": n_, "t": at(L - 0.5), "box": text_area, "text": motion.plain(sub)})
    elif layout == "stat":
        value = str(scene.get("value", ""))
        m = re.fullmatch(r"([^\d]*)([\d.,]+)(.*)", value)
        label = v.text_of(scene.get("label") or scene.get("headline"), lang)
        parts.append(f'<div id="{sid}-stat" class="stat-card"><div id="{sid}-num" class="num" data-hold>{esc(value)}</div>'
                     f'<p id="{sid}-lab">{esc(label)}</p>' + (f'<p id="{sid}-p" class="sub">{esc(motion.plain(sub))}</p>' if sub else "") + "</div>")
        if m:
            pre, num, post = m.groups()
            digits = len(num.split(".")[1]) if "." in num else 0
            target = float(num.replace(",", ""))
            tl += (f'(() => {{ const o = {{n: 0}}; const el = document.querySelector("#{sid}-num");'
                   f' tl.to(o, {{n: {target}, duration: 1.1, ease: "expo.out", onUpdate: () => {{ el.textContent = {js(pre)} + '
                   f'o.n.toLocaleString("en-US", {{minimumFractionDigits: {digits}, maximumFractionDigits: {digits}}}) + {js(post)}; }}}}, 0.05); }})();\n')
        tl += f'tl.fromTo("#{sid}-num", {{opacity: 0.2, scale: 1.25, filter: "blur(10px)"}}, {{opacity: 1, scale: 1, filter: "blur(0px)", duration: 0.6, ease: "expo.out"}}, 0);\n'
        tl += f'tl.fromTo("#{sid}-lab", {{opacity: 0, filter: "blur(8px)"}}, {{opacity: 1, filter: "blur(0px)", duration: 0.25, ease: "power4.out"}}, 0.7);\n'
        if sub:
            tl += f'tl.fromTo("#{sid}-p", {{opacity: 0}}, {{opacity: 1, duration: 0.3}}, 1.1);\n'
        checks.append({"kind": "stat", "scene": n_, "t": at(max(1.6, L - 0.5)), "box": text_area, "text": value})
        checks.append({"kind": "stat-label", "scene": n_, "t": at(max(1.6, L - 0.5)), "box": text_area, "text": label})
    elif layout == "steps":
        items = [v.text_of(s, lang) for s in scene.get("items", [])]
        beats = scene.get("beats") or [0.05 + n * max(0.45, (L - 1.2) / max(1, len(items))) for n in range(len(items))]
        rows = "".join(f'<li id="{sid}-i{n}"><span class="tick">✓</span><span>{esc(item)}</span></li>' for n, item in enumerate(items))
        parts.append(f'<div id="{sid}-steps" class="steps"><ul>{rows}</ul></div>')
        for n, b in enumerate(beats[:len(items)]):
            tl += (f'tl.fromTo("#{sid}-i{n}", {{opacity: 0, y: 40}}, {{opacity: 1, y: 0, duration: 0.45, ease: "expo.out"}}, {float(b):.2f});\n'
                   f'tl.fromTo("#{sid}-i{n} .tick", {{scale: 0}}, {{scale: 1, duration: 0.35, ease: "back.out(1.6)"}}, {float(b) + 0.12:.2f});\n')
        for item in items:
            checks.append({"kind": "step", "scene": n_, "t": at(L - 0.4), "box": text_area, "text": item})
    elif layout == "quote":
        quote = v.text_of(scene.get("quote"), lang) or ""
        who = v.text_of(scene.get("by"), lang) or ""
        # data-hold: a quote is held still on purpose while it is read (HyperFrames' dead_stretch check).
        parts.append(f'<div id="{sid}-quote" class="quote"><blockquote id="{sid}-h" data-hold>{words_html(quote, lang)}</blockquote>'
                     + (f'<cite id="{sid}-by">{esc(who)}</cite>' if who else "") + "</div>")
        tl += f'tl.fromTo("#{sid}-h .w", {{opacity: 0.15, filter: "blur(5px)"}}, {{opacity: 1, filter: "blur(0px)", duration: 0.16, stagger: 0.045, ease: "power2.out"}}, 0.02);\n'
        if who:
            tl += f'tl.fromTo("#{sid}-by", {{opacity: 0, x: -20}}, {{opacity: 1, x: 0, duration: 0.4, ease: "expo.out"}}, 0.7);\n'
        checks.append({"kind": "quote", "scene": n_, "t": at(L - 0.5), "box": text_area, "text": motion.plain(quote)})
        if who:
            checks.append({"kind": "quote-by", "scene": n_, "t": at(L - 0.5), "box": text_area, "text": who})

    for n, call in enumerate(x for x in scene.get("callouts", []) if "rect" not in x):
        # A label pill pinned to a point of the stage (stage-relative fractions or canvas pixels).
        lab = v.text_of(call.get("label"), lang) or ""
        px, py = call.get("at_xy", [W * 0.5, H * 0.75])
        a0, a1 = call["at"]
        parts.append(f'<div id="{sid}-k{n}" class="pill" data-layout-allow-overlap style="left:{px}px;top:{py}px">{esc(lab)}</div>')
        checks.append({"kind": "callout", "scene": n_, "t": round(sc["start"] + (a0 + a1) / 2, 3), "near": [px - 300, py - 10, 600, 80], "text": lab})
        tl += (f'tl.fromTo("#{sid}-k{n}", {{opacity: 0, y: 14, filter: "blur(6px)"}}, {{opacity: 1, y: 0, filter: "blur(0px)", duration: 0.3, ease: "expo.out"}}, {a0:.2f});\n'
               f'tl.to("#{sid}-k{n}", {{opacity: 0, filter: "blur(6px)", duration: 0.18, ease: "power2.in"}}, {max(a0 + 0.35, a1 - 0.18):.2f});\n')

    return (f'<template>\n<div id="root" data-composition-id="{sid}" data-width="{W}" data-height="{H}" data-layout-allow-overflow>\n'
            + "\n".join(parts) +
            f'\n</div>\n<script>\nconst tl = gsap.timeline({{ paused: true }});\n{tl}window.__timelines["{sid}"] = tl;\n</script>\n</template>\n')


def brand_html(kind, plan_, lang, title, tagline):
    """The opening (logo, Agentty, the feature's title and tagline) or the end card."""
    W, H = plan_["W"], plan_["H"]
    cid = kind
    portrait = W < H
    if kind == "intro":
        body = (f'<div class="brand-col">'
                f'<img id="intro-logo" class="logo" src="assets/logo.png" alt="">'
                f'<div id="intro-name" class="name">{"".join(f"<span class=l>{c}</span>" for c in "Agentty")}</div>'
                f'<div id="intro-rule" class="rule"></div>'
                f'<h1 id="intro-h" class="feat" style="font-size:{76 if portrait else 96}px">{words_html(title, lang)}</h1>'
                + (f'<p id="intro-p" class="tag">{esc(tagline)}</p>' if tagline else "") + "</div>")
        # The logo is already there, soft, on frame 0: the first frame is the thumbnail on social players.
        tl = ('tl.fromTo("#intro-logo", {opacity: 0.35, scale: 0.6, rotation: -8, filter: "blur(12px)"}, {opacity: 1, scale: 1, rotation: 0, filter: "blur(0px)", duration: 0.9, ease: "expo.out"}, 0);\n'
              'tl.fromTo("#intro-name .l", {opacity: 0, filter: "blur(4px)"}, {opacity: 1, filter: "blur(0px)", duration: 0.16, stagger: 0.05, ease: "power2.out"}, 0.55);\n'
              'tl.fromTo("#intro-rule", {scaleX: 0}, {scaleX: 1, duration: 0.5, ease: "expo.out"}, 0.85);\n'
              + headline_tweens("intro", "words-blur", 1.0) +
              ('tl.fromTo("#intro-p", {opacity: 0, filter: "blur(8px)"}, {opacity: 1, filter: "blur(0px)", duration: 0.25, ease: "power4.out"}, 1.4);\n' if tagline else ""))
    else:
        body = (f'<div class="brand-col">'
                f'<img id="outro-logo" class="logo small" src="assets/logo.png" alt="">'
                f'<div id="outro-name" class="name">Agentty</div>'
                f'<div id="outro-site" class="site">{esc(v.SITE)}</div>'
                f'<p id="outro-line" class="tag">{esc(v.OUTRO_LINE[lang])}</p>'
                f'<p id="outro-repo" class="repo">{esc(v.REPO_URL)}</p></div>'
                f'<div id="outro-black" class="black"></div>')
        tl = ('tl.fromTo("#outro-logo", {opacity: 0, scale: 1.35, filter: "blur(14px)"}, {opacity: 1, scale: 1, filter: "blur(0px)", duration: 0.7, ease: "expo.out"}, 0.05);\n'
              'tl.fromTo("#outro-name", {opacity: 0, filter: "blur(8px)"}, {opacity: 1, filter: "blur(0px)", duration: 0.25, ease: "power4.out"}, 0.3);\n'
              'tl.fromTo("#outro-site", {opacity: 0, y: 26}, {opacity: 1, y: 0, duration: 0.55, ease: "expo.out"}, 0.6);\n'
              'tl.fromTo("#outro-line", {opacity: 0}, {opacity: 1, duration: 0.4}, 0.95);\n'
              'tl.fromTo("#outro-repo", {opacity: 0}, {opacity: 1, duration: 0.4}, 1.2);\n'
              f'tl.fromTo("#outro-black", {{opacity: 0}}, {{opacity: 1, duration: 0.6, ease: "power1.in"}}, {OUTRO - 0.65});\n')
    return (f'<template>\n<div id="root" data-composition-id="{cid}" data-width="{W}" data-height="{H}">\n{body}\n</div>\n'
            f'<script>\nconst tl = gsap.timeline({{ paused: true }});\n{tl}window.__timelines["{cid}"] = tl;\n</script>\n</template>\n')


SEAM_LEN = {"curve-left": 0.3, "curve-right": 0.3, "curve-up": 0.3, "zoom-through": 0.2, "inverse-zoom": 0.2,
            "blur-swap": 0.18, "dip": 0.1, "whip": 0.22, "iris": 0.0, "cut": 0.0}


def seam_tweens(seam, a, b, cut, W, H, origin=None):
    """Main-timeline tweens on the two scene hosts across the cut at `cut` seconds."""
    A, B = f'"#{a}"', f'"#{b}"'
    if seam in ("curve-left", "curve-right", "curve-up"):
        axis, d = ("y", -230) if seam == "curve-up" else ("x", -230 if seam == "curve-left" else 230)
        return (f'tl.to({A}, {{{axis}: {d}, filter: "blur(9px)", duration: 0.3, ease: "power4.in"}}, {cut - 0.3:.3f});\n'
                f'tl.to({A}, {{opacity: 0, duration: 0.24, ease: "none"}}, {cut - 0.24:.3f});\n'
                f'tl.fromTo({B}, {{{axis}: {-d}, opacity: 0.35, filter: "blur(9px)"}}, {{{axis}: 0, opacity: 1, filter: "blur(0px)", duration: 0.42, ease: "power4.out"}}, {cut:.3f});\n')
    if seam == "zoom-through":
        return (f'tl.to({A}, {{scale: 1.2, filter: "blur(18px)", duration: 0.2, ease: "power3.in"}}, {cut - 0.2:.3f});\n'
                f'tl.to({A}, {{opacity: 0, duration: 0.2, ease: "none"}}, {cut - 0.2:.3f});\n'
                f'tl.fromTo({B}, {{scale: 0.75, opacity: 0.15, filter: "blur(18px)"}}, {{scale: 1, opacity: 1, filter: "blur(0px)", duration: 0.5, ease: "expo.out"}}, {cut:.3f});\n')
    if seam == "inverse-zoom":
        return (f'tl.to({A}, {{scale: 0.8, filter: "blur(18px)", duration: 0.2, ease: "power3.in"}}, {cut - 0.2:.3f});\n'
                f'tl.to({A}, {{opacity: 0, duration: 0.2, ease: "none"}}, {cut - 0.2:.3f});\n'
                f'tl.fromTo({B}, {{scale: 1.25, opacity: 0.15, filter: "blur(18px)"}}, {{scale: 1, opacity: 1, filter: "blur(0px)", duration: 0.55, ease: "expo.out"}}, {cut:.3f});\n')
    if seam == "blur-swap":
        return (f'tl.to({A}, {{opacity: 0, filter: "blur(8px)", duration: 0.18, ease: "power2.in"}}, {cut - 0.18:.3f});\n'
                f'tl.fromTo({B}, {{opacity: 0, filter: "blur(8px)"}}, {{opacity: 1, filter: "blur(0px)", duration: 0.2, ease: "power2.out"}}, {cut:.3f});\n')
    if seam == "dip":
        return (f'tl.to({A}, {{filter: "brightness(0.6)", duration: 0.1, ease: "none"}}, {cut - 0.1:.3f});\n'
                f'tl.set({A}, {{opacity: 0}}, {cut:.3f});\n'
                f'tl.fromTo({B}, {{opacity: 1, scale: 1.03}}, {{scale: 1, duration: 0.35, ease: "power3.out"}}, {cut:.3f});\n')
    if seam == "whip":
        return (f'tl.to({A}, {{x: -{W * 0.35:.0f}, filter: "blur(22px)", duration: 0.22, ease: "power3.in"}}, {cut - 0.22:.3f});\n'
                f'tl.set({A}, {{opacity: 0}}, {cut:.3f});\n'
                f'tl.fromTo({B}, {{x: {W * 0.35:.0f}, filter: "blur(22px)"}}, {{x: 0, filter: "blur(0px)", duration: 0.3, ease: "power3.out"}}, {cut:.3f});\n')
    if seam == "iris":
        ox, oy = origin or (W / 2, H / 2)
        return (f'tl.set({A}, {{opacity: 1}}, {cut:.3f});\n'
                f'tl.fromTo({B}, {{clipPath: "circle(0px at {ox:.0f}px {oy:.0f}px)"}}, {{clipPath: "circle({math.hypot(W, H):.0f}px at {ox:.0f}px {oy:.0f}px)", duration: 0.5, ease: "power2.in"}}, {cut:.3f});\n'
                f'tl.set({A}, {{opacity: 0}}, {cut + 0.5:.3f});\n')
    return f'tl.set({A}, {{opacity: 0}}, {cut:.3f});\n'


STYLE = """
* { margin: 0; padding: 0; box-sizing: border-box; }
html, body { width: __W__px; height: __H__px; overflow: hidden; background: var(--ground); }
#root { position: relative; width: 100%; height: 100%; background: var(--ground); font-family: "Display", sans-serif; color: var(--ink); }
.bar { position: absolute; left: 118px; top: 52px; width: 8px; height: 100px; background: var(--accent); border-radius: 4px; }
.head { position: absolute; left: 146px; top: 40px; right: 120px; }
.head h2 { font-weight: 600; letter-spacing: -0.02em; line-height: 1.12; }
.head p { margin-top: 8px; font-family: "Body", sans-serif; font-size: 30px; color: var(--muted); }
.l { display: inline-block; }
h1, h2, .feat, .tag, blockquote { text-wrap: balance; word-break: keep-all; }
.stage { position: absolute; overflow: hidden; border-radius: 18px; background: #000; }
.stage.pip { border-radius: 14px; box-shadow: 0 18px 50px rgba(0, 0, 0, .6), 0 0 0 2px rgba(255, 255, 255, .18); }
.cam { position: absolute; left: 0; top: 0; transform-origin: 0 0; }
.cam video, .cam img { position: absolute; left: 0; top: 0; width: 100%; height: 100%; }
.cur { position: absolute; left: 0; top: 0; z-index: 5; filter: drop-shadow(0 4px 7px rgba(0, 0, 0, .4)); }
.cur-svg { width: 100%; height: 100%; display: block; }
.spot { position: absolute; opacity: 0; border-radius: 14px; box-shadow: 0 0 0 4000px rgba(5, 7, 12, .58), 0 0 0 3px var(--accent), 0 0 28px 6px rgba(253, 207, 1, .35); }
.label { position: absolute; padding: 8px 18px; border-radius: 999px; background: rgba(10, 13, 20, .82); color: var(--ink); font-size: 30px; border: 1px solid rgba(255, 255, 255, .14); }
.pill { position: absolute; transform: translateX(-50%); padding: 10px 22px; border-radius: 999px; background: var(--accent); color: #0a0d14; font-size: 30px; white-space: nowrap; box-shadow: 0 10px 30px rgba(0, 0, 0, .35); }
.title-card, .stat-card, .quote, .steps { position: absolute; inset: 0; display: flex; flex-direction: column; align-items: center; justify-content: center; text-align: center; padding: 0 140px; }
.title-card h1 { font-weight: 600; letter-spacing: -0.03em; line-height: 1.1; }
.title-card p, .stat-card .sub { margin-top: 22px; font-family: "Body", sans-serif; font-size: 36px; color: var(--muted); }
.stat-card .num { font-size: 220px; font-weight: 600; letter-spacing: -0.04em; color: var(--accent-ink, var(--accent)); line-height: 1; }
.stat-card p { margin-top: 18px; font-size: 46px; }
.quote blockquote { font-size: 58px; line-height: 1.3; font-weight: 600; max-width: 1400px; }
.quote blockquote::before { content: "“"; color: var(--accent-ink, var(--accent)); }
.quote blockquote::after { content: "”"; color: var(--accent-ink, var(--accent)); }
.quote cite { margin-top: 28px; font-style: normal; font-family: "Body", sans-serif; font-size: 32px; color: var(--muted); }
.steps ul { list-style: none; display: flex; flex-direction: column; gap: 26px; align-items: flex-start; }
.steps li { display: flex; gap: 22px; align-items: center; font-size: 48px; }
.tick { display: inline-flex; align-items: center; justify-content: center; width: 58px; height: 58px; border-radius: 50%; background: var(--accent); color: #0a0d14; font-size: 34px; }
.brand-col { position: absolute; inset: 0; display: flex; flex-direction: column; align-items: center; justify-content: center; }
.logo { width: 220px; height: 220px; }
.logo.small { width: 170px; height: 170px; }
.name { margin-top: 18px; font-size: 76px; font-weight: 600; letter-spacing: -0.01em; }
.rule { margin: 34px 0 30px; width: 120px; height: 6px; border-radius: 3px; background: var(--accent); }
.feat { font-weight: 600; letter-spacing: -0.02em; text-align: center; padding: 0 80px; }
.tag { margin-top: 22px; font-family: "Body", sans-serif; font-size: 38px; color: var(--muted); text-align: center; padding: 0 80px; }
.site { margin-top: 40px; font-size: 64px; font-weight: 600; color: var(--accent-ink, var(--accent)); }
.repo { margin-top: 14px; font-family: "Body", sans-serif; font-size: 30px; color: var(--muted); }
.black { position: absolute; inset: 0; background: #000; opacity: 0; }
#caps { position: absolute; inset: 0; pointer-events: none; }
.cap { position: absolute; left: 0; right: 0; text-align: center; opacity: 0; }
.cap span.box { display: inline-block; max-width: 88%; padding: 12px 28px 14px; border-radius: 16px; background: var(--cap-bg); color: #f5f7fa; font-size: 42px; line-height: 1.32; font-weight: 600; }
.cap .cw { color: rgba(245, 247, 250, .7); }
.cap .cw.on { color: #ffffff; }
"""


def captions_html(plan_, lang):
    """The caption track: one group at a time, the spoken word lit as it is said."""
    W, H = plan_["W"], plan_["H"]
    karaoke = plan_["voice_mode"] == "each" or lang == "en"
    bottom = 300 if W < H else 64
    groups = []
    for sc in plan_["scenes"]:
        for line in sc["lines"]:
            text = v.text_of(line["say"], lang)
            voice = lang if plan_["voice_mode"] == "each" else "en"
            words = line["takes"][voice][2] if karaoke else None
            groups += caption_groups(text, lang, words, sc["start"] + line["start"], line["len"])
    markup, tl = [], ""
    for n, g in enumerate(groups):
        gid = f"cap{n}"
        if g["words"]:
            inner = (" " if lang not in CJK else "").join(f'<span class="cw" id="{gid}w{j}">{esc(w["w"])}</span>' for j, w in enumerate(g["words"]))
        else:
            inner = esc(g["text"])
        markup.append(f'<div id="{gid}" class="cap" style="bottom:{bottom}px"><span class="box">{inner}</span></div>')
        a, b = max(0.0, g["a"]), g["b"]
        if n + 1 < len(groups):
            b = min(b, groups[n + 1]["a"] - 0.02)
        tl += (f'tl.fromTo("#{gid}", {{opacity: 0, y: 10}}, {{opacity: 1, y: 0, duration: 0.14, ease: "power2.out"}}, {a:.3f});\n'
               f'tl.to("#{gid}", {{opacity: 0, duration: 0.1, ease: "power2.in"}}, {max(a + 0.15, b - 0.1):.3f});\n'
               f'tl.set("#{gid}", {{opacity: 0}}, {b:.3f});\n')
        if g["words"]:
            for j, w in enumerate(g["words"]):
                tl += f'tl.set("#{gid}w{j}", {{className: "cw on"}}, {max(a, w["a"]):.3f});\n'
    return groups, (f'<template>\n<div id="root" data-composition-id="caps" data-width="{W}" data-height="{H}" data-layout-allow-caption-zone>\n'
                    + "\n".join(markup) +
                    f'\n</div>\n<script>\nconst tl = gsap.timeline({{ paused: true }});\n{tl}window.__timelines["caps"] = tl;\n</script>\n</template>\n')


def project(plan_, board, lang, root, subtitles=True):
    """Writes one language's HyperFrames project; returns the caption groups (for the .srt)."""
    W, H = plan_["W"], plan_["H"]
    if root.exists():
        shutil.rmtree(root)
    (root / "compositions").mkdir(parents=True)
    (root / "assets").mkdir()
    for f in (plan_["work"] / "shared").iterdir():
        if f.suffix in (".mp4", ".png", ".jpg", ".jpeg"):
            os.link(f, root / "assets" / f.name)
    shutil.copy(v.LOGO, root / "assets" / "logo.png")
    title, tagline = v.text_of(board["title"], lang), v.text_of(board.get("tagline"), lang) or ""
    (root / "compositions" / "intro.html").write_text(brand_html("intro", plan_, lang, title, tagline))
    (root / "compositions" / "outro.html").write_text(brand_html("outro", plan_, lang, title, tagline))
    hosts, tl, audio, fx = [], "", [], []
    hosts.append(f'<div id="intro" data-composition-id="intro" data-composition-src="compositions/intro.html" data-start="0" data-duration="{INTRO + 0.5}" data-track-index="1" data-track-kind="graphics" data-width="{W}" data-height="{H}"></div>')
    scenes = plan_["scenes"]
    # What `verify` reads back from the finished video: every text where and when it should show.
    checks = [{"kind": "intro-title", "scene": 0, "t": INTRO - 0.5, "box": [0, 0, W, H], "text": title}]
    if tagline:
        checks.append({"kind": "intro-tagline", "scene": 0, "t": INTRO - 0.5, "box": [0, 0, W, H], "text": tagline})
    for n, sc in enumerate(scenes):
        (root / "compositions" / f"{sc['id']}.html").write_text(scene_html(sc, plan_, lang, n, checks))
        src = sc["scene"].get("source")
        if src:
            # A stat or a quote names the frame of the recording it comes from: the text must be there.
            footage = plan_["base"] / src["clip"]
            checks.append({"kind": "source", "scene": sc["i"] + 1, "clip": str(footage),
                           "at": resolve_time(src["at"], events_of(footage)), "text": v.text_of(src["text"], lang)})
        track = 2 + n % 2
        hosts.append(f'<div id="{sc["id"]}" data-composition-id="{sc["id"]}" data-composition-src="compositions/{sc["id"]}.html" '
                     f'data-start="{sc["start"]:.3f}" data-duration="{sc["len"] + HOST_TAIL:.3f}" data-track-index="{track}" '
                     f'data-track-kind="graphics" data-width="{W}" data-height="{H}"></div>')
        prev = "intro" if n == 0 else scenes[n - 1]["id"]
        origin = None
        if sc["seam"] == "iris" and n:
            pc = scenes[n - 1]["clips"]
            clicks = [e for e in (pc[0]["events"] if pc else []) if e["kind"] in ("click", "double")]
            if clicks:
                box = stage_box(scenes[n - 1]["layout"], W, H, True)
                cw, ch = pc[0]["size"]
                origin = (box[0] + clicks[-1]["x"] / cw * box[2], box[1] + clicks[-1]["y"] / ch * box[3])
        if sc["seam"] in motion.FX_SEAMS:
            m, j_ = motion.seam_fx(sc["seam"], prev, sc["id"], sc["start"], W, H, n)
            fx.append(m)
            tl += j_
        else:
            tl += seam_tweens(sc["seam"], prev, sc["id"], sc["start"], W, H, origin)
        for j, line in enumerate(sc["lines"]):
            voice = lang if plan_["voice_mode"] == "each" else "en"
            mp3 = line["takes"][voice][0]
            name = f"vo-{sc['id']}-{j}.mp3"
            if not (root / "assets" / name).exists():
                shutil.copy(mp3, root / "assets" / name)
            audio.append(f'<audio id="vo{sc["i"]}x{j}" class="clip" data-start="{sc["start"] + line["start"]:.3f}" '
                         f'data-duration="{line["takes"][voice][1]:.3f}" data-track-index="9" data-volume="1" src="assets/{name}"></audio>')
    last = scenes[-1]["id"] if scenes else "intro"
    hosts.append(f'<div id="outro" data-composition-id="outro" data-composition-src="compositions/outro.html" data-start="{plan_["body_end"]:.3f}" data-duration="{OUTRO:.3f}" data-track-index="4" data-track-kind="graphics" data-width="{W}" data-height="{H}"></div>')
    tl += seam_tweens("blur-swap", last, "outro", plan_["body_end"], W, H)
    checks.append({"kind": "outro-site", "scene": 0, "t": plan_["total"] - 1.5, "box": [0, 0, W, H], "text": v.SITE})
    groups = []
    if subtitles:
        groups, caps = captions_html(plan_, lang)
        band = [0, H - (300 if W < H else 64) - 150, W, 170]
        joiner = "" if lang in CJK else " "
        for g in groups:
            text = joiner.join(w["w"] for w in g["words"]) if g["words"] else g["text"]
            checks.append({"kind": "caption", "a": round(g["a"], 3), "b": round(g["b"], 3), "box": band, "text": text})
        (root / "compositions" / "caps.html").write_text(caps)
        hosts.append(f'<div id="caps" data-composition-id="caps" data-composition-src="compositions/caps.html" data-start="0" data-duration="{plan_["total"]:.3f}" data-track-index="8" data-track-kind="captions" data-width="{W}" data-height="{H}" style="z-index:50"></div>')
    theme = plan_["theme"]
    ground_html, ground_js = motion.ground(theme, W, H, plan_["total"])
    tl = ground_js + tl
    style = (motion.theme_css(theme, hex_rgb(v.ACCENT)) + STYLE.replace("__W__", str(W)).replace("__H__", str(H))
             + motion.GROUND_CSS + motion.TEXT_CSS + motion.ACCENT_CSS + motion.FX_CSS + motion.LAYOUT_CSS)
    page = f"""<!doctype html>
<html lang="{lang}">
<head>
<meta charset="UTF-8" />
<meta name="viewport" content="width={W}, height={H}" />
<script src="{GSAP}"></script>
<style>
{font_faces(lang)}{style}
</style>
</head>
<body>
<div id="root" data-composition-id="main" data-start="0" data-duration="{plan_['total']:.3f}" data-width="{W}" data-height="{H}">
{ground_html}
{chr(10).join(hosts)}
<div id="fx">{"".join(fx)}</div>
{chr(10).join(audio)}
</div>
<script>
const tl = gsap.timeline({{ paused: true }});
{tl}window.__timelines["main"] = tl;
</script>
</body>
</html>
"""
    (root / "index.html").write_text(page)
    (root / "hyperframes.json").write_text(json.dumps({"paths": {"blocks": "compositions", "components": "compositions/components", "assets": "assets"},
                                                       "media": {"autoProxy": True}}, indent=2))
    (root / "checks.json").write_text(json.dumps({"lang": lang, "W": W, "H": H, "total": plan_["total"], "checks": checks}, ensure_ascii=False, indent=1))
    return groups


# ---------------------------------------------------------------- render and checks


def hf(root, *args, check=True):
    env = {**os.environ, "HYPERFRAMES_NO_TELEMETRY": "1", "DO_NOT_TRACK": "1", "HYPERFRAMES_SKIP_SKILLS": "1"}
    result = subprocess.run(["npx", "-y", f"hyperframes@{HF_VERSION}", *args], cwd=root, env=env, capture_output=True, text=True)
    if check and result.returncode != 0:
        sys.exit(f"hyperframes {args[0]} failed in {root}:\n{(result.stdout + result.stderr)[-3000:]}")
    return result


def findings(report):
    def walk(node):
        if isinstance(node, dict):
            if "code" in node and "severity" in node:
                yield node
            for x in node.values():
                yield from walk(x)
        elif isinstance(node, list):
            for x in node:
                yield from walk(x)
    return list(walk(report))


# Layout findings that are the design, not a fault: footage larger than its window (the camera
# frames it), captions laid over the picture.
EXPECTED = {"container_overflow", "content_overlap", "audio_none", "text_occluded", "caption_zone", "composition_file_too_large"}


def gate(root):
    result = hf(root, "check", "--json", check=False)
    raw = result.stdout
    try:
        report = json.loads(raw[raw.find("{"):])
    except ValueError:
        sys.exit(f"hyperframes check printed no report:\n{(result.stdout + result.stderr)[-2000:]}")
    errors = [f for f in findings(report) if f["severity"] == "error"]
    warns = [f for f in findings(report) if f["severity"] == "warning" and f["code"] not in EXPECTED]
    for f in errors + warns:
        print(f"  {f['severity']}: {f['code']} {f.get('selector') or ''} t={f.get('time')} {(f.get('message') or '')[:120]}")
    if errors:
        sys.exit(f"hyperframes check: {len(errors)} error(s) in {root}")


def finish(raw, target, total):
    """Loudness to −16 LUFS (the voice is the only sound), the length exact, fast start."""
    run = ["ffmpeg", "-v", "error", "-y", "-i", str(raw), "-t", f"{total}", "-c:v", "copy"]
    has_audio = "audio" in v.run(["ffprobe", "-v", "error", "-show_entries", "stream=codec_type", "-of", "csv=p=0", str(raw)])
    run += (["-af", "loudnorm=I=-16:TP=-1.5:LRA=11,aresample=48000", "-c:a", "aac", "-b:a", "192k"] if has_audio else [])
    v.run(run + ["-movflags", "+faststart", str(target)])


def srt(groups, path, lang):
    joiner = "" if lang in CJK else " "
    cues = []
    for g in groups:
        text = joiner.join(w["w"] for w in g["words"]) if g["words"] else g["text"]
        cues.append((g["a"], g["b"], text))
    path.write_text("".join(f"{n}\n{v.srt_time(a)} --> {v.srt_time(b)}\n{s}\n\n" for n, (a, b, s) in enumerate(cues, 1)))


def film(args):
    try:
        major = int(subprocess.run(["node", "--version"], capture_output=True, text=True).stdout.strip().lstrip("v").split(".")[0])
    except (OSError, ValueError):
        major = 0
    if major < 22:
        sys.exit("HyperFrames needs Node.js 22 or newer (node --version); `video.sh build` is the fallback without it")
    board_path = Path(args.storyboard).resolve()
    board = json.loads(board_path.read_text())
    base = board_path.parent
    slug = board["slug"]
    langs = args.langs.split(",") if args.langs else v.LANGS
    out = Path(args.out or Path.home() / "Desktop/Agentty-videos" / slug).expanduser()
    out.mkdir(parents=True, exist_ok=True)
    work = Path(args.project or base / "film").resolve()
    work.mkdir(parents=True, exist_ok=True)
    print(f"projects in {work}")
    # The director fills what the storyboard leaves open (title effects, entrances, transitions)
    # with variety from the theme's palette; it writes the choices into the copy kept with the video.
    motion.direct(board)
    p = plan(board, base, langs, work)
    p["work"] = work
    p["base"] = base
    jobs = [(lang, True) for lang in langs] + [(langs[0], False)]
    roots = {}
    for lang, subs in jobs:
        root = work / (lang if subs else "nosub")
        groups = project(p, board, lang, root, subs)
        gate(root)
        roots[(lang, subs)] = (root, groups)
    timeline = {"total": p["total"], "intro": INTRO, "scenes": [{"n": s["i"] + 1, "start": s["start"], "len": s["len"], "layout": s["layout"],
                                                                "seam": s["seam"], "lines": [[s["start"] + l["start"], s["start"] + l["start"] + l["len"]] for l in s["lines"]]}
                                                               for s in p["scenes"]]}
    if args.no_render:
        print("checked; not rendered (--no-render). Preview one: npx hyperframes preview, in its folder")
        return

    def render(job):
        lang, subs = job
        root, groups = roots[job]
        hf(root, "render", "--quality", "delivery", "--output", "raw.mp4")
        target = out / (f"{slug}-{lang}.mp4" if subs else f"{slug}-nosub.mp4")
        finish(root / "raw.mp4", target, p["total"])
        if subs:
            srt(groups, out / f"{slug}.{lang}.srt", lang)
            shutil.copy(root / "checks.json", out / f"{slug}.{lang}.checks.json")
        return target

    with ThreadPoolExecutor(max_workers=int(args.jobs or 2)) as pool:
        for target in pool.map(render, jobs):
            print(f"{target}  {v.duration(target):.1f}s")
    (out / "storyboard.json").write_text(json.dumps(board, ensure_ascii=False, indent=1))
    (out / "timeline.json").write_text(json.dumps(timeline, indent=1))


# ---------------------------------------------------------------- review


def review(args):
    """Sheets to look at: each scene just after it starts, its middle and its end; every seam from
    −0.2 s to +0.2 s; and a narration check (speech heard where the captions say it is)."""
    target = Path(args.video)
    tl = json.loads((target.parent / "timeline.json").read_text())
    shots = [(0.5, "opening"), (tl["intro"] - 0.6, "title held")]
    for s in tl["scenes"]:
        shots += [(s["start"] + 0.6, f"{s['n']} start"), (s["start"] + s["len"] / 2, f"{s['n']} middle"), (s["start"] + s["len"] - 0.15, f"{s['n']} end")]
    seams = []
    for s in tl["scenes"]:
        seams += [(s["start"] + d, f"{s['n']} {s['seam']} {d:+.1f}") for d in (-0.2, -0.07, 0.07, 0.2)]
    shots.append((tl["total"] - 1.6, "end card"))
    for name, rows in (("scenes", shots), ("seams", seams)):
        sheet_at(target, rows, target.with_suffix(f".{name}.png"), cols=3 if name == "scenes" else 4)
        print(target.with_suffix(f".{name}.png"))
    # Narration: where speech is heard against where each line was placed.
    log = subprocess.run(["ffmpeg", "-i", str(target), "-af", "silencedetect=n=-40dB:d=0.25", "-f", "null", "-"], capture_output=True, text=True).stderr
    starts = [float(x) for x in re.findall(r"silence_end: ([\d.]+)", log)]
    worst = 0.0
    for s in tl["scenes"]:
        for a, _ in s["lines"]:
            near = min((abs(x - a) for x in starts), default=9)
            worst = max(worst, near)
            if near > 0.45:
                print(f"  scene {s['n']}: speech near {a:.2f}s not found (nearest {near:.2f}s away)")
    print(f"narration: every line starts within {worst:.2f}s of its caption" if worst <= 0.45 else "narration: see above")


def sheet_at(video, rows, path, cols=3):
    tiles = []
    tmp = Path(v.tempfile.mkdtemp(prefix="review-"))
    for n, (t, label) in enumerate(rows):
        f = tmp / f"{n:03}.png"
        v.run(["ffmpeg", "-v", "error", "-y", "-ss", f"{max(0, t):.3f}", "-i", str(video), "-frames:v", "1", "-vf", "scale=640:-1", str(f)])
        tiles.append((f, f"{t:.2f}s  {label}"))
    tw, th = v.Image.open(tiles[0][0]).size
    rows_n = (len(tiles) + cols - 1) // cols
    img = v.Image.new("RGB", (cols * tw, rows_n * (th + 30)), (0, 0, 0))
    draw = v.ImageDraw.Draw(img)
    for n, (f, label) in enumerate(tiles):
        x, y = n % cols * tw, n // cols * (th + 30)
        img.paste(v.Image.open(f), (x, y + 30))
        draw.text((x + 8, y + 5), label, font=v.font("en", 20), fill=(255, 255, 255))
    img.save(path)
    shutil.rmtree(tmp)
