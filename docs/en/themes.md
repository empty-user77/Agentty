---
title: Themes and fonts
description: Colors, fonts and the small appearance settings — including importing iTerm2 color schemes.
---

**Settings → Appearance** holds all of it.

## Themes

Agentty ships with several themes. You can also use any **iTerm2 color preset**: choose **Import .itermcolors…**, or drop `.itermcolors` files into `~/.agentty/themes/` and pick them from the list. Thousands are available at [iterm2colorschemes.com](https://iterm2colorschemes.com/).

An imported theme is referenced by its file name without the extension.

## Fonts

JetBrains Mono is bundled, along with a Nerd Font–patched version, so Powerlevel10k and Starship prompts render without installing anything. Any monospace font on your system can be used instead.

Font size, line height, padding, letter spacing (0–8 points of extra width per cell) and bold text are set on the same page. **⌘= / ⌘- / ⌘0** zoom the font in the current pane.

## Colours of your own

Background, text, cursor and selection can each be replaced without leaving the theme — the rest of it stays as it is, and **Theme's own** puts a colour back. The preview under the settings shows every change at once.

## Cursor and scrollback

Cursor shape (block, beam, underline) and blinking, and how many lines of scrollback each terminal keeps.

## The agent status bar

Which items appear above an AI pane, and in what order: model, context window, usage, status, elapsed time, links, ports, worktree, branch, folder. Model, context, status and branch cannot be hidden.

Its position — above the terminal or below it — is on the same page.
