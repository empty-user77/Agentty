---
name: video-verify
description: Test a finished Agentty feature video (built with `video.sh film`) for correctness — read back every caption, headline, subline, label, callout, stat, step and quote where and when it should show, check that each click the cursor makes lands on the control it names and each callout points at the text it describes, that the narration sits under its captions, that every stat or quote comes from the recording, that no translation is missing, and that the checker itself still catches planted errors. Use after building a feature video, before handing it over, when the user asks whether labels, captions or callouts are right, or when changing film.py or verify.py.
---

# Verify a feature video

A video can look fine on a contact sheet and still be wrong: a caption a line behind the voice, a callout ring
around the panel next to the one the label names, a click that lands on "New" while the narration says "Hot", a
stat card whose number is not in the recording, a Korean video showing an English line because a translation was
left out. This skill finds those with measurements, and keeps a person's eye for what measurements cannot judge.

Everything runs from `scripts/feature-video/` (see the `feature-video` skill for building).

## 1. Make the video checkable

`video.sh film` writes `<slug>.<lang>.checks.json` beside each video: every text the viewer should read, its place
on the canvas and the moment it should show, and every click with where the cursor puts it. Give it more to check
in the storyboard and the recording:

- **Name your clicks.** Record clicks with `drive.sh tap "<x y>" "<label>"` instead of `cmd click`: the label is
  the text on the control ("Format", "Hot"). `verify` reads that text under the film's cursor.
- **Say what a callout points at.** `"callouts": [{"rect": …, "at": …, "label": …, "expect": "<text inside the
  rect>"}]`: the text that must be inside the highlighted area when it shows (a function name, a button's
  caption). The same `expect` on a `zoom` rect checks where the camera lands.
- **Give each stat and quote its source.** `"source": {"clip": "footage/x.mov", "at": "@mark", "text": "23 pass"}`:
  the recording's frame where that number or line appears. `verify` reads that frame.

## 2. Run the checks, for every language

```sh
V=~/Desktop/Agentty-videos/<slug>
scripts/feature-video/video.sh verify $V/<slug>-en.mp4            # and -ko, -ja, -zh
scripts/feature-video/video.sh verify $V/<slug>-ko.mp4 --self-test
```

`verify` crops each place at its moment, reads it with Vision (the video's language first), and compares:

| Check | Passes when |
|---|---|
| `caption` | the group's words are in the caption band at two moments of its time, and gone 0.12 s after it ends |
| `headline`, `subline`, `title`, `label`, `step`, `quote`, `quote-by`, `stat-label` | the text is in its area while the scene holds |
| `stat` | the exact number is on the card (no tolerance) |
| `click` | the point is inside the window on the canvas, and the named control's text is under the cursor tip |
| `spot-label`, `callout` | the label is next to its rect |
| `spot-target`, `zoom-target` | the `expect` text is inside the highlighted rect / the zoomed view |
| `source` | the stat or quote text is in the recording's frame |
| `intro-title`, `intro-tagline`, `outro-site` | the opening and end card say what they should |

Then, without frames:
- **AUDIO**: speech is heard under each caption group (silence detection), and no half second of speech goes
  without a caption.
- **TEXT**: every storyboard text has this language (English fills a gap, and that shows in a Korean video).

Text is compared word by word (Korean, English) or character by character (Japanese, Chinese), with room for a
misread letter but not for a different word: "Six agents" fails for "Five agents", "32" fails for "23".

A failure reads `FAIL t=… · kind · scene · expected "…" · read "…" (score) · why`. The crops that failed are in
`<video>.verify.png`: **look at it with Read** before deciding. Exit code 1 means a failure; `--strict` also fails
on AUDIO and TEXT lines. The full result is in `<video>.verify.json`.

## 3. Prove the checker still works: `--self-test`

`--self-test` plants errors into a copy of the checks — a headline with other words, a caption 4 s late, a click
named for another control, a label moved away, a wrong number, a step and a quote that are not on screen — and
confirms each is caught, and that the real checks pass. Run it once per language after any change to `film.py`,
`verify.py` or `ocr.swift`, and on every new video before trusting a clean result: a checker that passes
everything has to show that it can fail.

## 4. Decide on each failure

A failure is either the video's fault or the checker's. Decide from the crop, never from the score alone:

| What the crop shows | It is | Do |
|---|---|---|
| The text is wrong, missing, late, cut off, or covered | the video | fix the storyboard or `film.py`, build, verify again |
| The callout ring or the cursor is beside the thing it names | the video (a wrong `rect`, events off, the layout changed between takes) | fix the rect from a still of that exact moment (`drive.sh snap`, or a frame of the clip at that time) |
| The text is there and readable, recognition missed it | the checker | make the video easier to read first (contrast, size, a calmer background); change `verify.py` only when the text is plainly legible, and then run `--self-test` |

Never loosen a threshold to make one video pass. Never edit `checks.json` by hand to pass.

Seen so far, all real: a callout drawn from a still taken after the layout changed (the in-app browser had
closed); captions grey over a white web page (the caption box is now more opaque); a tiny caption scrap
("テスト。") flashing by (scraps now join the part before them).

Recognition limits to know: it reads nothing in a very wide strip (crops are padded to 4:1), it misses Japanese and
Chinese next to a lot of Latin code (caption crops are narrowed to the caption), and it reads "AI" as "Al" (folded).

## 5. What a person still checks

Measurements cannot tell whether the film makes sense. After `verify` passes, look at the `review` sheets
(`video.sh review`) or hand them to a fresh sub-agent with the storyboard only, and ask for P0/P1 notes:
- Does each scene show what its narration says, in that order?
- Is the thing being talked about visible and big enough when it is talked about?
- Does any transition jump, or any frame just after a cut look empty?
- Would a first-time viewer know what the highlighted area is?

## Report

Per language: checks passed of total, every failure with its decision (fixed / checker limit and why), AUDIO and
TEXT lines, and the self-test result. Say plainly what was measured and what was only looked at; the sound itself
was not heard.
