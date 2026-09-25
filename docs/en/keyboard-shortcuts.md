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
| Search local sessions | ⇧⌘O |
| Mini mode | ⌃⌘M |

## Terminal and editor

| Action | Shortcut |
|---|---|
| Find in scrollback | ⌘F |
| Font zoom in / out / reset | ⌘= / ⌘- / ⌘0 |
| Copy / paste / select all / clear | ⌘C / ⌘V / ⌘A / ⌘K |
| Git page: commit / push / fetch / refresh | ⌘↩ / ⌘P / ⇧⌘T / ⌘R |
| File editor: save / format / close file | ⌘S / ⇧⌥F / ⌘W |

Double-click the title bar to zoom the window; double-click empty space in the tab strip for a new tab.

## Your own commands

The command palette (**⇧⌘P**) also lists custom entries from `agentty.json` in the project or `~/.agentty/commands.json`. Reserved words work too — typing `claude yolo` in a terminal expands to `claude --dangerously-skip-permissions`.
