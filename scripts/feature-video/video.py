#!/usr/bin/env python3
"""Builds an Agentty feature video from a storyboard (see SKILL.md and example.json).

    video.sh build <storyboard.json> [--out DIR] [--langs en,ko,ja,zh]
    video.sh brand [--out DIR] [--langs ...]     render only the opening and the end card
    video.sh sheet <video.mp4> [--every SECONDS]  contact sheet PNG to look at
    video.sh redact <footage.mov>                 finds private text in raw footage, writes what to blur
    video.sh ocr <video.mp4> [--every SECONDS]    reads every frame's text, flags private data left

Every video has the same opening (logo, "Agentty", the feature's title and tagline) and the same
end card (logo, agentty.run, platforms); only the footage, headlines, narration and subtitles
change. Narration comes from Microsoft's neural voices through `edge-tts`: only the narration
text is sent. ffmpeg here has no drawtext / libass, so every text is drawn with Pillow.
"""

import argparse
import asyncio
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

W, H, FPS = 1920, 1080, 30
REPO = Path(__file__).resolve().parents[2]
LOGO = REPO / "docs/assets/logo.png"
CACHE = Path(os.environ.get("AGENTTY_VIDEO_CACHE", Path.home() / ".cache/agentty-video"))
LANGS = ["en", "ko", "ja", "zh"]

# Brand: the same in every video. Change it here, never per video.
BG_TOP, BG_BOTTOM = (10, 13, 20), (17, 22, 34)
ACCENT = (253, 207, 1)  # the logo's yellow
TEXT, MUTED = (245, 247, 250), (156, 163, 175)
SITE, REPO_URL = "agentty.run", "github.com/empty-user77/Agentty"
OUTRO_LINE = {
    "en": "Free and open source · macOS · Windows · Linux",
    "ko": "무료 오픈소스 · macOS · Windows · Linux",
    "ja": "無料のオープンソース · macOS · Windows · Linux",
    "zh": "免费开源 · macOS · Windows · Linux",
}
INTRO_SECONDS, OUTRO_SECONDS, XFADE = 4.2, 4.6, 0.4
VOICES = {"en": "en-US-AndrewNeural", "ko": "ko-KR-InJoonNeural", "ja": "ja-JP-KeitaNeural", "zh": "zh-CN-YunxiNeural"}
# Footage sits in a framed box under the headline band.
BOX = (120, 150, 1680, 880)
RADIUS = 18
LEAD, GAP, TAIL = 0.5, 0.35, 0.7  # narration: before the first line, between lines, after the last

FONTS = {
    # (path, index of the bold face, index of the regular face)
    "en": ("/System/Library/Fonts/Avenir Next.ttc", 0, 5),
    "ko": ("/System/Library/Fonts/AppleSDGothicNeo.ttc", 6, 2),
    "ja": ("/System/Library/Fonts/ヒラギノ角ゴシック W6.ttc", 0, None),
    "zh": ("/System/Library/Fonts/Hiragino Sans GB.ttc", 2, 0),
}
JA_REGULAR = "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc"


def font(lang, size, bold=True):
    path, b, r = FONTS[lang]
    if lang == "ja" and not bold:
        return ImageFont.truetype(JA_REGULAR, size, index=0)
    return ImageFont.truetype(path, size, index=b if bold else r)


def run(cmd, **kw):
    result = subprocess.run(cmd, capture_output=True, text=True, **kw)
    if result.returncode != 0:
        sys.exit(f"command failed: {' '.join(map(str, cmd[:6]))} …\n{result.stderr[-3000:]}")
    return result.stdout


def duration(path):
    return float(run(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", str(path)]).strip())


def ease(p):
    p = max(0.0, min(1.0, p))
    return 1 - (1 - p) ** 3


def text_of(value, lang):
    """A storyboard text: a string (the same in every language) or {lang: text}; English fills gaps."""
    if value is None or isinstance(value, str):
        return value
    return value.get(lang) or value.get("en")


# ---------------------------------------------------------------- drawing


def background():
    img = Image.new("RGB", (W, H))
    draw = ImageDraw.Draw(img)
    for y in range(H):
        t = y / (H - 1)
        draw.line([(0, y), (W, y)], fill=tuple(int(a + (b - a) * t) for a, b in zip(BG_TOP, BG_BOTTOM)))
    glow = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    ImageDraw.Draw(glow).ellipse([-300, -500, 1100, 500], fill=(0, 120, 190, 46))
    glow = glow.filter(ImageFilter.GaussianBlur(160))
    return Image.alpha_composite(img.convert("RGBA"), glow)


def fit(src_w, src_h, box=BOX):
    bx, by, bw, bh = box
    scale = min(bw / src_w, bh / src_h)
    w, h = int(src_w * scale) // 2 * 2, int(src_h * scale) // 2 * 2
    return bx + (bw - w) // 2, by + (bh - h) // 2, w, h


def frame_layers(rect, work):
    """The scene's backdrop (with a shadow under the footage) and the mask that rounds its corners."""
    x, y, w, h = rect
    key = f"{x}-{y}-{w}-{h}"
    under, over = work / f"under-{key}.png", work / f"over-{key}.png"
    if not under.exists():
        bg = background()
        shadow = Image.new("RGBA", (W, H), (0, 0, 0, 0))
        ImageDraw.Draw(shadow).rounded_rectangle([x, y + 14, x + w, y + h + 14], RADIUS, fill=(0, 0, 0, 150))
        bg = Image.alpha_composite(bg, shadow.filter(ImageFilter.GaussianBlur(26)))
        bg.convert("RGB").save(under)
        hole = Image.new("L", (W, H), 255)
        ImageDraw.Draw(hole).rounded_rectangle([x, y, x + w - 1, y + h - 1], RADIUS, fill=0)
        top = bg.copy()
        top.putalpha(hole)
        ImageDraw.Draw(top).rounded_rectangle([x, y, x + w - 1, y + h - 1], RADIUS, outline=(255, 255, 255, 34), width=2)
        top.save(over)
    return under, over


def wrap(text, fnt, width, lang):
    # Japanese and Chinese break between characters; the others between words.
    words, joiner = (list(text), "") if lang in ("ja", "zh") else (text.split(" "), " ")
    lines, line = [], ""
    for word in words:
        trial = (line + joiner + word) if line else word
        if fnt.getlength(trial) <= width or not line:
            line = trial
        else:
            lines.append(line)
            line = word.lstrip()
    if line:
        lines.append(line)
    return lines


def subtitle_png(text, lang, path):
    img = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    if text:
        for size in (44, 40, 36, 32):
            fnt = font(lang, size, bold=lang == "en")
            lines = wrap(text, fnt, 1480, lang)
            if len(lines) <= 2:
                break
        draw = ImageDraw.Draw(img)
        line_h = int(size * 1.38)
        block_w = max(fnt.getlength(line) for line in lines)
        block_h = line_h * len(lines)
        bottom = BOX[1] + BOX[3] - 22
        top = bottom - block_h - 20
        left = (W - block_w) / 2
        draw.rounded_rectangle([left - 26, top, left + block_w + 26, bottom], 14, fill=(0, 0, 0, 168))
        for i, line in enumerate(lines):
            lw = fnt.getlength(line)
            draw.text(((W - lw) / 2, top + 10 + i * line_h), line, font=fnt, fill=TEXT)
    img.save(path)


def headline_png(text, sub, lang, x, path):
    img = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    if text:
        draw = ImageDraw.Draw(img)
        big, small = font(lang, 50), font(lang, 28, bold=False)
        draw.rounded_rectangle([x, 44, x + 8, 44 + (98 if sub else 62)], 4, fill=ACCENT)
        draw.text((x + 28, 40), text, font=big, fill=TEXT)
        if sub:
            draw.text((x + 30, 108), sub, font=small, fill=MUTED)
    img.save(path)


def callout_png(rect, label, lang, path):
    x, y, w, h = rect
    img = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    glow = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    ImageDraw.Draw(glow).rounded_rectangle([x - 8, y - 8, x + w + 8, y + h + 8], 14, outline=ACCENT + (255,), width=10)
    img = Image.alpha_composite(img, glow.filter(ImageFilter.GaussianBlur(8)))
    draw = ImageDraw.Draw(img)
    draw.rounded_rectangle([x - 6, y - 6, x + w + 6, y + h + 6], 12, outline=(255, 232, 140, 255), width=4)
    if label:
        fnt = font(lang, 30)
        lw = fnt.getlength(label)
        lx = min(max(x + w / 2 - lw / 2 - 18, 20), W - lw - 56)
        ly = y + h + 18 if y + h + 80 < H - 120 else y - 70
        draw.rounded_rectangle([lx, ly, lx + lw + 36, ly + 50], 25, fill=ACCENT + (240,))
        draw.text((lx + 18, ly + 6), label, font=fnt, fill=BG_TOP)
    img.save(path)


# ---------------------------------------------------------------- opening and end card


def pipe_frames(path, seconds, draw_frame):
    proc = subprocess.Popen(
        ["ffmpeg", "-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{W}x{H}", "-r", str(FPS), "-i", "-",
         "-c:v", "libx264", "-preset", "medium", "-crf", "16", "-pix_fmt", "yuv420p", str(path)],
        stdin=subprocess.PIPE,
    )
    for i in range(int(round(seconds * FPS))):
        proc.stdin.write(draw_frame(i / FPS).convert("RGB").tobytes())
    proc.stdin.close()
    if proc.wait() != 0:
        sys.exit(f"could not render {path}")


def layer(draw_fn=None):
    img = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    if draw_fn:
        draw_fn(ImageDraw.Draw(img), img)
    return img


def place(base, overlay, alpha, dy=0, scale=1.0, center=None):
    if alpha <= 0:
        return base
    if scale != 1.0 and center:
        cx, cy = center
        resized = overlay.resize((int(W * scale), int(H * scale)), Image.LANCZOS)
        canvas = Image.new("RGBA", (W, H), (0, 0, 0, 0))
        canvas.paste(resized, (int(cx - cx * scale), int(cy - cy * scale)))
        overlay = canvas
    if dy:
        canvas = Image.new("RGBA", (W, H), (0, 0, 0, 0))
        canvas.paste(overlay, (0, int(dy)))
        overlay = canvas
    if alpha < 1:
        a = overlay.getchannel("A").point(lambda v: int(v * alpha))
        overlay = overlay.copy()
        overlay.putalpha(a)
    return Image.alpha_composite(base, overlay)


def centered(text, fnt, y, fill):
    def draw_fn(draw, _):
        draw.text(((W - fnt.getlength(text)) / 2, y), text, font=fnt, fill=fill)
    return layer(draw_fn)


def logo_layer(size, y):
    logo = Image.open(LOGO).convert("RGBA").resize((size, size), Image.LANCZOS)
    img = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    img.paste(logo, ((W - size) // 2, y), logo)
    return img


def render_intro(title, tagline, lang, path):
    bg = background()
    logo = logo_layer(200, 250)
    name = centered("Agentty", font("en", 76), 470, TEXT)
    bar = layer(lambda d, _: d.rounded_rectangle([W / 2 - 60, 612, W / 2 + 60, 618], 3, fill=ACCENT))
    head = centered(title, font(lang, 84), 660, TEXT)
    sub = centered(tagline or "", font(lang, 38, bold=False), 790, MUTED)

    def frame(t):
        img = bg
        img = place(img, logo, ease(t / 0.7), scale=0.82 + 0.18 * ease(t / 0.7), center=(W / 2, 350))
        img = place(img, name, ease((t - 0.35) / 0.6), dy=24 * (1 - ease((t - 0.35) / 0.6)))
        img = place(img, bar, ease((t - 1.1) / 0.4))
        img = place(img, head, ease((t - 1.3) / 0.6), dy=30 * (1 - ease((t - 1.3) / 0.6)))
        if tagline:
            img = place(img, sub, ease((t - 1.75) / 0.6), dy=24 * (1 - ease((t - 1.75) / 0.6)))
        return img

    pipe_frames(path, INTRO_SECONDS, frame)


def render_outro(lang, path):
    bg = background()
    logo = logo_layer(180, 230)
    name = centered("Agentty", font("en", 72), 430, TEXT)
    site = centered(SITE, font("en", 64), 580, ACCENT)
    line = centered(OUTRO_LINE[lang], font(lang, 36, bold=False), 690, MUTED)
    repo = centered(REPO_URL, font("en", 30, bold=False), 760, MUTED)
    black = Image.new("RGBA", (W, H), (0, 0, 0, 255))

    def frame(t):
        img = bg
        img = place(img, logo, ease(t / 0.6), scale=0.85 + 0.15 * ease(t / 0.6), center=(W / 2, 320))
        img = place(img, name, ease((t - 0.25) / 0.6))
        img = place(img, site, ease((t - 0.6) / 0.6), dy=24 * (1 - ease((t - 0.6) / 0.6)))
        img = place(img, line, ease((t - 0.95) / 0.6))
        img = place(img, repo, ease((t - 1.25) / 0.6))
        return place(img, black, ease((t - (OUTRO_SECONDS - 0.6)) / 0.6))

    pipe_frames(path, OUTRO_SECONDS, frame)


def brand(langs, titles, taglines):
    """Opening and end card per language, cached by everything that goes into them."""
    out = {}
    stamp = hashlib.sha1((Path(__file__).read_bytes() + LOGO.read_bytes())).hexdigest()[:10]
    for lang in langs:
        title, tagline = titles[lang], taglines.get(lang) or ""
        key = hashlib.sha1(f"{stamp}|{lang}|{title}|{tagline}".encode()).hexdigest()[:12]
        intro, outro = CACHE / "brand" / f"intro-{key}.mp4", CACHE / "brand" / f"outro-{stamp}-{lang}.mp4"
        intro.parent.mkdir(parents=True, exist_ok=True)
        if not intro.exists():
            render_intro(title, tagline, lang, intro)
        if not outro.exists():
            render_outro(lang, outro)
        out[lang] = (intro, outro)
    return out


# ---------------------------------------------------------------- narration


def tts(text, voice, rate):
    key = hashlib.sha1(f"{voice}|{rate}|{text}".encode()).hexdigest()[:16]
    path = CACHE / "tts" / f"{key}.mp3"
    if not path.exists():
        import edge_tts

        path.parent.mkdir(parents=True, exist_ok=True)
        part = path.with_suffix(".part.mp3")
        asyncio.run(edge_tts.Communicate(text, voice, rate=rate).save(str(part)))
        part.rename(path)
    return path, duration(path)


# ---------------------------------------------------------------- footage


def scene_clip(scene, base, seconds, out, work):
    """One scene's picture: footage (trimmed, sped up, zoomed) in the frame, held on its last frame."""
    src = base / (scene.get("clip") or scene["image"])
    if not src.exists():
        sys.exit(f"missing footage: {src}")
    is_image = "image" in scene
    if is_image:
        sw, sh = Image.open(src).size
    else:
        sw, sh = map(int, run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "csv=p=0", str(src)]).strip().split(",")[:2])
    cx, cy, cw, ch = scene.get("crop") or [0, 0, sw, sh]
    rect = (0, 0, W, H) if scene.get("full") else fit(cw, ch)
    x, y, w, h = rect
    under, over = frame_layers(rect, work)

    start = 0.0 if is_image else float(scene.get("from", 0))
    pre, label = blur_graph(blur_regions(src, scene), start), "[1:v]"
    if pre:
        pre, label = pre + ";", "[src]"
    chain = [f"crop={cw}:{ch}:{cx}:{cy}"]
    if not is_image:
        speed = float(scene.get("speed", 1))
        chain += [f"setpts=(PTS-STARTPTS)/{speed}", f"fps={FPS}"]
    zoom = scene.get("zoom")
    if zoom:
        # Ease from the whole picture into `rect` (same coordinates as `crop`) between `at[0]` and `at[1]`.
        zx, zy, zw, zh = zoom["rect"]
        zx, zy = zx - cx, zy - cy
        aspect = cw / ch
        if zw / zh < aspect:
            zx, zw = zx - (zh * aspect - zw) / 2, zh * aspect
        else:
            zy, zh = zy - (zw / aspect - zh) / 2, zw / aspect
        t0, t1 = zoom.get("at", [0.5, 1.5])
        back = zoom.get("back")
        p = f"(if(lt(it,{t0}),0,if(gt(it,{t1}),1,(it-{t0})/({t1}-{t0}))))"
        p = f"(3*{p}*{p}-2*{p}*{p}*{p})"
        if back:
            b0, b1 = back
            q = f"(if(lt(it,{b0}),0,if(gt(it,{b1}),1,(it-{b0})/({b1}-{b0}))))"
            p = f"({p}*(1-(3*{q}*{q}-2*{q}*{q}*{q})))"
        z = f"{cw}/({cw}+({zw}-{cw})*{p})"
        # zoompan works on a 2x picture so the slow pan does not step from pixel to pixel.
        chain += [
            f"scale={cw * 2}:{ch * 2}",
            f"zoompan=z='{z}':x='max(0,min(iw-iw/zoom,{zx * 2}*{p}))':y='max(0,min(ih-ih/zoom,{zy * 2}*{p}))'"
            f":d=1:s={w}x{h}:fps={FPS}",
        ]
    else:
        chain.append(f"scale={w}:{h}:flags=lanczos")
    chain += [f"tpad=stop_mode=clone:stop_duration={seconds + 1}", "setsar=1", "format=yuv420p"]

    inputs = ["-loop", "1", "-t", f"{seconds}", "-i", str(src)] if is_image else (
        ["-ss", f"{start}"] + (["-t", f"{float(scene['to']) - start}"] if "to" in scene else []) + ["-i", str(src)]
    )
    graph = (
        f"{pre}{label}{','.join(chain)}[f];"
        f"[0:v][f]overlay={x}:{y}:shortest=0[a];"
        f"[a][2:v]overlay=0:0,format=yuv420p,fps={FPS}[v]"
    )
    run(["ffmpeg", "-v", "error", "-y", "-loop", "1", "-framerate", str(FPS), "-t", f"{seconds}", "-i", str(under), *inputs,
         "-loop", "1", "-framerate", str(FPS), "-t", f"{seconds}", "-i", str(over),
         "-filter_complex", graph, "-map", "[v]", "-t", f"{seconds}", "-an",
         "-c:v", "libx264", "-preset", "veryfast", "-crf", "16", "-r", str(FPS), str(out)])
    return rect, (cx, cy, cw, ch)


def blur_regions(src, scene):
    """What to blur in this footage: the `redact` sidecar (<footage>.blur.json) and the scene's own `blur`."""
    regions = []
    sidecar = src.with_name(src.name + ".blur.json")
    if sidecar.exists():
        regions += json.loads(sidecar.read_text())
    for r in scene.get("blur", []):
        regions.append(r if isinstance(r, dict) else {"rect": r})
    return regions


def blur_graph(regions, start):
    """Filters that blur each region (footage pixels, footage seconds) on input 1, ending in [src]."""
    if not regions:
        return ""
    parts, last = [], "[1:v]"
    for k, r in enumerate(regions):
        x, y, w, h = (max(0, int(v)) for v in r["rect"])
        w, h = max(w, 8), max(h, 8)
        when = ""
        if "from" in r or "to" in r:
            a, b = float(r.get("from", 0)) - start, float(r.get("to", 1e9)) - start
            when = f":enable='between(t,{a:.3f},{b:.3f})'"
        nxt = "[src]" if k == len(regions) - 1 else f"[r{k}]"
        parts.append(f"{last}split[k{k}][c{k}];[c{k}]crop={w}:{h}:{x}:{y},gblur=sigma=14:steps=3[g{k}];[k{k}][g{k}]overlay={x}:{y}{when}{nxt}")
        last = nxt
    return ";".join(parts)


def to_screen(r, rect, crop):
    """A rectangle in footage pixels to output pixels."""
    x, y, w, h = rect
    cx, cy, cw, ch = crop
    sx, sy = w / cw, h / ch
    return [int(x + (r[0] - cx) * sx), int(y + (r[1] - cy) * sy), int(r[2] * sx), int(r[3] * sy)]


# ---------------------------------------------------------------- build


def srt_time(t):
    ms = int(round(t * 1000))
    return f"{ms // 3600000:02}:{ms // 60000 % 60:02}:{ms // 1000 % 60:02},{ms % 1000:03}"


def build(args):
    board_path = Path(args.storyboard).resolve()
    board = json.loads(board_path.read_text())
    base = board_path.parent
    slug = board["slug"]
    langs = args.langs.split(",") if args.langs else LANGS
    out = Path(args.out or Path.home() / "Desktop/Agentty-videos" / slug).expanduser()
    out.mkdir(parents=True, exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix=f"video-{slug}-", dir=args.work or None))
    voice_mode = board.get("narration", "en")
    rate = board.get("rate", "+0%")
    voices = {**VOICES, **board.get("voices", {})}
    spoken = langs if voice_mode == "each" else ["en"]
    print(f"work folder: {work}")

    # 1. Narration first: it sets every scene's length.
    timeline, t = [], 0.0
    for i, scene in enumerate(board["scenes"]):
        lines = []
        cursor = LEAD
        for say in scene.get("say", []):
            takes = {lang: tts(text_of(say, lang), voices[lang], rate) for lang in spoken}
            slot = max(d for _, d in takes.values())
            lines.append({"start": cursor, "end": cursor + slot, "say": say, "audio": {k: v[0] for k, v in takes.items()}})
            cursor += slot + GAP
        spoken_len = (cursor - GAP + TAIL) if lines else 0
        if "image" in scene:
            footage = float(scene.get("seconds", 3))
        else:
            src = base / scene["clip"]
            end = float(scene["to"]) if "to" in scene else duration(src)
            footage = (end - float(scene.get("from", 0))) / float(scene.get("speed", 1))
        seconds = round(max(footage, spoken_len, float(scene.get("min", 0))), 3)
        timeline.append({"start": t, "seconds": seconds, "lines": lines})
        print(f"scene {i + 1}: {seconds:.1f}s (footage {footage:.1f}s, narration {spoken_len:.1f}s)")
        t += seconds
    body_seconds = t

    # 2. Shared picture: every scene in its frame, joined.
    clips, geometry = [], []
    for i, (scene, slot) in enumerate(zip(board["scenes"], timeline)):
        clip = work / f"scene{i + 1:02}.mp4"
        geometry.append(scene_clip(scene, base, slot["seconds"], clip, work))
        clips.append(clip)
    (work / "scenes.txt").write_text("".join(f"file '{c}'\n" for c in clips))
    body = work / "body.mp4"
    run(["ffmpeg", "-v", "error", "-y", "-f", "concat", "-safe", "0", "-i", str(work / "scenes.txt"), "-c", "copy", str(body)])

    titles = {lang: text_of(board["title"], lang) for lang in langs}
    taglines = {lang: text_of(board.get("tagline"), lang) for lang in langs}
    branding = brand(langs, titles, taglines)
    offset = INTRO_SECONDS - XFADE  # where the body starts in the finished video

    def overlays(lang, subtitles):
        """Headlines, callouts and subtitles for one language, as concat lists of full-frame PNGs."""
        blank = work / "blank.png"
        Image.new("RGBA", (W, H), (0, 0, 0, 0)).save(blank)
        heads, subs, calls = [], [], []
        for i, (scene, slot, (rect, crop)) in enumerate(zip(board["scenes"], timeline, geometry)):
            png = work / f"head-{lang}-{i}.png"
            headline_png(text_of(scene.get("headline"), lang), text_of(scene.get("subline"), lang), lang, rect[0] if not scene.get("full") else 120, png)
            heads.append((png, slot["seconds"]))
            cursor = 0.0
            for j, line in enumerate(slot["lines"]):
                gap = line["start"] - cursor
                if gap > 0:
                    subs.append((blank, gap))
                cue = work / f"sub-{lang}-{i}-{j}.png"
                subtitle_png(text_of(line["say"], lang) if subtitles else "", lang, cue)
                subs.append((cue, line["end"] - line["start"]))
                cursor = line["end"]
            subs.append((blank, slot["seconds"] - cursor))
            for k, c in enumerate(scene.get("callouts", [])):
                png = work / f"call-{lang}-{i}-{k}.png"
                callout_png(to_screen(c["rect"], rect, crop), text_of(c.get("label"), lang), lang, png)
                calls.append((png, slot["start"] + c["at"][0], slot["start"] + c["at"][1]))
        return heads, subs, calls

    def concat_list(items, name):
        path = work / name
        text = "".join(f"file '{p}'\nduration {max(d, 0.001):.3f}\n" for p, d in items)
        path.write_text(text + f"file '{items[-1][0]}'\n")
        return path

    def render(lang, subtitles, target):
        heads, subs, calls = overlays(lang, subtitles)
        inputs = ["-i", str(body),
                  "-f", "concat", "-safe", "0", "-i", str(concat_list(heads, f"heads-{lang}.txt")),
                  "-f", "concat", "-safe", "0", "-i", str(concat_list(subs, f"subs-{lang}-{int(subtitles)}.txt"))]
        graph = "[1:v]format=rgba,fps=30[h];[2:v]format=rgba,fps=30[s];[0:v][h]overlay=0:0:eof_action=pass[a0];[a0][s]overlay=0:0:eof_action=pass[b0]"
        last = "b0"
        for k, (png, a, b) in enumerate(calls):
            inputs += ["-loop", "1", "-framerate", str(FPS), "-t", f"{body_seconds}", "-i", str(png)]
            n = 3 + k
            graph += (f";[{n}:v]format=rgba,fade=t=in:st={a}:d=0.3:alpha=1,fade=t=out:st={max(b - 0.3, a)}:d=0.3:alpha=1[c{k}]"
                      f";[{last}][c{k}]overlay=0:0:enable='between(t,{a},{b})'[o{k}]")
            last = f"o{k}"
        styled = work / f"body-{lang}-{int(subtitles)}.mp4"
        run(["ffmpeg", "-v", "error", "-y", *inputs, "-filter_complex", graph + f";[{last}]format=yuv420p[v]", "-map", "[v]",
             "-t", f"{body_seconds}", "-c:v", "libx264", "-preset", "veryfast", "-crf", "16", "-r", str(FPS), str(styled)])

        # Narration track: each line at its place, the rest silence, then loudness-normalised.
        voice = lang if voice_mode == "each" else "en"
        audio_in, mix, labels = [], [], []
        for slot in timeline:
            for line in slot["lines"]:
                n = len(audio_in) // 2
                audio_in += ["-i", str(line["audio"][voice])]
                delay = int((offset + slot["start"] + line["start"]) * 1000)
                mix.append(f"[{n + 3}:a]adelay={delay}|{delay},aresample=48000[l{n}]")
                labels.append(f"[l{n}]")
        intro, outro = branding[lang]
        total = INTRO_SECONDS + body_seconds + OUTRO_SECONDS - 2 * XFADE
        v = (f"[0:v]settb=AVTB,fps={FPS},format=yuv420p[i];[1:v]settb=AVTB,fps={FPS},format=yuv420p[m];"
             f"[2:v]settb=AVTB,fps={FPS},format=yuv420p[e];"
             f"[i][m]xfade=transition=fade:duration={XFADE}:offset={INTRO_SECONDS - XFADE}[im];"
             f"[im][e]xfade=transition=fade:duration={XFADE}:offset={INTRO_SECONDS + body_seconds - 2 * XFADE}[v]")
        if labels:
            a = ";".join(mix) + f";{''.join(labels)}amix=inputs={len(labels)}:normalize=0,apad,atrim=0:{total},loudnorm=I=-16:TP=-1.5:LRA=11,aresample=48000[a]"
        else:
            a = f"anullsrc=r=48000:cl=stereo,atrim=0:{total}[a]"
        run(["ffmpeg", "-v", "error", "-y", "-i", str(intro), "-i", str(styled), "-i", str(outro), *audio_in,
             "-filter_complex", v + ";" + a, "-map", "[v]", "-map", "[a]", "-t", f"{total}",
             "-c:v", "libx264", "-preset", "medium", "-crf", "18", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "192k",
             "-movflags", "+faststart", str(target)])
        return total

    for lang in langs:
        target = out / f"{slug}-{lang}.mp4"
        total = render(lang, True, target)
        cues = []
        for slot in timeline:
            for line in slot["lines"]:
                a = offset + slot["start"] + line["start"]
                cues.append((a, a + line["end"] - line["start"], text_of(line["say"], lang)))
        (out / f"{slug}.{lang}.srt").write_text("".join(f"{n}\n{srt_time(a)} --> {srt_time(b)}\n{s}\n\n" for n, (a, b, s) in enumerate(cues, 1)))
        print(f"{target}  {total:.1f}s")
    clean = out / f"{slug}-nosub.mp4"
    render(langs[0], False, clean)
    print(f"{clean}  (no subtitles, for uploading with the .srt files)")
    (out / "storyboard.json").write_text(board_path.read_text())
    if not args.keep:
        shutil.rmtree(work, ignore_errors=True)


# ---------------------------------------------------------------- checks


def sheet(args):
    video = Path(args.video)
    every = float(args.every)
    tmp = Path(tempfile.mkdtemp(prefix="sheet-"))
    run(["ffmpeg", "-v", "error", "-i", str(video), "-vf", f"fps=1/{every},scale=480:-1", str(tmp / "f%03d.png")])
    frames = sorted(tmp.glob("f*.png"))
    cols = 4
    rows = (len(frames) + cols - 1) // cols
    tile = Image.open(frames[0]).size
    img = Image.new("RGB", (cols * tile[0], rows * (tile[1] + 28)), (0, 0, 0))
    draw = ImageDraw.Draw(img)
    for n, f in enumerate(frames):
        x, y = n % cols * tile[0], n // cols * (tile[1] + 28)
        img.paste(Image.open(f), (x, y + 28))
        draw.text((x + 8, y + 4), f"{n * every:.1f}s", font=font("en", 20), fill=(255, 255, 255))
    target = video.with_suffix(".sheet.png")
    img.save(target)
    shutil.rmtree(tmp)
    print(target)


# Words that must not show in a video: paths outside the demo, accounts, addresses, tokens.
PRIVATE = {
    "path": r"(/Users/|/private/|/var/folders|/home/|~/(?!agentty-demo))",
    "email": r"[\w.+-]+@[\w-]+\.[\w.]+",
    "token": r"(ghp_|gho_|github_pat_|sk-|xox[bp]-|figd_|AKIA|eyJ[\w-]{10,})",
    "ip": r"\b(?!127\.0\.0\.1\b)\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b",  # loopback is harmless
    "tailnet": r"\.ts\.net",
}


def private_patterns(extra=()):
    """PRIVATE plus this Mac's user, full name and host names, and any patterns the caller adds."""
    found = dict(PRIVATE)
    names = [os.environ.get("USER", "")]
    for cmd in (["id", "-F"], ["scutil", "--get", "ComputerName"], ["scutil", "--get", "LocalHostName"]):
        try:
            names.append(subprocess.run(cmd, capture_output=True, text=True).stdout.strip())
        except OSError:
            pass
    words = {w for n in names for w in re.split(r"[\s'’]+", n) if len(w) > 2}
    if words:
        found["account"] = r"(?i)(" + "|".join(re.escape(w) for w in sorted(words)) + r")"
    for k, p in enumerate(extra):
        found[f"extra{k + 1}"] = p
    return found


def read_words(video, every):
    """(seconds, x, y, w, h, word, line) for every word Vision reads, one frame per `every` seconds."""
    tmp = Path(tempfile.mkdtemp(prefix="ocr-"))
    run(["ffmpeg", "-v", "error", "-i", str(video), "-vf", f"fps=1/{every}", str(tmp / "f%05d.png")])
    out = run(["swift", str(Path(__file__).with_name("ocr.swift")), str(tmp)])
    shutil.rmtree(tmp)
    words = []
    for row in out.splitlines():
        cols = row.split("\t")
        if len(cols) < 7:
            continue
        n = int(re.sub(r"\D", "", cols[0]) or 1)
        words.append(((n - 1) * every, *map(int, cols[1:5]), cols[5], cols[6]))
    return words


def private_hits(words, patterns):
    for t, x, y, w, h, word, line in words:
        for name, p in patterns.items():
            if re.search(p, word):
                yield name, t, (x, y, w, h), word, line
                break


def redact(args):
    """Finds private words in raw footage and writes <footage>.blur.json; `build` blurs them."""
    every, pad = float(args.every), int(args.pad)
    patterns = private_patterns(args.also or [])
    regions = []
    for name, t, (x, y, w, h), _, _ in private_hits(read_words(args.footage, every), patterns):
        box = [x - pad, y - pad, w + 2 * pad, h + 2 * pad]
        for r in regions:
            rx, ry, rw, rh = r["rect"]
            # The same spot in the next frames: one region that lasts longer.
            if abs(rx - box[0]) < 24 and abs(ry - box[1]) < 16 and t - r["to"] <= every * 1.5:
                r["rect"] = [min(rx, box[0]), min(ry, box[1]), max(rx + rw, box[0] + box[2]) - min(rx, box[0]), max(ry + rh, box[1] + box[3]) - min(ry, box[1])]
                r["to"] = t + every
                break
        else:
            regions.append({"rect": box, "from": max(0.0, t - every), "to": t + every, "kind": name})
    sidecar = Path(args.footage + ".blur.json")
    # Only where and what kind: the private text itself is never written down.
    sidecar.write_text(json.dumps(regions, indent=1))
    kinds = sorted({r["kind"] for r in regions})
    print(f"{len(regions)} region(s) to blur ({', '.join(kinds) or 'none'}) -> {sidecar}")


def ocr(args):
    """The finished video's check: lists every frame where private-looking text still shows."""
    hits = 0
    for name, t, box, word, line in private_hits(read_words(args.video, float(args.every)), private_patterns(args.also or [])):
        masked = line.replace(word, f"<{name}>")
        print(f"{t:7.1f}s  {name:8} at {box}  {masked[:100]}")
        hits += 1
    print(f"{hits} suspicious word(s)" if hits else "no private-looking text found")
    sys.exit(1 if hits else 0)


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("build")
    b.add_argument("storyboard")
    b.add_argument("--out")
    b.add_argument("--langs")
    b.add_argument("--work")
    b.add_argument("--keep", action="store_true", help="keep the work folder")
    r = sub.add_parser("brand")
    r.add_argument("--langs")
    r.add_argument("--title", default='{"en": "Feature title", "ko": "기능 이름", "ja": "機能名", "zh": "功能名称"}')
    s = sub.add_parser("sheet")
    s.add_argument("video")
    s.add_argument("--every", default="2")
    o = sub.add_parser("ocr")
    o.add_argument("video")
    o.add_argument("--every", default="1")
    o.add_argument("--also", action="append", help="another regex to treat as private")
    d = sub.add_parser("redact")
    d.add_argument("footage")
    d.add_argument("--every", default="0.5")
    d.add_argument("--pad", default="6")
    d.add_argument("--also", action="append", help="another regex to treat as private")
    args = parser.parse_args()
    if args.cmd == "build":
        build(args)
    elif args.cmd == "brand":
        langs = args.langs.split(",") if args.langs else LANGS
        titles = json.loads(args.title)
        for lang, (intro, outro) in brand(langs, {l: text_of(titles, l) for l in langs}, {}).items():
            print(lang, intro, outro)
    elif args.cmd == "sheet":
        sheet(args)
    elif args.cmd == "redact":
        redact(args)
    else:
        ocr(args)


if __name__ == "__main__":
    main()
