---
name: feature-video
description: Make a short intro video for one Agentty feature (e.g. "a remote access intro video", asked in any language) — drive a demo debug build through the feature while recording its window, blur private text, add Microsoft neural-voice narration and subtitles in English, Korean, Japanese and Chinese, between the same branded opening and end card every time. Use when the user asks for a feature video, demo video, promo clip or intro video of the app.
---

# Feature intro videos

One video per feature. Every video has the **same opening and end card** and the same look. Only the footage,
headlines, narration and subtitles change. You record real footage yourself by driving a debug build. You never
use the user's real projects, accounts or data.

Tools in `scripts/feature-video/` (outside `.claude/skills/` because the storyboards and brand strings hold Korean,
Japanese and Chinese text):

| File | What it does |
|---|---|
| `drive.sh` | Starts a demo debug build (`AGENTTY_DEBUG=1`, a throwaway data folder, window 1920x1080). Sends it debug commands, waits for what a pane shows, records its window, and quits it. |
| `video.sh` | Runs `video.py` (makes `~/.cache/agentty-video/venv` with Pillow and edge-tts on first use). |
| `video.py` | `build` a storyboard into the finished videos. `redact` finds private text in footage. `ocr` checks a finished video. `sheet` makes a contact sheet to look at. |
| `ocr.swift` | Vision text recognition with word boxes (used by `redact` and `ocr`). |
| `example.json` | A full storyboard (remote access) to copy from. |

## What comes out

`~/Desktop/Agentty-videos/<slug>/`:
- `<slug>-en.mp4`, `-ko.mp4`, `-ja.mp4`, `-zh.mp4`: subtitles burned in. Headlines, opening title and end card are in that language.
- `<slug>-nosub.mp4`: no subtitles. Upload it to YouTube with the four `.srt` files as caption tracks.
- `<slug>.{en,ko,ja,zh}.srt` and a copy of `storyboard.json`.

All files are 1920x1080 at 30 fps, H.264 and AAC, with loudness normalized to −16 LUFS. Narration is
`en-US-AndrewNeural` in every version by default (`"narration": "en"`). With `"narration": "each"`, each language
is voiced in its own language (`ko-KR-InJoonNeural`, `ja-JP-KeitaNeural`, `zh-CN-YunxiNeural`), and each scene
gets as long as its longest take. Voices come from Microsoft through `edge-tts`. Only the narration lines are sent
to Microsoft, nothing else. The audio is narration only: no music and no sound effects (the user removed them
before).

## The fixed parts: never change them per video

`video.py` holds the brand: the background, the logo's yellow as the accent, the fonts, the opening (logo,
"Agentty", the feature's title and tagline, 4.2 s) and the end card (logo, agentty.run, "Free and open source ·
macOS · Windows · Linux", the GitHub address, 4.6 s, ending on black). A storyboard supplies only `title` and
`tagline`. If the user wants the brand changed, change these constants once, for all future videos, and say so.
`video.sh brand` renders just the opening and end card for a look.

## Steps

### 1. Learn the feature (facts only from the product)
Read `docs/en/<feature>.md` and the code it names. Write down what a viewer must see: the problem, how to turn it
on, the moment it works live, and the payoff. Never claim what the docs don't say.

### 2. Storyboard
Copy `scripts/feature-video/example.json` to `<scratchpad>/video-<slug>/storyboard.json` and write 3–6 scenes, aiming for a body of
30–60 s:
- **`say`**: narration lines. Write one short sentence per line, in plain words that say what the viewer sees and
  why it matters. Give each line all four languages; English fills in for any language left out.
- **`headline`** / **`subline`**: the big caption at the top of a scene. Keep it to 2–5 words.
- **`clip`** + `from` / `to` / `speed` (seconds in the source), or **`image`** + `seconds`.
- Optional:
  - **`crop`** `[x,y,w,h]` (footage pixels)
  - **`zoom`** `{rect, at:[t0,t1], back:[t2,t3]}`: eases into a region and out again. The times are scene seconds after `speed`.
  - **`callouts`** `[{rect, at, label}]`: a glowing ring with a label. Not combined with `zoom`.
  - **`blur`** `[[x,y,w,h]]` or `[{rect, from, to}]` (footage seconds)
  - **`min`**: a minimum length in seconds.
  - **`full`**: footage fills the frame instead of sitting in the window frame.

A scene lasts as long as the longer of its footage and its narration. Short footage holds its last frame.

Show the user the scene list with the English and Korean narration in one short message, and **say that a demo
window is about to open**, before you record.

### 3. Record real footage with the demo build
```sh
D=scripts/feature-video/drive.sh
export VIDEO_DIR=<scratchpad>/video-<slug>      # app.log, data/, footage/ live here
df -h /                                          # under ~20 GB free: clean up first (CLAUDE.md → Build output)
$D demo-project                                  # ~/agentty-demo/taskboard, a harmless git project
$D start            # or: $D start --remote  (remote page on http://127.0.0.1:<port>, no Tailscale)
$D cmd setting language=en                       # English UI in the footage; subtitles do the translating
$D cmd setting appearance=dark
```
Commands are the debug driver's (`debug_command` in `crates/agentty-app/src/workbench/mod.rs`). The most useful are:
- Panes: `workspace claude <dir>`, `launch-split` / `launch-split-down claude <dir>`, `agent claude <dir>`
  (waits until it is ready)
- Input: `prompt "<text>"`, `cmd type '\n'`
- Mouse and keys: `cmd click "x y"`, `cmd key …`, `cmd hover`, `cmd drag-to "x y"`
- Pages and panels: `cmd page flow|git|remote|settings|…`, `cmd panel sessions`, `cmd browser <url>`,
  `cmd browser-viewport iphone-15-pro`
- Remote: `cmd remote on|off|password <throwaway>|forget`
- Waiting: `wait "<regex>"` on the active pane's screen

Feature commands (`git`, `idea`, `docker`, `db`, `link`, `connect-pick`, …) are in the same `match`. Read a
command before you use it.

How to work:
- **Rehearse without recording first.** Take `$D snap <name>` stills and look at them with Read to find
  coordinates. Window coordinates are points from the window's top-left corner.
- **Record one take per scene:** `$D record <scene>` → drive → `$D stop`. That gives
  `footage/<scene>.mov`. Leave about 1 s still before and after: trimming is free, a cut-off action is not. Retake
  freely.
- **Let things happen for real.** Agents really run, pages really load. Speed up the waiting in the storyboard
  (`speed`) instead of faking it.
- **A second device** (e.g. remote access): show the remote page in the in-app browser at phone size
  (`cmd browser http://127.0.0.1:<port>`, `cmd browser-viewport iphone-15-pro`) beside the terminal. Then the phone
  and the Mac are live in one recording.
- **Before recording,** turn on Do Not Disturb so no notification banner lands in the window. The window must stay
  in front and uncovered.
- `screencapture` needs Screen Recording permission for the terminal running you. If the recording comes out
  black or empty, ask the user to allow it in System Settings → Privacy & Security.

### 4. Blur private text (always, before building)
```sh
for f in "$VIDEO_DIR"/footage/*.mov; do scripts/feature-video/video.sh redact "$f"; done
```
`redact` reads every half second of footage. It writes `<footage>.blur.json` with only the place, time and kind of
each hit, never the text. `build` then blurs those places automatically. It catches:
- paths outside `~/agentty-demo` (`/Users/…`, `/private/…`, `~/…`)
- email addresses, tokens, IP addresses, `*.ts.net` names
- this Mac's user name, full name and host name

Add more patterns with `--also '<regex>'`. If something private shows that text recognition cannot read (an
avatar, a QR code, a picture), add it to the scene's `blur` by hand.

### 5. Build and check
```sh
scripts/feature-video/video.sh build "$VIDEO_DIR/storyboard.json"     # all four languages, about 1 min
scripts/feature-video/video.sh sheet ~/Desktop/Agentty-videos/<slug>/<slug>-en.mp4
scripts/feature-video/video.sh ocr   ~/Desktop/Agentty-videos/<slug>/<slug>-en.mp4
```
- **Look** at the contact sheet and at single frames from every scene with Read. Check the following:
  - Every headline and subtitle is readable and on screen.
  - Each zoom lands on the right spot.
  - Each callout ring is around the right element.
  - Nothing shows half-done.
- Then check the `ko`, `ja` and `zh` versions as well.
- `ocr` must report nothing. If it reports something, add `blur` or `crop` and build again.
- You cannot listen to the audio. Check that the narration sits where the subtitles are, using ffmpeg
  `silencedetect` against the `.srt` times, and say in your report that you did not hear the audio.

### 6. Hand over and clean up
- **Report to the user (in their language):**
  - the output folder and each file's length
  - the scene list
  - the narration voice
  - what you checked (frames, OCR, timing)
  - what you could not check (the sound)
- **Clean up:**
  - `$D cmd remote forget` if you set a remote password.
  - `$D quit`.
  - Keep `footage/` until the user is happy with the video: re-cuts need no new recording. Delete it, the demo
    data folder and `~/agentty-demo` once they are.
- **Don't publish anything.** Uploading to YouTube or anywhere else is the user's call.

## Rules
- The demo build uses its own `AGENTTY_DATA_DIR` and the demo project only. Never record the user's real
  workspaces, sessions, accounts, Keychain prompts, browser tabs or other windows.
- Only Agentty's window is recorded (`screencapture -R` with the window's frame). Never record the whole screen.
- Every claim in the narration must be true of the build you recorded. Don't record a feature that isn't merged yet
  unless the user asks for it.
- This file and the scripts stay in English. Narration and subtitles are written in all four languages.
