---
title: Quick start
description: From an empty window to two agents working in parallel — the first ten minutes with Agentty.
---

This page walks through the first session: open a project, start an agent, let a second one work beside it, and find your way around while they run.

Shortcuts are written for macOS. On Windows and Linux, ⌘ becomes Ctrl+Shift, ⇧⌘ becomes Ctrl+Alt+Shift, ⌥⌘ becomes Ctrl+Alt, and ⌘1…9 become Alt+1…9 — see [Keyboard shortcuts](/docs/keyboard-shortcuts).

## 1. Open a project

Press **⌘N** for a new workspace and pick a folder. That folder becomes the workspace's home: every tab you open in it starts there.

The sidebar (**⌘B** toggles it) lists your workspaces. Drag them to reorder, drop one onto another to group them, and double-click a name to rename it. Everything is restored the next time you launch.

> [!TIP]
> Already have a project open in a terminal? Type `cd` into a folder that has earlier Claude Code or Codex sessions and a bar offers to resume them in place.

## 2. Start an agent

Use the **+** menu in the tab strip, or a shortcut:

| Tab | Shortcut |
|---|---|
| Terminal | ⌘T |
| Claude Code | ⌥⌘C |
| Codex | ⌥⌘X |

The menu lists the models you can start with (Opus, Sonnet, Haiku for Claude Code; your recent models for Codex) and any other agent CLIs it found on your machine.

The agent runs as it would in your own terminal — same tool, same configuration, same provider. Type your request and let it work.

## 3. Watch without watching

While the agent runs you do not need to keep the window in front:

- The workspace row in the sidebar shows **working**, **done**, or **waiting for input**.
- A pane whose agent asked you something lights up its border until you click it.
- A notification arrives when an agent finishes or needs an answer. Clicking it opens that pane.
- **⇧⌘U** jumps to the latest unread pane.
- **⌃⌘M** folds the window into a small always-on-top panel at the screen edge. Finished agents pop up as speech bubbles; a click brings the full window back.

The status bar above each agent pane shows the model, context window, rate-limit usage, branch and folder.

## 4. Add a second agent — safely

Open another AI session in the same project from the **+** menu. Agentty starts it in **its own git worktree on a new branch**, taken from the project's default branch, so the two agents never edit the same files.

The **files panel** (**⌥⌘B**) shows the project tree, every working tree, who is working in each one, and what changed there. Right-click a tree to open a terminal in it, copy its path, or remove it once its branch is merged.

You can also ask an agent to split work up itself — "do A, B and C in parallel". It asks Agentty, you confirm once, and each task starts in its own split pane and its own worktree.

> [!NOTE]
> Worktrees come into play only for a *second* session in a project that is already busy — Agentty asks whether to create a new worktree or open the existing one. Working alone keeps you in the folder you opened.

## 5. Arrange the window

| Action | Shortcut |
|---|---|
| Split right / down | ⌘D / ⇧⌘D |
| Next / previous pane | ⌘] / ⌘[ |
| Next / previous tab | ⇧⌘] / ⇧⌘[ |
| Go to workspace / tab 1–9 | ⌘1…⌘9 / ⌃1…⌃9 |
| Close pane / tab | ⌘W / ⇧⌘W |

Every panel docked beside the terminals — files, browser, plugin, Docker — can be resized by dragging its edge.

## 6. Review what they did

Press **⇧⌘G** for the git page: switch repository and branch, fetch, pull, push, review the diff file by file, stage what you want and commit, browse history, and merge branches. Clicking the branch name in any pane header switches branches, pulls, pushes or copies the name.

Click a file in the files panel to open it in the editor next to your terminals — syntax colors, undo/redo, **⌘S** to save, **⇧⌥F** to format with the formatter you have installed. Files an agent changes reload by themselves.

## 7. See what it cost

**⌥⌘U** opens Monitoring: cost, calls, cache hit rate, tokens, models, projects and tools — all computed from the transcript files already on your disk. The same page lists the AI processes that are running, and a capture proxy that shows what your tabs talk to.

## Worth knowing early

- **⇧⌘P** — command palette. Everything is in it, including your own commands from `agentty.json`.
- **⇧⌘O** — search local sessions by title, content or path.
- **⇧⌘B** — an in-app browser next to your terminals. Dev servers appear as `:port` chips and open there as soon as they answer.
- **⇧⌘X** — extensions: skills, subagents, commands, plugins and MCP servers in one place.
- **⌘,** — settings: language (English, 한국어, 日本語, 中文), themes, fonts, and the behavior of everything above.

## Next

- [Workspaces, tabs and panes](/docs/workspaces-tabs-panes) — the layout model in full
- [Agent status](/docs/agent-status) — how Agentty knows what an agent is doing
- [Plugins](/docs/plugins-overview) — add panels, buttons and integrations
- [Settings](/docs/settings) — every preference explained
