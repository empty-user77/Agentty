"""The motion-graphics library of `film.py`: themes, title effects, transitions, accents, the extra
layouts and the director that varies them so no two films (and no two scenes) feel alike.

Every effect is GSAP on the paused HyperFrames timeline, CSS or inline SVG: a frame is a function of
time, so renders are identical. Ideas come from the HyperFrames registry (transitions, kinetic type,
hand-drawn callouts, lower thirds, camera moves); the code here is our own.
"""

import html
import math
import re

# ---------------------------------------------------------------- themes

# Each theme: colours (CSS variables), the ground under every scene (markup + main-timeline tweens
# over the whole film), how a footage window looks, and which title effects and transitions suit it.
THEMES = {
    "midnight": {
        "vars": {"--ground": "#0a0d14", "--ground2": "#111622", "--ink": "#f5f7fa", "--muted": "#9ca3af",
                 "--panel": "rgba(255,255,255,.06)", "--line": "rgba(255,255,255,.09)", "--cap-bg": "rgba(6,8,13,.86)"},
        "window": "box-shadow: 0 32px 90px rgba(0,0,0,.55), 0 0 0 1px rgba(255,255,255,.09);",
        "titles": ["mask-up", "words-blur", "slam", "rise", "tracking", "highlight", "scramble", "letters"],
        "seams": ["curve-left", "cover", "zoom-blur", "light-leak", "doors"],
        "sweep": True,
    },
    "aurora": {
        "vars": {"--ground": "#070a16", "--ground2": "#0c1024", "--ink": "#f7f8ff", "--muted": "#aab0d6",
                 "--panel": "rgba(255,255,255,.08)", "--line": "rgba(255,255,255,.16)", "--cap-bg": "rgba(8,10,24,.82)"},
        "window": "box-shadow: 0 40px 120px rgba(40,20,120,.55), 0 0 0 1px rgba(255,255,255,.18), inset 0 0 0 1px rgba(255,255,255,.06);",
        "titles": ["rise", "highlight", "tracking", "mask-up", "words-blur", "letters"],
        "seams": ["light-leak", "zoom-blur", "curve-left", "flash", "doors"],
        "sweep": True,
    },
    "neon": {
        "vars": {"--ground": "#030407", "--ground2": "#07080f", "--ink": "#ffffff", "--muted": "#8b93a7",
                 "--panel": "rgba(253,207,1,.07)", "--line": "rgba(253,207,1,.35)", "--cap-bg": "rgba(0,0,0,.84)"},
        "window": "box-shadow: 0 0 0 2px rgba(253,207,1,.55), 0 0 40px rgba(253,207,1,.28), 0 30px 90px rgba(0,0,0,.8);",
        "titles": ["slam", "scramble", "glitch", "mask-up", "tracking"],
        "seams": ["glitch", "pixelate", "flash", "cube", "curve-left"],
        "sweep": False,
        "glow": True,
    },
    "editorial": {
        "vars": {"--ground": "#f2eee6", "--ground2": "#e9e3d6", "--ink": "#121212", "--muted": "#5d5a52",
                 "--panel": "rgba(0,0,0,.04)", "--line": "rgba(0,0,0,.12)", "--cap-bg": "rgba(18,18,18,.88)",
                 # The brand yellow cannot carry text on cream: accent words are ink on a yellow marker.
                 "--accent-ink": "#121212", "--mark-op": ".75"},
        "window": "box-shadow: 0 30px 70px rgba(60,45,20,.28), 0 0 0 1px rgba(0,0,0,.12);",
        "titles": ["highlight", "mask-up", "rise", "tracking", "words-ink"],
        "seams": ["cover", "blinds", "curve-left", "doors", "blur-swap"],
        "sweep": False,
    },
    "blueprint": {
        "vars": {"--ground": "#0a1a33", "--ground2": "#0d2142", "--ink": "#eaf2ff", "--muted": "#8fb0dc",
                 "--panel": "rgba(120,170,255,.08)", "--line": "rgba(140,190,255,.35)", "--cap-bg": "rgba(5,14,30,.86)"},
        "window": "box-shadow: 0 0 0 1px rgba(140,190,255,.45), 0 30px 80px rgba(0,0,0,.5);",
        "titles": ["scramble", "tracking", "mask-up", "rise", "letters"],
        "seams": ["pixelate", "blinds", "curve-left", "zoom-blur", "cube"],
        "sweep": False,
        "brackets": True,
    },
}
THEME_NAMES = sorted(THEMES)


def theme_of(board):
    name = board.get("theme", "midnight")
    if name not in THEMES:
        raise SystemExit(f"theme must be one of {', '.join(THEME_NAMES)}")
    return name, THEMES[name]


def theme_css(name, accent):
    t = THEMES[name]
    vars_ = ";".join(f"{k}:{v}" for k, v in t["vars"].items())
    css = f":root {{ {vars_}; --accent: {accent}; }}\n.stage {{ {t['window']} }}\n"
    if t.get("glow"):
        css += ".head h2, .title-card h1, .kin-word { text-shadow: 0 0 24px rgba(253,207,1,.35), 0 0 2px rgba(255,255,255,.4); }\n"
    if name == "editorial":
        css += ".bar { background: var(--ink) !important; } .cur-svg path { fill: #fff; }\n"
    return css


def ground(name, W, H, total):
    """The ground under every scene, and its slow motion over the whole film (it never stops dead,
    and it never loops: one long move from start to end)."""
    T = max(1.0, total)
    if name == "aurora":
        blobs = [("#2f6bff", -0.1, 0.0, 0.9), ("#8b3dff", 0.55, 0.15, 0.8), ("#00c2a8", 0.2, 0.55, 0.7), ("#fdcf01", 0.85, 0.7, 0.35)]
        markup = "".join(
            f'<div id="gb{n}" class="blob" data-layout-ignore style="left:{x * W:.0f}px;top:{y * H:.0f}px;width:{W * 0.7:.0f}px;height:{W * 0.7:.0f}px;'
            f'background:radial-gradient(circle, {c} 0%, transparent 62%);opacity:{o}"></div>'
            for n, (c, x, y, o) in enumerate(blobs))
        js = "".join(
            f'tl.fromTo("#gb{n}", {{x: 0, y: 0, scale: 1}}, {{x: {(-1) ** n * W * 0.22:.0f}, y: {(-1) ** (n + 1) * H * 0.18:.0f}, '
            f'scale: {1.25 if n % 2 else 0.85}, duration: {T:.2f}, ease: "sine.inOut"}}, 0);\n' for n in range(len(blobs)))
        return f'<div id="ground" class="g-aurora">{markup}<div class="g-veil"></div></div>', js
    if name == "neon":
        markup = ('<div id="ground" class="g-neon"><div class="g-sun"></div><div class="g-floor-wrap"><div id="gfloor" class="g-floor"></div></div>'
                  '<div class="g-scan"></div></div>')
        return markup, f'tl.fromTo("#gfloor", {{backgroundPosition: "0px 0px"}}, {{backgroundPosition: "0px {int(T * 60)}px", duration: {T:.2f}, ease: "none"}}, 0);\n'
    if name == "editorial":
        return ('<div id="ground" class="g-editorial"><div id="gword" class="g-word" data-layout-ignore aria-hidden="true">AGENTTY</div><div class="g-grain"></div></div>',
                f'tl.fromTo("#gword", {{x: 0}}, {{x: {-W * 0.25:.0f}, duration: {T:.2f}, ease: "none"}}, 0);\n')
    if name == "blueprint":
        return ('<div id="ground" class="g-blueprint"><div id="ggrid" class="g-grid"></div><div class="g-vig"></div></div>',
                f'tl.fromTo("#ggrid", {{backgroundPosition: "0px 0px, 0px 0px"}}, {{backgroundPosition: "{int(T * 8)}px {int(T * 4)}px, {int(T * 8)}px {int(T * 4)}px", duration: {T:.2f}, ease: "none"}}, 0);\n')
    return ('<div id="ground" class="g-midnight"><div id="gglow" class="g-glow"></div><div class="g-grid-faint"></div></div>',
            f'tl.fromTo("#gglow", {{x: 0, y: 0}}, {{x: {W * 0.35:.0f}, y: {H * 0.1:.0f}, duration: {T:.2f}, ease: "sine.inOut"}}, 0);\n')


GROUND_CSS = """
#ground { position: absolute; inset: 0; overflow: hidden; background: linear-gradient(180deg, var(--ground) 0%, var(--ground2) 100%); }
.g-glow { position: absolute; left: -10%; top: -40%; width: 80%; height: 110%; background: radial-gradient(closest-side, rgba(0,120,190,.26), transparent); }
.g-grid-faint { position: absolute; inset: 0; background-image: linear-gradient(rgba(255,255,255,.025) 1px, transparent 1px), linear-gradient(90deg, rgba(255,255,255,.025) 1px, transparent 1px); background-size: 64px 64px; -webkit-mask-image: radial-gradient(ellipse at 50% 40%, #000 30%, transparent 75%); }
.blob { position: absolute; border-radius: 50%; filter: blur(40px); margin-left: -20%; margin-top: -20%; }
.g-veil { position: absolute; inset: 0; background: radial-gradient(ellipse at 50% 120%, rgba(0,0,0,.55), transparent 60%); }
.g-neon .g-sun { position: absolute; left: 50%; top: 18%; width: 46%; aspect-ratio: 1; transform: translateX(-50%); border-radius: 50%; background: radial-gradient(circle, rgba(253,207,1,.22), rgba(255,60,140,.08) 45%, transparent 65%); }
.g-floor-wrap { position: absolute; left: -50%; right: -50%; bottom: -10%; height: 60%; perspective: 600px; }
.g-floor { position: absolute; inset: 0; transform: rotateX(68deg); transform-origin: 50% 100%; background-image: linear-gradient(rgba(253,207,1,.35) 2px, transparent 2px), linear-gradient(90deg, rgba(253,207,1,.35) 2px, transparent 2px); background-size: 80px 80px; -webkit-mask-image: linear-gradient(to top, #000 10%, transparent 85%); }
.g-scan { position: absolute; inset: 0; background: repeating-linear-gradient(0deg, rgba(255,255,255,.025) 0 1px, transparent 1px 4px); }
.g-editorial .g-word { position: absolute; left: 4%; top: 50%; margin-top: -0.55em; font-size: 520px; font-weight: 600; letter-spacing: -0.05em; color: rgba(0,0,0,.035); white-space: nowrap; }
.g-grain { position: absolute; inset: 0; opacity: .5; background-image: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' width='220' height='220'><filter id='n'><feTurbulence type='fractalNoise' baseFrequency='.85' numOctaves='2' seed='7'/><feColorMatrix values='0 0 0 0 0  0 0 0 0 0  0 0 0 0 0  0 0 0 .09 0'/></filter><rect width='100%' height='100%' filter='url(%23n)'/></svg>"); }
.g-grid { position: absolute; inset: -200px; background-image: linear-gradient(rgba(140,190,255,.10) 1px, transparent 1px), linear-gradient(90deg, rgba(140,190,255,.10) 1px, transparent 1px); background-size: 40px 40px, 40px 40px; }
.g-blueprint .g-grid::after { content: ""; position: absolute; inset: 0; background-image: linear-gradient(rgba(140,190,255,.18) 1px, transparent 1px), linear-gradient(90deg, rgba(140,190,255,.18) 1px, transparent 1px); background-size: 200px 200px; }
.g-vig { position: absolute; inset: 0; background: radial-gradient(ellipse at 50% 45%, transparent 40%, rgba(0,0,0,.45)); }
"""


# ---------------------------------------------------------------- text and title effects


def esc(text):
    return html.escape(text or "", quote=True)


def plain(text):
    """A headline without its emphasis marks (`*word*`): what the viewer reads."""
    return (text or "").replace("*", "")


def words_html(text, lang, cjk):
    """A headline as spans to animate: `.w` (a word, or a character in Japanese and Chinese) with an
    inner `.wi` for masked rises; `*marked*` words get `.em` (the accent and the marker sweep)."""
    if not text:
        return ""
    out, em = [], False
    pieces = re.split(r"(\*)", text)
    for piece in pieces:
        if piece == "*":
            em = not em
            continue
        if not piece:
            continue
        if lang in cjk:
            tokens = re.findall(r"[A-Za-z0-9⌘.+-]+[。、，！？」』）]*|\s+|.[。、，！？」』）]*", piece)
        else:
            tokens = re.findall(r"\S+|\s+", piece)
        for t in tokens:
            if not t.strip():
                out.append(" ")
            else:
                out.append(f'<span class="w{" em" if em else ""}"><span class="wi">{esc(t)}</span></span>')
    return "".join(out)


SCRAMBLE = "ABCDEFGHJKLMNPQRSTUVWXYZ0123456789#%&*+=/<>"


def title_fx(sel, style, at, ink):
    """Tweens that bring in the headline `sel` (its `.w` / `.wi` spans) with `style`."""
    w, wi = f"{sel} .w", f"{sel} .wi"
    if style == "mask-up":
        return (f'tl.set("{sel}", {{opacity: 1}}, {at});\n'
                f'tl.fromTo("{wi}", {{yPercent: 110}}, {{yPercent: 0, duration: 0.62, stagger: 0.06, ease: "expo.out"}}, {at});\n'
                f'tl.set("{w}", {{overflow: "hidden"}}, {at});\n'
                f'tl.set("{w}", {{overflow: "visible"}}, {at + 1.4:.2f});\n')
    if style == "slam":
        return (f'tl.fromTo("{sel}", {{opacity: 0, scale: 1.7, filter: "blur(12px)"}}, {{opacity: 1, scale: 1, filter: "blur(0px)", duration: 0.26, ease: "power4.in"}}, {at});\n'
                + "".join(f'tl.set("{sel}", {{x: {dx}, y: {dy}}}, {at + 0.26 + k / 30:.3f});\n'
                          for k, (dx, dy) in enumerate([(9, -5), (-7, 4), (4, -2), (0, 0)])))
    if style == "rise":
        return f'tl.fromTo("{w}", {{opacity: 0, y: 46, filter: "blur(8px)"}}, {{opacity: 1, y: 0, filter: "blur(0px)", duration: 0.55, stagger: 0.07, ease: "expo.out"}}, {at});\n'
    if style == "tracking":
        # Words start spread apart from the middle and close in (transforms only: letter-spacing
        # snaps to whole pixels and shimmers).
        return (f'tl.fromTo("{w}", {{opacity: 0, x: (i, el, all) => (i - (all.length - 1) / 2) * 90, filter: "blur(6px)"}}, '
                f'{{opacity: 1, x: 0, filter: "blur(0px)", duration: 0.9, ease: "expo.out"}}, {at});\n')
    if style == "scramble" or style == "glitch":
        js = (f'(() => {{ const els = [...document.querySelectorAll("{wi}")]; const fin = els.map(e => e.textContent); const o = {{p: 0}};'
              f' const G = {SCRAMBLE!r};'
              f' tl.fromTo(o, {{p: 0}}, {{p: 1, duration: 0.8, ease: "none", onUpdate: () => {{'
              f' const step = Math.floor(o.p * 24); els.forEach((e, i) => {{ const f = fin[i]; const keep = Math.floor(o.p * 1.35 * f.length - i * 0.4);'
              f' e.textContent = f.split("").map((ch, j) => (j < keep || /[\\s\\u3000-\\u9fff\\uac00-\\ud7af]/.test(ch)) ? ch : G[(j * 7 + i * 13 + step * 5) % G.length]).join(""); }}); }} }}, {at}); }})();\n'
              f'tl.fromTo("{sel}", {{opacity: 0}}, {{opacity: 1, duration: 0.1}}, {at});\n')
        if style == "glitch":
            js += "".join(f'tl.set("{sel}", {{x: {dx}, filter: "drop-shadow({dx * 0.8:.0f}px 0 0 rgba(255,0,80,.8)) drop-shadow({-dx * 0.8:.0f}px 0 0 rgba(0,220,255,.8))"}}, {at + k / 30:.3f});\n'
                          for k, dx in enumerate([10, -8, 6, -12, 4, 0])) + f'tl.set("{sel}", {{filter: "none"}}, {at + 0.2:.3f});\n'
        return js
    if style == "highlight":
        return (f'tl.fromTo("{w}", {{opacity: 0, filter: "blur(6px)"}}, {{opacity: 1, filter: "blur(0px)", duration: 0.13, stagger: 0.067, ease: "power4.out"}}, {at});\n'
                f'tl.fromTo("{sel} .em", {{"--mark": 0}}, {{"--mark": 1, duration: 0.45, stagger: 0.12, ease: "power2.inOut"}}, {at + 0.45:.2f});\n')
    if style == "words-ink":
        return (f'tl.fromTo("{w}", {{color: "#4b5563"}}, {{color: "{ink}", duration: 0.25, stagger: 0.085, ease: "power1.out"}}, {at});\n'
                f'tl.fromTo("{sel}", {{opacity: 0}}, {{opacity: 1, duration: 0.12}}, {at});\n')
    if style == "line-blur":
        return f'tl.fromTo("{sel}", {{opacity: 0, filter: "blur(8px)"}}, {{opacity: 1, filter: "blur(0px)", duration: 0.2, ease: "power4.out"}}, {at});\n'
    if style == "letters":
        return f'tl.fromTo("{w}", {{opacity: 0.3, filter: "blur(3px)"}}, {{opacity: 1, filter: "blur(0px)", duration: 0.16, stagger: 0.05, ease: "power2.out"}}, {at});\n'
    return f'tl.fromTo("{w}", {{opacity: 0, filter: "blur(6px)"}}, {{opacity: 1, filter: "blur(0px)", duration: 0.13, stagger: 0.067, ease: "power4.out"}}, {at});\n'


TITLE_STYLES = {"words-blur", "words-ink", "line-blur", "letters", "mask-up", "slam", "rise", "tracking", "scramble", "glitch", "highlight"}

TEXT_CSS = """
.w { display: inline-block; vertical-align: top; }
.wi { display: inline-block; }
.w.em { color: var(--accent-ink, var(--accent)); position: relative; isolation: isolate; --mark: 0; }
.w.em::before { content: ""; position: absolute; left: -0.08em; right: -0.08em; bottom: 0.08em; height: 0.36em; background: var(--accent); opacity: var(--mark-op, .28); border-radius: 4px; transform: scaleX(var(--mark)); transform-origin: 0 50%; z-index: -1; }
.chip { position: absolute; top: 46px; right: 64px; font-size: 30px; letter-spacing: .18em; color: var(--muted); font-family: "Body", sans-serif; }
.chip b { color: var(--accent-ink, var(--accent)); font-weight: 600; margin-right: 10px; }
"""


# ---------------------------------------------------------------- stage entrances and accents

ENTERS = ["rise", "tilt", "scale", "wipe", "swing"]


def enter_fx(sel, kind, at=0.0):
    """How a footage window arrives inside its scene (on top of the seam's own move)."""
    if kind == "tilt":
        return (f'tl.fromTo("{sel}", {{rotationX: 24, y: 60, opacity: 0.2, transformPerspective: 1600, transformOrigin: "50% 100%"}}, '
                f'{{rotationX: 0, y: 0, opacity: 1, duration: 0.9, ease: "expo.out"}}, {at});\n')
    if kind == "swing":
        return (f'tl.fromTo("{sel}", {{rotationY: -22, x: 80, opacity: 0.2, transformPerspective: 1800, transformOrigin: "0% 50%"}}, '
                f'{{rotationY: 0, x: 0, opacity: 1, duration: 0.95, ease: "expo.out"}}, {at});\n')
    if kind == "scale":
        return f'tl.fromTo("{sel}", {{scale: 0.86, opacity: 0.3}}, {{scale: 1, opacity: 1, duration: 0.7, ease: "expo.out"}}, {at});\n'
    if kind == "wipe":
        return f'tl.fromTo("{sel}", {{clipPath: "inset(0% 100% 0% 0% round 18px)"}}, {{clipPath: "inset(0% 0% 0% 0% round 18px)", duration: 0.7, ease: "power3.inOut"}}, {at});\n'
    if kind == "rise":
        return f'tl.fromTo("{sel}", {{y: 70, opacity: 0.2}}, {{y: 0, opacity: 1, duration: 0.7, ease: "expo.out"}}, {at});\n'
    return ""


def sweep_fx(prefix, w, h, at):
    """A light band that crosses the window once as it lands."""
    markup = f'<div id="{prefix}-sweep" class="sweep" data-layout-ignore style="width:{w * 0.35:.0f}px;height:{h * 1.6:.0f}px"></div>'
    js = (f'tl.fromTo("#{prefix}-sweep", {{x: {-w * 0.5:.0f}, y: {-h * 0.3:.0f}}}, '
          f'{{x: {w * 1.2:.0f}, y: {-h * 0.3:.0f}, duration: 1.0, ease: "power2.inOut"}}, {at});\n'
          f'tl.fromTo("#{prefix}-sweep", {{opacity: 0}}, {{opacity: 1, duration: 0.3}}, {at});\n'
          f'tl.to("#{prefix}-sweep", {{opacity: 0, duration: 0.2}}, {at + 0.8:.2f});\n')
    return markup, js


def brackets_fx(prefix, w, h, at):
    """Corner brackets that close in on the window (the blueprint look)."""
    s = 34
    corners = [(0, 0, "border-left,border-top"), (w - s, 0, "border-right,border-top"), (0, h - s, "border-left,border-bottom"), (w - s, h - s, "border-right,border-bottom")]
    markup = "".join(
        f'<div id="{prefix}-bk{n}" class="bk" style="left:{x}px;top:{y}px;width:{s}px;height:{s}px;'
        + ";".join(f"{side}:3px solid var(--accent)" for side in sides.split(",")) + '"></div>'
        for n, (x, y, sides) in enumerate(corners))
    js = "".join(f'tl.fromTo("#{prefix}-bk{n}", {{x: {(-1 if x == 0 else 1) * 40}, y: {(-1 if y == 0 else 1) * 40}, opacity: 0}}, '
                 f'{{x: 0, y: 0, opacity: 1, duration: 0.55, ease: "expo.out"}}, {at + n * 0.04:.2f});\n'
                 for n, (x, y, _) in enumerate(corners))
    return markup, js


def wobble_ellipse(w, h, seed=3):
    """A hand-drawn ellipse path around a w×h box: a little more than one turn, uneven radius."""
    cx, cy = w / 2, h / 2
    rx, ry = w / 2 + 26, h / 2 + 22
    pts = []
    n = 46
    for k in range(n + 6):
        a = -2.4 + 2 * math.pi * k / n
        wob = 1 + 0.035 * math.sin(k * 1.7 + seed) + 0.02 * math.sin(k * 0.6 + seed * 2)
        pts.append((cx + rx * wob * math.cos(a), cy + ry * wob * math.sin(a)))
    d = f"M{pts[0][0]:.1f},{pts[0][1]:.1f} " + " ".join(f"L{x:.1f},{y:.1f}" for x, y in pts[1:])
    return d


def callout_shape(prefix, n, kind, rect, a0, a1, length_hint=4000):
    """A callout's drawn shape in stage pixels: `circle` (a hand-drawn ring), `brackets`, `arrow`
    or the default spotlight. Returns markup and tweens."""
    sx, sy, sw, sh = rect
    if kind == "circle":
        d = wobble_ellipse(sw, sh, seed=n + 2)
        markup = (f'<svg id="{prefix}-ring{n}" class="ring" style="left:{sx - 40:.0f}px;top:{sy - 40:.0f}px;width:{sw + 80:.0f}px;height:{sh + 80:.0f}px" '
                  f'viewBox="-40 -40 {sw + 80:.0f} {sh + 80:.0f}"><path d="{d}" pathLength="1000"/></svg>')
        js = (f'tl.fromTo("#{prefix}-ring{n} path", {{strokeDashoffset: 1000}}, {{strokeDashoffset: 0, duration: 0.6, ease: "power2.inOut"}}, {a0:.3f});\n'
              f'tl.fromTo("#{prefix}-ring{n}", {{opacity: 1}}, {{opacity: 0, duration: 0.22}}, {max(a0 + 0.7, a1 - 0.22):.3f});\n')
        return markup, js
    if kind == "brackets":
        s = max(18, min(40, sw * 0.18, sh * 0.3))
        pad = 10
        corners = [(sx - pad, sy - pad, "l,t"), (sx + sw + pad - s, sy - pad, "r,t"), (sx - pad, sy + sh + pad - s, "l,b"), (sx + sw + pad - s, sy + sh + pad - s, "r,b")]
        side = {"l": "border-left", "r": "border-right", "t": "border-top", "b": "border-bottom"}
        markup = "".join(f'<div id="{prefix}-cb{n}x{k}" class="bk" style="left:{x:.0f}px;top:{y:.0f}px;width:{s:.0f}px;height:{s:.0f}px;'
                         + ";".join(f"{side[q]}:4px solid var(--accent)" for q in sides.split(",")) + '"></div>'
                         for k, (x, y, sides) in enumerate(corners))
        js = "".join(f'tl.fromTo("#{prefix}-cb{n}x{k}", {{x: {(-1 if "l" in sides else 1) * 60}, y: {(-1 if "t" in sides else 1) * 60}, opacity: 0}}, '
                     f'{{x: 0, y: 0, opacity: 1, duration: 0.5, ease: "expo.out"}}, {a0:.3f});\n'
                     f'tl.to("#{prefix}-cb{n}x{k}", {{opacity: 0, duration: 0.2}}, {max(a0 + 0.6, a1 - 0.2):.3f});\n'
                     for k, (x, y, sides) in enumerate(corners))
        return markup, js
    if kind == "arrow":
        # From below-left of the rect, curving into its edge.
        x0, y0 = sx - 180, sy + sh + 120
        x1, y1 = sx - 12, sy + sh / 2
        cx, cy = x0 + 20, y1 + 10
        left, top = min(x0, x1) - 30, min(y0, y1) - 30
        wv, hv = abs(x1 - x0) + 60, abs(y1 - y0) + 60
        d = f"M{x0 - left:.0f},{y0 - top:.0f} Q{cx - left:.0f},{cy - top:.0f} {x1 - left:.0f},{y1 - top:.0f}"
        head = f"M{x1 - left - 22:.0f},{y1 - top - 14:.0f} L{x1 - left:.0f},{y1 - top:.0f} L{x1 - left - 18:.0f},{y1 - top + 18:.0f}"
        markup = (f'<svg id="{prefix}-arw{n}" class="ring" style="left:{left:.0f}px;top:{top:.0f}px;width:{wv:.0f}px;height:{hv:.0f}px" viewBox="0 0 {wv:.0f} {hv:.0f}">'
                  f'<path d="{d}" pathLength="1000"/><path class="head" d="{head}" pathLength="1000"/></svg>')
        js = (f'tl.fromTo("#{prefix}-arw{n} path:not(.head)", {{strokeDashoffset: 1000}}, {{strokeDashoffset: 0, duration: 0.5, ease: "power2.out"}}, {a0:.3f});\n'
              f'tl.fromTo("#{prefix}-arw{n} .head", {{strokeDashoffset: 1000}}, {{strokeDashoffset: 0, duration: 0.18, ease: "power2.out"}}, {a0 + 0.45:.3f});\n'
              f'tl.to("#{prefix}-arw{n}", {{opacity: 0, duration: 0.2}}, {max(a0 + 0.7, a1 - 0.2):.3f});\n')
        return markup, js
    return None


def ripple_fx(prefix, n, x, y, t, size):
    """A ring that spreads from the cursor tip on a click."""
    markup = f'<div id="{prefix}-rp{n}" class="ripple" style="left:{x - size / 2:.0f}px;top:{y - size / 2:.0f}px;width:{size:.0f}px;height:{size:.0f}px"></div>'
    js = (f'tl.fromTo("#{prefix}-rp{n}", {{scale: 0.2, opacity: 0.9}}, {{scale: 1.6, opacity: 0, duration: 0.55, ease: "power2.out"}}, {t:.3f});\n')
    return markup, js


ACCENT_CSS = """
.sweep { position: absolute; left: 0; top: 0; pointer-events: none; opacity: 0; transform: rotate(18deg); background: linear-gradient(90deg, transparent, rgba(255,255,255,.16), transparent); mix-blend-mode: screen; z-index: 6; }
.bk { position: absolute; z-index: 7; pointer-events: none; }
.ring { position: absolute; z-index: 7; overflow: visible; pointer-events: none; }
.ring path { fill: none; stroke: var(--accent); stroke-width: 7; stroke-linecap: round; stroke-linejoin: round; stroke-dasharray: 1000; filter: drop-shadow(0 2px 6px rgba(0,0,0,.45)); }
.ripple { position: absolute; z-index: 6; border-radius: 50%; border: 5px solid var(--accent); opacity: 0; pointer-events: none; }
"""


# ---------------------------------------------------------------- transitions (main timeline)

FX_SEAMS = {"flash", "light-leak", "glitch", "pixelate", "blinds", "cover", "zoom-blur", "doors", "cube"}


def seam_fx(seam, A, B, cut, W, H, uid):
    """Overlay markup (under the captions) and main-timeline tweens for one of the effect seams."""
    a, b = f'"#{A}"', f'"#{B}"'
    fx = f"fx{uid}"
    if seam == "flash":
        markup = f'<div id="{fx}" class="fx-flash" data-layout-allow-occlusion data-layout-ignore></div>'
        js = (f'tl.to({a}, {{filter: "brightness(1.9)", duration: 0.16, ease: "power2.in"}}, {cut - 0.16:.3f});\n'
              f'tl.fromTo("#{fx}", {{opacity: 0}}, {{opacity: 1, duration: 0.14, ease: "power2.in"}}, {cut - 0.14:.3f});\n'
              f'tl.set({a}, {{opacity: 0}}, {cut:.3f});\n'
              f'tl.fromTo({b}, {{scale: 1.08, filter: "brightness(1.6)"}}, {{scale: 1, filter: "brightness(1)", duration: 0.6, ease: "expo.out"}}, {cut:.3f});\n'
              f'tl.to("#{fx}", {{opacity: 0, duration: 0.4, ease: "power2.out"}}, {cut:.3f});\n')
        return markup, js
    if seam == "light-leak":
        markup = f'<div id="{fx}" class="fx-leak" data-layout-allow-occlusion data-layout-ignore style="width:{W * 1.4:.0f}px;height:{H * 1.6:.0f}px"></div>'
        js = (f'tl.fromTo("#{fx}", {{x: {-W * 1.4:.0f}, y: {-H * 0.3:.0f}, opacity: 0}}, {{x: {W * 0.4:.0f}, opacity: 1, duration: 0.7, ease: "power1.inOut"}}, {cut - 0.45:.3f});\n'
              f'tl.to("#{fx}", {{x: {W * 1.1:.0f}, opacity: 0, duration: 0.5, ease: "power1.in"}}, {cut + 0.25:.3f});\n'
              f'tl.to({a}, {{opacity: 0, duration: 0.3, ease: "none"}}, {cut - 0.15:.3f});\n'
              f'tl.fromTo({b}, {{opacity: 0, scale: 1.04}}, {{opacity: 1, scale: 1, duration: 0.6, ease: "power2.out"}}, {cut - 0.1:.3f});\n')
        return markup, js
    if seam == "glitch":
        jit = [(14, 0.0), (-22, 0.033), (9, 0.066), (-30, 0.1), (18, 0.133)]
        js = "".join(f'tl.set({a}, {{x: {dx}, clipPath: "inset({(k * 17) % 60}% 0% {(k * 23) % 40}% 0%)", '
                     f'filter: "drop-shadow({dx // 2}px 0 0 rgba(255,0,90,.85)) drop-shadow({-dx // 2}px 0 0 rgba(0,230,255,.85))"}}, {cut - 0.17 + dt:.3f});\n'
                     for k, (dx, dt) in enumerate(jit))
        js += f'tl.set({a}, {{opacity: 0}}, {cut:.3f});\n'
        js += "".join(f'tl.set({b}, {{opacity: 1, x: {-dx}, clipPath: "inset({(k * 29) % 50}% 0% {(k * 11) % 45}% 0%)", '
                      f'filter: "drop-shadow({-dx // 2}px 0 0 rgba(255,0,90,.85)) drop-shadow({dx // 2}px 0 0 rgba(0,230,255,.85))"}}, {cut + dt:.3f});\n'
                      for k, (dx, dt) in enumerate(jit[:4]))
        js += f'tl.set({b}, {{x: 0, clipPath: "inset(0% 0% 0% 0%)", filter: "none"}}, {cut + 0.14:.3f});\n'
        return "", js
    if seam == "pixelate":
        cols, rows = 16, 9
        tw, th = W / cols, H / rows
        cells = "".join(f'<i style="left:{c * tw:.1f}px;top:{r * th:.1f}px;width:{tw + 1:.1f}px;height:{th + 1:.1f}px"></i>' for r in range(rows) for c in range(cols))
        markup = f'<div id="{fx}" class="fx-tiles" data-layout-allow-occlusion data-layout-ignore>{cells}</div>'
        # A fixed scattered order (37 is coprime with 144): GSAP's own "random" is not seeded.
        cells_n = cols * rows
        js = (f'tl.fromTo("#{fx} i", {{scale: 0}}, {{scale: 1, duration: 0.16, ease: "power2.in", stagger: (i) => ((i * 37 + {uid}) % {cells_n}) * 0.0022}}, {cut - 0.48:.3f});\n'
              f'tl.set({a}, {{opacity: 0}}, {cut:.3f});\n'
              f'tl.to("#{fx} i", {{scale: 0, duration: 0.16, ease: "power2.out", stagger: (i) => ((i * 61 + {uid}) % {cells_n}) * 0.0022}}, {cut + 0.02:.3f});\n')
        return markup, js
    if seam == "blinds":
        n = 9
        bars = "".join(f'<i style="top:{k * H / n:.1f}px;height:{H / n + 1:.1f}px"></i>' for k in range(n))
        markup = f'<div id="{fx}" class="fx-blinds" data-layout-allow-occlusion data-layout-ignore>{bars}</div>'
        js = (f'tl.fromTo("#{fx} i", {{scaleY: 0}}, {{scaleY: 1, duration: 0.24, ease: "power3.in", stagger: 0.025, transformOrigin: "50% 0%"}}, {cut - 0.44:.3f});\n'
              f'tl.set({a}, {{opacity: 0}}, {cut:.3f});\n'
              f'tl.to("#{fx} i", {{scaleY: 0, duration: 0.28, ease: "power3.out", stagger: 0.025, transformOrigin: "50% 100%"}}, {cut + 0.02:.3f});\n')
        return markup, js
    if seam == "cover":
        markup = f'<div id="{fx}" class="fx-cover" data-layout-allow-occlusion data-layout-ignore><div class="fx-cover-a"></div><div class="fx-cover-b"><img src="assets/logo.png" alt=""></div></div>'
        # Off the frame from the start (set by GSAP, never by a CSS transform it would fight).
        js = (f'tl.set("#{fx} .fx-cover-a, #{fx} .fx-cover-b", {{x: {W + 40}}}, 0);\n'
              f'tl.fromTo("#{fx} .fx-cover-a", {{x: {W}}}, {{x: 0, duration: 0.34, ease: "power3.in"}}, {cut - 0.38:.3f});\n'
              f'tl.fromTo("#{fx} .fx-cover-b", {{x: {W}}}, {{x: 0, duration: 0.34, ease: "power3.in"}}, {cut - 0.32:.3f});\n'
              f'tl.set({a}, {{opacity: 0}}, {cut:.3f});\n'
              f'tl.to("#{fx} .fx-cover-b", {{x: {-W}, duration: 0.42, ease: "power3.out"}}, {cut + 0.08:.3f});\n'
              f'tl.to("#{fx} .fx-cover-a", {{x: {-W}, duration: 0.42, ease: "power3.out"}}, {cut + 0.14:.3f});\n'
              f'tl.fromTo({b}, {{x: 160}}, {{x: 0, duration: 0.6, ease: "expo.out"}}, {cut + 0.1:.3f});\n')
        return markup, js
    if seam == "zoom-blur":
        js = (f'tl.to({a}, {{scale: 1.7, filter: "blur(24px)", duration: 0.28, ease: "power3.in"}}, {cut - 0.28:.3f});\n'
              f'tl.to({a}, {{opacity: 0, duration: 0.12, ease: "none"}}, {cut - 0.12:.3f});\n'
              f'tl.fromTo({b}, {{scale: 0.55, opacity: 0.2, filter: "blur(24px)"}}, {{scale: 1, opacity: 1, filter: "blur(0px)", duration: 0.55, ease: "expo.out"}}, {cut:.3f});\n')
        return "", js
    if seam == "doors":
        js = (f'tl.to({a}, {{scale: 0.92, filter: "brightness(0.45)", duration: 0.5, ease: "power2.inOut"}}, {cut - 0.1:.3f});\n'
              f'tl.fromTo({b}, {{clipPath: "inset(0% 50% 0% 50%)"}}, {{clipPath: "inset(0% 0% 0% 0%)", duration: 0.55, ease: "power3.inOut"}}, {cut:.3f});\n'
              f'tl.set({a}, {{opacity: 0}}, {cut + 0.56:.3f});\n')
        return "", js
    if seam == "cube":
        js = (f'tl.to({a}, {{rotationY: -88, x: {-W * 0.5:.0f}, transformPerspective: 2600, transformOrigin: "100% 50%", filter: "brightness(0.5)", duration: 0.5, ease: "power2.inOut"}}, {cut - 0.25:.3f});\n'
              f'tl.fromTo({b}, {{rotationY: 88, x: {W * 0.5:.0f}, transformPerspective: 2600, transformOrigin: "0% 50%", filter: "brightness(0.5)"}}, '
              f'{{rotationY: 0, x: 0, filter: "brightness(1)", duration: 0.5, ease: "power2.inOut"}}, {cut - 0.25:.3f});\n'
              f'tl.set({a}, {{opacity: 0}}, {cut + 0.26:.3f});\n')
        return "", js
    return "", ""


FX_CSS = """
#fx { position: absolute; inset: 0; pointer-events: none; z-index: 40; overflow: hidden; }
.fx-flash { position: absolute; inset: 0; background: #fff; opacity: 0; }
.fx-leak { position: absolute; left: 0; top: 0; opacity: 0; mix-blend-mode: screen; background: radial-gradient(ellipse at 40% 50%, rgba(255,214,120,.95), rgba(255,120,60,.55) 35%, rgba(255,60,140,.25) 55%, transparent 72%); filter: blur(30px); }
.fx-tiles i, .fx-blinds i { position: absolute; display: block; background: var(--ground); transform: scale(0); }
.fx-blinds i { left: 0; width: 100%; transform: scaleY(0); }
.fx-cover { position: absolute; inset: 0; }
.fx-cover-a, .fx-cover-b { position: absolute; inset: 0; }
.fx-cover-a { background: var(--ground2); }
.fx-cover-b { background: var(--accent); display: flex; align-items: center; justify-content: center; }
.fx-cover-b img { width: 180px; height: 180px; filter: brightness(0) saturate(100%); opacity: .85; }
"""


# ---------------------------------------------------------------- the director


ALL_SEAMS_PLAIN = ["curve-left", "curve-up", "zoom-through", "inverse-zoom", "blur-swap", "dip", "whip", "iris", "cut"]


def direct(board):
    """Fills what a storyboard leaves open with variety: each scene's title effect, window entrance
    and transition, from its theme's palette, never the same twice in a row, three to five
    transition kinds over the film. A scene that names its own keeps it."""
    _, theme = theme_of(board)
    titles, seams = theme["titles"], theme["seams"]
    last_title = last_seam = last_enter = None
    used = []
    for i, s in enumerate(board["scenes"]):
        if not s.get("title_style"):
            pick = titles[(i * 3) % len(titles)]
            if pick == last_title:
                pick = titles[(i * 3 + 1) % len(titles)]
            s["title_style"] = pick
        last_title = s["title_style"]
        if i and not s.get("transition"):
            # Payoff cards (stat, steps at the end) arrive bigger: an inverse zoom or a flash.
            if s.get("layout") == "stat":
                pick = "inverse-zoom" if last_seam != "inverse-zoom" else "flash"
            else:
                pick = seams[i % len(seams)]
                if pick == last_seam:
                    pick = seams[(i + 2) % len(seams)]
            s["transition"] = pick
        last_seam = s.get("transition")
        used.append(last_seam)
        if not s.get("enter"):
            pick = ENTERS[(i * 2) % len(ENTERS)]
            if pick == last_enter:
                pick = ENTERS[(i * 2 + 1) % len(ENTERS)]
            s["enter"] = pick
        last_enter = s["enter"]
        if theme.get("brackets") and s.get("callouts"):
            for c in s["callouts"]:
                c.setdefault("style", "brackets")
    return board


LAYOUT_CSS = """
.device { position: absolute; }
.device .bezel { position: absolute; border-radius: 26px; background: linear-gradient(180deg, #2b2f38, #16181d); box-shadow: 0 50px 120px rgba(0,0,0,.6), inset 0 0 0 2px rgba(255,255,255,.08); }
.device .base { position: absolute; height: 26px; border-radius: 0 0 22px 22px; background: linear-gradient(180deg, #3a3f49, #1d2026); box-shadow: 0 20px 40px rgba(0,0,0,.5); }
.device .base::before { content: ""; position: absolute; left: 42%; right: 42%; top: 0; height: 8px; border-radius: 0 0 8px 8px; background: #121418; }
.tile { position: absolute; border-radius: 22px; background: var(--panel); border: 1px solid var(--line); overflow: hidden; }
.tile .t-big { position: absolute; left: 36px; right: 36px; top: 34px; font-size: 54px; font-weight: 600; line-height: 1.08; letter-spacing: -0.02em; }
.tile .t-num { position: absolute; left: 36px; top: 22px; font-size: 120px; font-weight: 600; color: var(--accent-ink, var(--accent)); letter-spacing: -0.04em; }
.tile .t-lab { position: absolute; left: 36px; right: 36px; bottom: 30px; font-size: 32px; color: var(--muted); font-family: "Body", sans-serif; }
.kin { position: absolute; inset: 0; display: flex; align-items: center; justify-content: center; }
.kin-word { position: absolute; font-weight: 600; letter-spacing: -0.04em; white-space: nowrap; }
.kin-final { position: absolute; inset: 0; display: flex; flex-wrap: wrap; align-content: center; justify-content: center; gap: 0 0.25em; padding: 0 8%; text-align: center; font-weight: 600; letter-spacing: -0.03em; line-height: 1.05; opacity: 0; }
.kin-dim { position: absolute; inset: 0; background: rgba(0,0,0,.62); }
.l3 { position: absolute; padding: 22px 34px 24px 30px; border-left: 8px solid var(--accent); background: var(--cap-bg); border-radius: 0 18px 18px 0; }
.l3 h2 { font-size: 54px; font-weight: 600; letter-spacing: -0.02em; }
.l3 p { margin-top: 6px; font-size: 30px; color: var(--muted); font-family: "Body", sans-serif; }
"""
