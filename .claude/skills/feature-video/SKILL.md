---
name: feature-video
description: Make a short intro, launch, tutorial or teaser video for an Agentty feature (e.g. "a remote access intro video", "a 9:16 short", asked in any language) — drive a demo debug build through the feature while recording its window and logging every click, blur private text, then build it with HyperFrames (HTML + GSAP rendered by headless Chrome) — a camera that eases into the action, an oversized cursor that causes each click, transitions with a cause, word-by-word captions — with Microsoft neural-voice narration in English, Korean, Japanese and Chinese, between the same branded opening and end card every time. Use when the user asks for a feature video, demo video, promo clip, short, teaser or intro video of the app.
---

# Feature videos

Every video has the **same opening and end card** and the same look. Only the footage, the scenes, the narration
and the captions change. You record real footage yourself by driving a debug build. You never use the user's real
projects, accounts or data.

Tools in `scripts/feature-video/` (outside `.claude/skills/` because the storyboards and brand strings hold Korean,
Japanese and Chinese text):

| File | What it does |
|---|---|
| `drive.sh` | Starts a demo debug build (`AGENTTY_DEBUG=1`, a throwaway data folder, window 1920x1080 or the largest 16:9 that fits). Sends it debug commands, waits for what a pane shows, records its window, logs every click and mark while recording, and quits it. |
| `events.py`, `park.swift` | Used by `drive.sh`: the events file of a recording, and parking the real pointer outside the window. |
| `video.sh` | Runs `video.py` (makes `~/.cache/agentty-video/venv` with Pillow and edge-tts on first use). |
| `video.py film` | **The build.** A storyboard into one HyperFrames project per language (`film.py`), checked with `hyperframes check`, rendered and loudness-normalized. |
| `video.py review` | Sheets of every scene and every seam of a built video, and a narration check. |
| `video.py verify` (`verify.py`) | Reads back every caption, headline, label, callout, click target, stat and quote; narration and translations; `--self-test` plants errors to prove it catches them. See the `video-verify` skill. |
| `video.py redact` / `ocr` / `sheet` / `cut` | Private text in footage → blur map; private text left in a finished video; a contact sheet; joining pieces of a recording. |
| `video.py build` | The older ffmpeg-only build (no camera, cursor or transitions). Use it only when HyperFrames cannot run. |
| `motion.py` | The motion-graphics library: themes and moving grounds, title effects, window entrances, drawn callouts, effect transitions, the extra layouts, and the director that varies them. |
| `examples/*.json` | Storyboard forms to start from: `launch`, `tutorial`, `before-after`, `teaser-vertical`, `showcase` (every layout and theme feature in one film). `example.json` is an older plain storyboard. |

HyperFrames (Apache-2.0, by HeyGen) is run as `npx hyperframes@<pinned version>` (`HF_VERSION` in `film.py`). It needs
Node.js 22+, FFmpeg and Chrome (it downloads its own headless shell on first use). Telemetry is off in every call
(`HYPERFRAMES_NO_TELEMETRY=1`, `DO_NOT_TRACK=1`). Nothing of the footage leaves the machine; GSAP is loaded from
jsDelivr and fonts are the Mac's own.

## What comes out

`~/Desktop/Agentty-videos/<slug>/`:
- `<slug>-en.mp4`, `-ko.mp4`, `-ja.mp4`, `-zh.mp4`: captions burned in. Headlines, opening title and end card are in that language.
- `<slug>-nosub.mp4`: no captions. Upload it to YouTube with the four `.srt` files as caption tracks.
- `<slug>.{en,ko,ja,zh}.srt`, a copy of `storyboard.json`, and `timeline.json` (where each scene and line sits; `review` reads it).

1920x1080 (or 1080x1920 for `"aspect": "9:16"`, 1080x1080 for `"1:1"`) at 30 fps, H.264 and AAC, loudness
−16 LUFS. Narration is `en-US-AndrewNeural` in every version by default (`"narration": "en"`). With
`"narration": "each"`, each language is voiced in its own language (`ko-KR-InJoonNeural`, `ja-JP-KeitaNeural`,
`zh-CN-YunxiNeural`) and its captions light word by word as they are said. Voices come from Microsoft through
`edge-tts`; only the narration lines are sent. The audio is narration only: no music and no sound effects (the user
removed them before).

The HyperFrames projects stay in `film/` beside the storyboard (`--project` to move them). To look or hand-edit:
`npx hyperframes@<version> preview` in one of them. An edit there is lost on the next `film`: put lasting changes in
the storyboard or in `film.py`.

## The fixed parts: never change them per video

The brand lives in `video.py` (background colours, the logo's yellow as the accent, `SITE`, `OUTRO_LINE`,
`REPO_URL`) and `film.py` (`brand_html`, the opening of 4.2 s and the end card of 4.6 s ending on black, the faces in
`FACES`). A storyboard supplies only `title` and `tagline`. If the user wants the brand changed, change it there
once, for all future videos, and say so.

## Steps

### 1. Learn the feature (facts only from the product)
Read `docs/en/<feature>.md` and the code it names. Write down what a viewer must see: the problem, how to turn it
on, the moment it works live, and the payoff. Never claim what the docs don't say.

### 2. Pick a form, then write the storyboard
Copy the closest form from `examples/` to `<scratchpad>/video-<slug>/storyboard.json`:

| Form | Shape | Length |
|---|---|---|
| `launch` | hook (a title card naming the pain) → the old way → turn it on → watch it work → a real number → what you get | 45–75 s |
| `tutorial` | the steps as a checklist → one held, spotlit framing per step | 40–90 s |
| `before-after` | the two ways side by side (`split`) → the new way alone → a real quote | 30–50 s |
| `teaser-vertical` | `9:16`: claim → one proof → one number | 15–30 s |

For a team or multi-agent film, mix them: a `title` hook, `window` scenes of the agents at work, a `quote` of what
an agent really wrote, a `split` of terminals and editor, a `stat`, a `steps` close.

**Pick a look first: `"theme"`** (motion graphics live in `motion.py`). Each theme brings its own colours, a ground
that moves slowly under the whole film, a window style, and the title effects and transitions the director picks
from:

| `theme` | Look | Suits |
|---|---|---|
| `midnight` (default) | dark navy, a drifting glow, a light pass over each window | product demos, the house look |
| `aurora` | deep blue with drifting colour blobs, glowing windows | launches, big announcements |
| `neon` | near-black, a yellow-lit grid floor, glow on titles, glitch and pixel cuts | short, punchy teasers |
| `editorial` | cream paper, a giant faint wordmark, ink type on yellow markers, cover and blind cuts | explainers, calm tutorials |
| `blueprint` | blue grid paper, corner brackets that close on each window, scramble titles | technical features, CLI, data |

`"chapters": true` puts a `02 / 08` counter in the corner. Mark the words to stress with stars —
`"Built in *twenty minutes*"` — they take the accent colour (and a marker sweep with the `highlight` title effect).

**The director** fills what a scene leaves open: its title effect, how its window enters and its transition, from
the theme's palette, never the same twice in a row. Name `title_style`, `enter` or `transition` on a scene to choose
yourself. The storyboard saved with the video shows what was picked.

Each scene has one `layout`:

| `layout` | Shows | Keys |
|---|---|---|
| `window` (default) | footage in a framed window under a headline | `clip`, `from`/`to`/`speed` or `pieces`, `zoom`, `reveal`, `cursor`, `callouts`, `spotlight`, `crop`, `blur` |
| `full` | footage filling the frame | as `window` |
| `device` | footage as the screen of a laptop that rises into place | as `window` |
| `hero3d` | the window swings in from a 3D tilt and settles flat | as `window` |
| `lower3rd` | full-bleed footage with the headline as a lower third that wipes in and out | as `window` |
| `bento` | the footage in a big tile, the headline and a number in tiles beside it | as `window`, `value`, `label` |
| `split` | two clips side by side (stacked on 9:16) | `clips: [{clip, from, to, speed, zoom}, …]`, `labels` |
| `pip` | a main clip and a small inset | `clips` (main first), `labels` |
| `mosaic` | up to four clips assemble as tiles, then the camera pushes into one | `clips`, `labels`, `focus` (tile index), `focus_at` |
| `kinetic` | the words slam in one at a time, then the line locks in the middle (a clip may play dimmed behind) | `headline`, optional `clip` |
| `title` | a big statement | `headline`, `subline` |
| `stat` | a number that counts up | `value` (`"23"`, `"3×"`, `"1.5s"`), `label`, `subline` |
| `steps` | a checklist that ticks in | `items`, optional `beats` (seconds) |
| `quote` | a line someone or an agent really wrote | `quote`, `by` |

Mix layouts: a film of only `window` scenes looks like slides. A good 60–90 s film uses five or more of them, opens
on `kinetic` or `title`, puts the proof in footage layouts, and lands on `stat`, `bento` or `steps`.

Common keys:
- **`say`**: narration lines, one short plain sentence each, in all four languages (English fills gaps).
- **`headline`** / **`subline`**: 2–5 words / one short line. `title_style` picks the entrance: `mask-up` (words
  rise out of a mask), `slam` (lands big with a three-frame shake), `rise`, `tracking` (words close in from apart),
  `scramble` (characters decode), `glitch`, `highlight` (a marker sweeps the `*starred*` words), `words-blur`,
  `words-ink`, `line-blur`, `letters`.
- **`enter`**: how a footage window arrives — `rise`, `tilt`, `swing`, `scale`, `wipe`. `sweep` / `brackets` /
  `ripple` (`true`/`false`) turn the light pass, the corner brackets and the click rings on or off.
- **`reveal`** `[x,y,w,h]` (footage pixels): open tight on that detail, then pull back to the whole window
  (`reveal_at`, default 1 s).
- **Times** are footage seconds: a number, or a mark the driver logged — `"@result"`, `"@result+1.5"`,
  `"@result-1"`. Prefer marks: they stay right when a take is re-recorded.
- **`pieces`**: `[[from, to, speed], …]` skips the waiting inside one scene.
- **`zoom`**: `"auto"` eases into each burst of clicks the driver logged and back out (`zoom_depth`, default 1.6),
  travelling between bursts that come close together; or `{rect, at, back}` (footage pixels, scene seconds), or a
  list of them.
- **`cursor`**: on by default when the footage has clicks: an oversized pointer enters from below, reaches each
  click just before it happens, taps, and leaves. `false` turns it off.
- **`callouts`** `[{rect, at, label, expect, style}]`: marks a rect and pins the label under it. `style`: the
  default dims everything else; `circle` draws a hand-drawn ring around it; `brackets` closes corner brackets on
  it; `arrow` draws a curved arrow into it. `expect` is the text that must be inside the rect (checked by `verify`).
- **`source`** on a `stat` or `quote`: `{clip, at, text}`, the recording's frame the number or line comes from.
- **`transition`**: how this scene enters. Use 3–5 kinds in a film, each for a reason:

| `transition` | Use for |
|---|---|
| `curve-left` (default) | the next beat — a velocity-matched cut that carries the eye left |
| `curve-up` | a conclusion rising above what came before |
| `zoom-through` | going deeper into the same thought (the camera pushes through) |
| `inverse-zoom` | an arrival or payoff: something bigger lands |
| `blur-swap` | a quiet change on the same ground |
| `dip` | a hard cut you want read as deliberate |
| `whip` | a fast jump to another place |
| `iris` | opening the next scene from the last click of the one before |
| `cut` | inside one thought only |
| `zoom-blur` | a hard push through the frame into the next place |
| `flash` | a bright reveal: the frame burns to white and opens on the next |
| `light-leak` | a warm, soft change of place (a film light leak sweeps across) |
| `doors` | the next scene opens from the middle while the last one falls back |
| `cube` | a turn to another side of the same subject |
| `cover` | a yellow panel with the logo wipes across (chapter breaks) |
| `blinds` | stripes close and open (editorial, lists) |
| `pixelate` | tiles fill and clear in a scattered order (technical) |
| `glitch` | a jittering RGB split (neon teasers; use once) |

- **`min`** a minimum length; **`blur`** `[[x,y,w,h]]` or `[{rect, from, to}]` extra regions to hide.

A scene lasts as long as the longest of its footage, its narration and the time its card needs to be read
(0.5 s + 0.3 s a word). Short footage holds its last frame.

Show the user the scene list with the English and Korean narration in one short message, and **say that a demo
window is about to open**, before you record.

### 3. Record real footage with the demo build
```sh
D=scripts/feature-video/drive.sh
export VIDEO_DIR=<scratchpad>/video-<slug>      # app.log, data/, footage/ live here
df -h /                                          # under ~20 GB free: clean up first (CLAUDE.md → Build output)
$D demo-project                                  # ~/agentty-demo/taskboard, a harmless git project
$D start            # or: $D start --remote  (remote page on http://127.0.0.1:<port>, no Tailscale)
                    #     $D start --release (plugins: a debug build runs WebAssembly far too slowly)
$D cmd setting language=en                       # English UI in the footage; captions do the translating
$D cmd setting appearance=dark
```
Commands are the debug driver's (`debug_command` in `crates/agentty-app/src/workbench/mod.rs`). The most useful are:
- Panes: `workspace claude <dir>`, `launch-split` / `launch-split-down claude <dir>`, `agent claude <dir>`
  (waits until it is ready). Several agents in one folder: `cmd setting auto_worktree=false` first.
- Input: `prompt "<text>"`, `cmd type '\n'`; in the file editor `cmd editor "type <text>"`, `cmd editor "select 3:1-5:10"`,
  `cmd editor "ask <instruction>"`, `cmd editor follow`, `cmd edit <file>`.
- Mouse and keys: `cmd click "x y [double|right]"`, `cmd key …`, `cmd hover "x y"`, `cmd drag-to "x y"`
- Pages and panels: `cmd page flow|git|remote|settings|…`, `cmd panel sessions`, `cmd browser <url>`,
  `cmd browser-viewport iphone-15-pro`
- State: `cmd state` prints every pane's status (busy, waiting for a permission) to `app.log`.
- Waiting: `wait "<regex>"` on the active pane's screen.

How to record well (the capture rules):
- **Rehearse without recording first.** Take `$D snap <name>` stills and read them to find coordinates (window
  points from the window's top-left corner). Panels the agents open (the in-app browser) move every coordinate:
  snap again after anything changes the layout.
- **One take per beat:** `$D record <scene>` → drive → `$D stop`. Leave about 1 s still before and after. Retake
  freely; a re-take keeps its marks, so the storyboard stays right.
- **Mark the moments** you will cut on: `$D mark result` right when the result appears. `stop` writes every click,
  key and mark to `footage/<scene>.mov.events.json` in footage seconds, corrected for screencapture's start delay.
  The cursor, the auto zoom and `@mark` times all come from it: **click through the driver** (`cmd click`), not by
  hand, or the film cannot see the click.
- **The real pointer is parked** under the window when recording starts; don't move the mouse while recording.
- **Let things happen for real.** Agents really run, pages really load. Speed up the waiting with `speed` or
  `pieces` instead of faking it. A long multi-agent session can be one long take with marks, cut into scenes later.
- **A second device**: show the remote page in the in-app browser at phone size beside the terminal.
- **Before recording,** turn on Do Not Disturb so no notification banner lands in the window. The window must stay
  in front and uncovered. Agents that ask for permissions: answer through the driver, or start them in a mode that
  needs none for the demo folder only (a `.claude/settings.json` in the demo project).
- `screencapture` needs Screen Recording permission for the terminal running you. If the recording comes out
  black or empty, ask the user to allow it in System Settings → Privacy & Security.

### 4. Blur private text (always, before building)
```sh
for f in "$VIDEO_DIR"/footage/*.mov; do scripts/feature-video/video.sh redact "$f"; done
```
`redact` reads every half second of footage. It writes `<footage>.blur.json` with only the place, time and kind of
each hit, never the text. `film` bakes those blurs into the clips it prepares, before HyperFrames sees them. It
catches paths outside `~/agentty-demo`, email addresses, tokens, IP addresses, `*.ts.net` names, and this Mac's user
name, full name and host name. A demo character who shares the user's first name gets blurred too:
pick a different name for the human in a demo. Add patterns with `--also '<regex>'`; add anything recognition
cannot read (an avatar, a QR code) to the scene's `blur` by hand.

### 5. Build, check, verify, review
```sh
scripts/feature-video/video.sh film "$VIDEO_DIR/storyboard.json" --no-render --langs en   # write + check fast
scripts/feature-video/video.sh film "$VIDEO_DIR/storyboard.json"                          # all languages
scripts/feature-video/video.sh verify ~/Desktop/Agentty-videos/<slug>/<slug>-en.mp4      # every language
scripts/feature-video/video.sh review ~/Desktop/Agentty-videos/<slug>/<slug>-en.mp4
scripts/feature-video/video.sh ocr    ~/Desktop/Agentty-videos/<slug>/<slug>-en.mp4
```
- `verify` reads back every caption, headline, label, callout, click target, stat and quote, and checks narration
  and translations: follow the **`video-verify` skill** (it also covers `--self-test`, and how to tell a fault of
  the video from a limit of recognition). Record clicks with `drive.sh tap "<x y>" "<label>"` and give callouts an
  `expect` so there is more to check.
- `film` stops on any `hyperframes check` error and prints its warnings. Weigh each warning: `line_unread` (a card
  too short to read), `dead_stretch` (3 s+ with nothing changing), `text_below_floor`, `first_frame_empty` are real;
  footage overflowing its window is the camera at work.
- `review` writes `<video>.scenes.png` (each scene just after its start, its middle and its end) and
  `<video>.seams.png` (every transition at −0.2, −0.07, +0.07 and +0.2 s), and checks that speech starts where each
  caption says (silence detection against `timeline.json`).
- **Look at both sheets with Read**, as a harsh first-time viewer, and write each problem as one line —
  `t=4.2s · where · problem → change` — then fix and rebuild. What to look for:
  - a frame just after a seam that is empty or half-built (entrances must start on the cut);
  - a headline or caption cut off, wrapped badly, or too small to read on a phone;
  - a zoom that lands beside the action, or stays in when the scene ends;
  - the cursor somewhere the click is not (the events are off: check the `lag` in the events file);
  - the same entrance or transition twice in a row; a crossfade between unrelated places;
  - anything private (also `ocr` must report nothing; else add `blur` and build again).
- Check the `ko`, `ja` and `zh` versions too (line breaks, fonts).
- You cannot listen to the audio. Say in your report that `review` measured where speech starts but nobody heard it.

### 6. Hand over and clean up
- **Report to the user (in their language):** the output folder and each file's length, the scene list (form,
  layouts, transitions), the narration voice, what you checked (sheets, seams, OCR, narration timing), and what you
  could not check (the sound).
- **Clean up:** `$D cmd remote forget` if you set a remote password; `$D quit`. Keep `footage/` and `film/` until the
  user is happy: re-cuts need no new recording. Delete them, the demo data folder and `~/agentty-demo` once they
  are.
- **Don't publish anything.** Uploading to YouTube or anywhere else is the user's call.

## Motion rules `film.py` follows (keep them when you change it)

Taken from the HyperFrames launch-film doctrine:
- One move is one tween: a fast move 0.3–0.6 s; arrivals on a long ease-out (`expo.out`, `power4.out`), camera moves
  ease both ways (`power2.inOut`), exits accelerate (`power3.in`) and are shorter than entrances.
- Seams cut mid-motion with the same direction and speed on both sides; the first frame after a cut already shows the
  scene. Never a plain crossfade between two places.
- A landed title holds still; the motion moves to the next element, the camera or the footage. No breathing,
  floating or pulsing loops.
- The cursor is big (about 6% of the stage), enters from off-screen, and every click causes the next thing on the
  same frame.
- Captions are an overlay at the bottom, one group at a time; the picture keeps the full frame.
- Text stays at least 2.6% of the short side; a line holds 0.5 s plus 0.3 s a word.

## Rules
- The demo build uses its own `AGENTTY_DATA_DIR` and the demo project only. Never record the user's real
  workspaces, sessions, accounts, Keychain prompts, browser tabs or other windows.
- Only Agentty's window is recorded (`screencapture -R` with the window's frame). Never record the whole screen.
- Every claim in the narration must be true of the build you recorded; every number on a `stat` card and every
  `quote` comes from the recording. Don't record a feature that isn't merged yet unless the user asks for it.
- This file and the scripts stay in English. Narration and captions are written in all four languages.
