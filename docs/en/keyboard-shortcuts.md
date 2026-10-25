---
title: Keyboard shortcuts
description: Every default shortcut, and how they translate to Windows and Linux.
---

The full list is also in **Settings → Keyboard shortcuts**, shown in your platform's notation.

## Windows and Linux

Bindings are written once for macOS and translated, so Ctrl+letter always reaches the shell:

| macOS | Windows / Linux |
|---|---|
| ⌘X | Ctrl+Shift+X |
| ⇧⌘X | Ctrl+Alt+Shift+X |
| ⌥⌘X | Ctrl+Alt+X |
| ⌘1…⌘9 | Alt+1…Alt+9 |
| ⌘= / ⌘- / ⌘0 | Ctrl+= / Ctrl+- / Ctrl+0 |
| ⌘-click | Ctrl-click |

Text fields and the file editor use plain Ctrl (Ctrl+C / V / X / A / Z, Ctrl+S, Ctrl+Y, Ctrl+←/→ by word, Ctrl+Home / End).

## Tabs and panes

| Action | Shortcut |
|---|---|
| New terminal / Claude Code / Codex tab | ⌘T / ⌥⌘C / ⌥⌘X |
| New workspace | ⌘N |
| Split right / down | ⌘D / ⇧⌘D |
| Next / previous pane | ⌘] / ⌘[ |
| Next / previous tab | ⇧⌘] / ⇧⌘[, ⌃Tab |
| Next / previous workspace | ⌥⌘↓ / ⌥⌘↑ |
| Close pane / tab | ⌘W / ⇧⌘W |
| Go to workspace / tab 1–9 | ⌘1…⌘9 / ⌃1…⌃9 |

## Panels and pages

| Action | Shortcut |
|---|---|
| Toggle sidebar | ⌘B |
| Workspaces / Local sessions | ⇧⌘E / ⇧⌘S |
| Git / Extensions | ⇧⌘G / ⇧⌘X |
| Session Flow / Monitoring / Settings | ⇧⌘F / ⌥⌘U / ⌘, |
| In-app browser / Files panel | ⇧⌘B / ⌥⌘B |
| Command palette / Jump to unread | ⇧⌘P / ⇧⌘U |
| Voice input (record · ⏎ send · Esc cancel) | ⇧⌘M |
| Search local sessions | ⇧⌘O |
| Go to a file or symbol / Ask AI about the selection (file editor) | ⌘P / ⌘K |
| Go to definition / Find usages / Back (file editor) | F12, ⌘-click / ⇧F12 / ⌃- |
| Go to implementation (file editor) | ⌘F12 |
| Organize imports (file editor) | ⇧⌥O |
| Next / previous problem (file editor) | F8 / ⇧F8 |
| Quick fix (file editor) | ⌘. |
| Rename everywhere (file editor) | F2 |
| Completions (file editor) | ⌃Space |
| Replace in the file (file editor) | ⌥⌘F (↩ one, ⌘↩ all; ⌥⌘C match case, ⌥⌘W whole word, ⌥⌘R regular expression with `$1` for its groups) |
| Comment out lines / move them / duplicate them / delete them (file editor) | ⌘/ / ⌥↑ ⌥↓ / ⇧⌥↑ ⇧⌥↓ / ⇧⌘K |
| Select line / new line below or above / delete to line start / delete word before or after (file editor) | ⌘L / ⌘↩ ⇧⌘↩ / ⌘⌫ / ⌥⌫ ⌥⌦ |
| Indent / outdent lines (file editor) | ⌘] / ⌘[ |
| Add a cursor at the next match / at every match / where you click / on the line above or below (file editor; Esc goes back to one) | ⌘D / ⇧⌘L / ⌥-click / ⌥⌘↑ ⌥⌘↓ |
| Go to the matching bracket (file editor) | ⇧⌘\ |
| Fold / unfold the region at the cursor (file editor; or click the arrow beside a line number, or the `⋯` of a folded line) | ⌥⌘[ / ⌥⌘] |
| Fold / unfold every region of the file (file editor) | ⇧⌥⌘[ / ⇧⌥⌘] |
| Mini mode | ⌃⌘M |

While the file editor has focus, ⌘D, ⌘] / ⌘[, ⇧⌘↩ and ⌥⌘↑ / ⌥⌘↓ work on the text as in VS Code; click a terminal to split, switch or zoom panes or switch workspaces with them.

## Terminal and editor

| Action | Shortcut |
|---|---|
| Find in scrollback | ⌘F |
| Font zoom in / out / reset | ⌘= / ⌘- / ⌘0 |
| Copy / paste / select all / clear | ⌘C / ⌘V / ⌘A / ⌘K |
| Git page: commit / push / fetch / refresh | ⌘↩ / ⌘P / ⇧⌘T / ⌘R |
| File editor: find in file / next / previous | ⌘F / ↩ or ⌘G / ⇧↩ |
| File editor: save / format / close file | ⌘S / ⇧⌥F / ⌘W |
| File editor: Markdown preview on / off (macOS) | ⇧⌘V |
| File editor: Markdown preview beside the text on / off (macOS) | ⌥⌘V |

Double-click the title bar to zoom the window; double-click empty space in the tab strip for a new tab.

## Your own commands

The command palette (**⇧⌘P**) also lists custom entries from `agentty.json` in the project or `~/.agentty/commands.json`. Reserved words work too — typing `claude yolo` in a terminal expands to `claude --dangerously-skip-permissions`.
