"""Checks a built film against what it was meant to show: `video.sh verify <video.mp4>`.

`film` writes `<slug>.<lang>.checks.json` beside the video: every text the viewer should read (the
opening title, headlines, sublines, labels, callouts, stat values, steps, quotes, each caption group),
where it should be on the canvas and when, every click the cursor makes with what it should land on,
and every stat or quote's source frame in the recording. This reads those places back with text
recognition and compares, then checks the narration against the captions and the storyboard for
missing translations.

A failure is printed as `t=… · kind · scene · expected "…" · read "…"`, a sheet of every failing crop
is written to `<video>.verify.png`, the whole result to `<video>.verify.json`, and the exit code is 1.
"""

import json
import re
import shutil
import subprocess
import sys
import tempfile
import unicodedata
from difflib import SequenceMatcher
from pathlib import Path

import video as v

OCR_LANGS = {"en": "en-US,ko-KR,ja-JP,zh-Hans", "ko": "ko-KR,en-US", "ja": "ja-JP,en-US", "zh": "zh-Hans,en-US"}
# How much of the expected text recognition must find, in order (recognition misreads a little).
NEED = {"caption": 0.8, "headline": 0.8, "subline": 0.75, "title": 0.8, "intro-title": 0.8, "intro-tagline": 0.75,
        "outro-site": 0.8, "label": 0.75, "callout": 0.7, "spot-label": 0.7, "stat": 1.0, "stat-label": 0.75,
        "step": 0.75, "quote": 0.75, "quote-by": 0.7, "click": 0.6, "spot-target": 0.6, "zoom-target": 0.6, "source": 0.7}
TEXT_KEYS = ("say", "headline", "subline", "labels", "items", "quote", "by", "label")


def norm(text):
    """Letters and digits only, case-folded, full-width forms folded: what recognition can compare."""
    text = unicodedata.normalize("NFKC", text or "").casefold()
    return "".join(c for c in text if c.isalnum())


def fold(word):
    """Letters recognition mixes up in a word that has letters ("Al" for "AI", "0" for "O");
    numbers stay as they are, so a wrong count still fails."""
    if not re.search(r"[a-z]", word):
        return word
    return word.translate(str.maketrans({"l": "i", "1": "i", "0": "o"}))


def coverage(expected, read):
    """How much of `expected` shows in `read` (1.0: all of it).

    Text with spaces is compared word by word: the share of its words recognition found (each
    allowed a misread letter or two), so "Six agents" never passes for "Five agents" on shared
    letters. Japanese and Chinese have no spaces: the best-matching stretch of the same length."""
    words = [fold(norm(w)) for w in (expected or "").split()]
    words = [w for w in words if w]
    if not words:
        return 1.0
    seen = [fold(norm(w)) for w in (read or "").split()]
    seen = [w for w in seen if w]
    # Japanese and Chinese go by characters even with spaces in them ("5 つのエージェント"):
    # recognition puts its own spaces in other places.
    if not re.search(r"[぀-ヿ一-鿿]", expected or ""):
        flat = "".join(seen)
        found = 0.0
        for w in words:
            best = max((SequenceMatcher(None, w, s, autojunk=False).ratio() for s in seen), default=0)
            # A word recognition glued to its neighbours ("Fiveagents") still counts.
            if best < 0.8 and w in flat:
                best = 1.0
            found += 1.0 if best >= 0.8 else 0.0
        return found / len(words)
    # Recognition returns a wrapped caption as two lines with other text between them: compare
    # against the lines that are in these scripts only, joined, and take the share of the expected
    # characters found in order.
    a = norm(expected)
    cjk = [p for p in (read or "").split() if re.search(r"[぀-ヿ一-鿿]", p)]
    best = 0.0
    # Against those lines alone, and against everything read (text that mixes in Latin words,
    # "@Backend 上面的约定"), whichever finds more.
    for b in {norm("".join(cjk)), norm(read)}:
        if not b:
            continue
        blocks = SequenceMatcher(None, a, b, autojunk=False).get_matching_blocks()
        in_order = sum(blk.size for blk in blocks) / len(a)
        # A long read can hold the expected characters by chance; a long run of them cannot.
        longest = max((blk.size for blk in blocks), default=0) / len(a)
        best = max(best, in_order if longest >= 0.3 else min(in_order, 0.5))
    return best


def clamp_box(box, W, H):
    x, y, w, h = (int(round(c)) for c in box)
    x, y = max(0, x), max(0, y)
    w, h = max(8, min(w, W - x)), max(8, min(h, H - y))
    return x, y, w, h


def grab(video, t, box, out, W, H, scale=2):
    x, y, w, h = clamp_box(box, W, H)
    v.run(["ffmpeg", "-v", "error", "-y", "-ss", f"{max(0.0, t):.3f}", "-i", str(video), "-frames:v", "1",
           # Vision reads nothing in a very wide strip (a caption band is 11:1): pad it to 4:1 at most.
           "-vf", f"crop={w}:{h}:{x}:{y},scale=iw*{scale}:ih*{scale}:flags=lanczos,pad=iw:'max(ih,trunc(iw/4/2)*2)':0:'(oh-ih)/2':black",
           str(out)])


def read_all(folder, lang):
    """Recognized lines per PNG of `folder`, in one Vision run."""
    env = {**dict(__import__("os").environ), "OCR_LANGS": OCR_LANGS.get(lang, OCR_LANGS["en"])}
    out = subprocess.run(["swift", str(Path(v.__file__).with_name("ocr.swift")), str(folder)], capture_output=True, text=True, env=env).stdout
    lines = {}
    for row in out.splitlines():
        cols = row.split("\t")
        if len(cols) >= 7:
            seen = lines.setdefault(cols[0], [])
            if cols[6] not in seen:
                seen.append(cols[6])
    return lines


def still_there(expected, lines):
    """Whether one recognized line *is* the caption (not footage text that happens to share letters)."""
    want = norm(expected)
    for line in lines:
        got = norm(line)
        if got and len(got) <= len(want) * 1.6 + 4 and coverage(expected, line) >= 0.8:
            return True
    return False


def speech_spans(video, total):
    log = subprocess.run(["ffmpeg", "-i", str(video), "-af", "silencedetect=n=-38dB:d=0.18", "-f", "null", "-"], capture_output=True, text=True).stderr
    starts = [float(x) for x in re.findall(r"silence_start: ([\d.]+)", log)]
    ends = [float(x) for x in re.findall(r"silence_end: ([\d.]+)", log)]
    # Speech is what lies between silences.
    spans, t = [], 0.0
    for s, e in zip(starts, ends + [total]):
        if s > t + 0.05:
            spans.append((t, s))
        t = e
    if t < total - 0.05 and len(ends) == len(starts):
        spans.append((t, total))
    return spans


def overlap(a, b, spans):
    return sum(max(0.0, min(b, y) - max(a, x)) for x, y in spans)


def verify(args):
    video = Path(args.video).resolve()
    m = re.search(r"-(en|ko|ja|zh)\.mp4$", video.name)
    if not m and not args.checks:
        sys.exit("name the checks file (--checks): the video's name has no -en/-ko/-ja/-zh")
    lang = m.group(1) if m else args.lang or "en"
    slug = video.name[: m.start()] if m else video.stem
    checks_path = Path(args.checks) if args.checks else video.with_name(f"{slug}.{lang}.checks.json")
    if not checks_path.exists():
        sys.exit(f"no {checks_path.name}: build the video with `video.sh film` first")
    data = json.loads(checks_path.read_text())
    if args.self_test:
        return self_test(video, data, lang)
    W, H, total = data["W"], data["H"], data["total"]
    checks = data["checks"]
    tmp = Path(tempfile.mkdtemp(prefix="verify-"))
    evaluate(video, data, lang, tmp)
    report_and_exit(video, data, lang, tmp, args)


def evaluate(video, data, lang, tmp):
    """Reads every check's place in `video` and records `result` on each check."""
    W, H, total = data["W"], data["H"], data["total"]
    checks = data["checks"]
    jobs = []  # (png name, check, role, time, box); role "show" must contain, "gone" must not

    for n, c in enumerate(checks):
        kind = c["kind"]
        if kind == "caption":
            a, b = c["a"], c["b"]
            # Only the caption's own width (it is centred): footage text beside it drowns out
            # Japanese and Chinese in recognition.
            wide = sum(42 if re.match(r"[぀-ヿ一-鿿가-힯]", ch) else 24 for ch in c["text"]) + 160
            bx, by, bw, bh = c["box"]
            box = [max(bx, (W - wide) / 2), by, min(bw, wide), bh]
            jobs.append((f"{n:04}-show.png", c, "show", a + (b - a) * 0.45, box))
            jobs.append((f"{n:04}-show2.png", c, "show2", a + (b - a) * 0.75, box))
            nxt = next((d for d in checks[n + 1:] if d["kind"] == "caption"), None)
            if b + 0.15 < total and (nxt is None or coverage(c["text"], nxt["text"]) < 0.5):
                jobs.append((f"{n:04}-gone.png", c, "gone", b + 0.12, box))
        elif kind == "click":
            x, y = c["x"], c["y"]
            sx, sy, sw, sh = c["stage"]
            if not (sx <= x <= sx + sw and sy <= y <= sy + sh):
                c["result"] = {"ok": False, "why": f"the click at ({x}, {y}) lands outside the window on the canvas"}
                continue
            # The words under the cursor tip, a line above and below (the arrow covers some of them).
            jobs.append((f"{n:04}-show.png", c, "show", c["t"], [x - 300, y - 100, 600, 200]))
        elif kind == "source":
            out = tmp / f"{n:04}-show.png"
            v.run(["ffmpeg", "-v", "error", "-y", "-ss", f"{c['at']:.3f}", "-i", c["clip"], "-frames:v", "1", str(out)])
            jobs.append((out.name, c, "show", None, None))
        elif "near" in c:
            x, y, w, h = c["near"]
            jobs.append((f"{n:04}-show.png", c, "show", c["t"], [x - 220, y - 120, w + 440, h + 240]))
        else:
            jobs.append((f"{n:04}-show.png", c, "show", c["t"], c["box"]))

    for name, c, role, t, box in jobs:
        if t is not None:
            grab(video, t, box, tmp / name, W, H, scale=3 if c["kind"] == "click" else 2 if box[3] < 400 else 1)
    read = read_all(tmp, lang)

    results = []
    # A caption read at two moments passes when either reading finds it (one frame can catch a
    # word lighting up, or footage text crowding the line).
    second = {}
    for name, c, role, t, box in jobs:
        if role == "show2":
            second[id(c)] = read.get(name, [])
    for name, c, role, t, box in jobs:
        if role == "show2":
            continue
        lines = read.get(name, [])
        got = " ".join(lines)
        score = coverage(c["text"], got)
        if id(c) in second:
            other = coverage(c["text"], " ".join(second[id(c)]))
            if other > score:
                score, got = other, " ".join(second[id(c)])
        need = NEED.get(c["kind"], 0.75)
        ok = score >= need if role == "show" else not still_there(c["text"], lines)
        why = None if ok else ("not found where and when it should show" if role == "show" else "still on screen after its time")
        r = c.setdefault("result", {"ok": True})
        if not ok or r.get("ok", True):
            c["result"] = {"ok": ok and r.get("ok", True), "score": round(score, 2), "read": got[:160], "why": why or r.get("why"),
                           "png": name if not ok else r.get("png")}


# Each plants one error of a kind that matters; the checker must flag the check it touched.
MUTATIONS = [
    ("a headline with other words", "headline", lambda c: c.update(text="Nothing like this here")),
    ("a caption shown 4 s late", "caption", lambda c: c.update(a=c["a"] + 4, b=c["b"] + 4)),
    ("a click on the wrong control", "click", lambda c: c.update(text="Unrelated Button")),
    ("a label in the wrong place", "label", lambda c: c.update(box=[c["box"][0] + 900, c["box"][1] + 500, c["box"][2], c["box"][3]])),
    ("a stat with the wrong number", "stat", lambda c: c.update(text=str(int(re.sub(r"\D", "", c["text"]) or 0) + 17))),
    ("a step that is not on the card", "step", lambda c: c.update(text="Ship it to the moon")),
    ("a quote that was never written", "quote", lambda c: c.update(text="Nobody ever wrote these words")),
]


def self_test(video, data, lang):
    """Plants errors into a copy of the checks and confirms each is caught, and that the real checks pass."""
    import copy

    tmp = Path(tempfile.mkdtemp(prefix="verify-self-"))
    clean = copy.deepcopy(data)
    evaluate(video, clean, lang, tmp)
    false_alarms = [c for c in clean["checks"] if not c.get("result", {}).get("ok", True)]
    broken = copy.deepcopy(data)
    planted = []
    for label, kind, mutate in MUTATIONS:
        target = next((c for c in broken["checks"] if c["kind"] == kind and "_planted" not in c), None)
        if target:
            mutate(target)
            target["_planted"] = label
            planted.append(target)
    shutil.rmtree(tmp, ignore_errors=True)
    tmp = Path(tempfile.mkdtemp(prefix="verify-self-"))
    evaluate(video, broken, lang, tmp)
    shutil.rmtree(tmp, ignore_errors=True)
    missed = [c for c in planted if c.get("result", {}).get("ok", True)]
    print(f"self-test on {video.name}:")
    for c in planted:
        print(f"  {'caught' if not c['result']['ok'] else 'MISSED'}  {c['_planted']}")
    print(f"  real checks: {len(clean['checks']) - len(false_alarms)}/{len(clean['checks'])} pass"
          + (" — false alarms: " + ", ".join(f"{c['kind']}@{c.get('t', c.get('a', 0)):.1f}s" for c in false_alarms) if false_alarms else ""))
    sys.exit(1 if missed or false_alarms else 0)


def report_and_exit(video, data, lang, tmp, args):
    W, H, total = data["W"], data["H"], data["total"]
    checks = data["checks"]
    # Narration against captions: speech inside every caption group, and no long speech without one.
    spans = speech_spans(video, total)
    caps = [c for c in checks if c["kind"] == "caption"]
    audio = []
    for c in caps:
        heard = overlap(c["a"], c["b"], spans) / max(0.01, c["b"] - c["a"])
        if heard < 0.35:
            audio.append(f"t={c['a']:.2f}s · caption with little speech under it ({heard:.0%}) · \"{c['text'][:50]}\"")
    for x, y in spans:
        if y - x > 0.5 and overlap(x, y, [(c["a"] - 0.15, c["b"] + 0.15) for c in caps]) < (y - x) * 0.5 and caps:
            audio.append(f"t={x:.2f}s · {y - x:.1f}s of speech with no caption")

    # Missing translations: English shows where a language is left out.
    board_path = video.with_name("storyboard.json")
    gaps = []
    if board_path.exists() and lang != "en":
        board = json.loads(board_path.read_text())

        def look(value, where):
            if isinstance(value, dict) and "en" in value and lang not in value:
                gaps.append(f"{where}: no {lang} text, English shows (\"{value['en'][:40]}\")")
            elif isinstance(value, list):
                for i, item in enumerate(value):
                    look(item, f"{where}[{i}]")

        look(board.get("title"), "title")
        look(board.get("tagline"), "tagline")
        for i, s in enumerate(board.get("scenes", [])):
            for k in TEXT_KEYS:
                look(s.get(k), f"scene {i + 1} {k}")
            for j, call in enumerate(s.get("callouts", []) + s.get("spotlight", [])):
                look(call.get("label"), f"scene {i + 1} callout {j + 1}")

    failed = [c for c in checks if not c.get("result", {}).get("ok", True)]
    by_kind = {}
    for c in checks:
        k = c["kind"]
        ok = c.get("result", {}).get("ok", True)
        p, f = by_kind.get(k, (0, 0))
        by_kind[k] = (p + ok, f + (not ok))
    print(f"{video.name}: {len(checks) - len(failed)}/{len(checks)} checks passed")
    for k, (p, f) in sorted(by_kind.items()):
        print(f"  {k:14} {p} ok" + (f", {f} failed" if f else ""))
    for c in failed:
        r = c["result"]
        t = c.get("t", c.get("a", c.get("at", 0)))
        print(f"  FAIL t={t:.2f}s · {c['kind']} · scene {c.get('scene', '-')} · expected \"{c['text'][:60]}\" · read \"{r.get('read', '')[:60]}\""
              f" ({r.get('score', 0):.0%}) · {r.get('why')}")
    for line in audio:
        print(f"  AUDIO {line}")
    for line in gaps:
        print(f"  TEXT {line}")

    report = {"video": video.name, "lang": lang, "passed": len(checks) - len(failed), "total": len(checks),
              "failed": failed, "audio": audio, "missing_translations": gaps}
    video.with_suffix(".verify.json").write_text(json.dumps(report, ensure_ascii=False, indent=1))
    if failed:
        tiles = [(tmp / c["result"]["png"], f"{c['kind']} t={c.get('t', c.get('a', 0)):.1f}s") for c in failed if c["result"].get("png")]
        tiles = [(p, label) for p, label in tiles if p.exists()]
        if tiles:
            width = 900
            rows = []
            for p, label in tiles:
                im = v.Image.open(p).convert("RGB")
                im = im.resize((width, max(1, int(im.height * width / im.width))))
                rows.append((im, label))
            sheet = v.Image.new("RGB", (width, sum(im.height + 34 for im, _ in rows)), (0, 0, 0))
            draw = v.ImageDraw.Draw(sheet)
            y = 0
            for im, label in rows:
                draw.text((8, y + 6), label, font=v.font("en", 20), fill=(255, 220, 90))
                sheet.paste(im, (0, y + 34))
                y += im.height + 34
            sheet.save(video.with_suffix(".verify.png"))
            print(f"  failing crops: {video.with_suffix('.verify.png')}")
    shutil.rmtree(tmp, ignore_errors=True)
    sys.exit(1 if failed or (args.strict and (audio or gaps)) else 0)
