---
title: Workspaces, tabs and panes
description: How Agentty organizes work — a workspace per project, tabs inside it, and panes inside those.
---

Agentty has three levels. A **workspace** is a project, a **tab** is one screen inside it, and a **pane** is one program: a shell, an AI agent, a file editor or a browser.

## Workspaces

Each row in the sidebar (**⌘B**) is a workspace, with a folder it opens in. **⌘N** creates one.

- Drag rows to reorder them, or drop one onto another to make a **group**. Groups collapse.
- Double-click a name to rename it.
- **⌥⌘↓ / ⌥⌘↑** move between workspaces, **⌘1…⌘9** jump to one.
- Every workspace, its tabs, splits and groups come back after a restart.

A workspace row shows the status of its agents, so you can see which project needs you without opening it.

## Tabs

**⌘T** opens a terminal tab, **⌥⌘C** Claude Code, **⌥⌘X** Codex. The **+** menu has the same, plus the models and any other agent CLI found on your machine.

- **⇧⌘] / ⇧⌘[** or **⌃Tab** move between tabs, **⌃1…⌃9** jump to one.
- **⇧⌘W** closes a tab, **⌘W** closes a pane.
- Double-click empty space in the tab strip for a new tab.

New tabs start in the workspace's folder. **Settings → General** can ask for a folder each time instead.

## Panes

**⌘D** splits right, **⇧⌘D** splits down. **⌘] / ⌘[** move between panes; drag the divider to resize.

Each agent pane has a status bar with the model, context window, rate-limit usage, branch, folder and the local server ports running in that pane. Its position (above or below the terminal) and which items it shows are in **Settings → Appearance**.

Double-click that bar — the status, or the icon it is dragged by — to give the pane the whole tab. Double-click again to put the split back the way it was.

## Side panels

Panels dock beside the terminals and are resized by dragging their edge:

| Panel | Shortcut |
|---|---|
| Files and working trees | ⌥⌘B |
| In-app browser | ⇧⌘B |
| Docker | status bar 🐳 |
| Plugin panels | tab strip button |

## Mini mode

**⌃⌘M** folds the window into a small always-on-top panel at the edge of the screen. Agents that finish pop up as speech bubbles; clicking one brings the full window back.

On macOS the menu bar icon animates while agents work, lists them, and keeps Agentty running after you close the window.
