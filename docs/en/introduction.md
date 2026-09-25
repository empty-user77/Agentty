---
title: Introduction
description: What Agentty is, who it is for, and the ideas behind it — a native terminal built for running several AI coding agents at once.
---

Agentty is a terminal for people who run AI coding agents. It does what a terminal does — shells, tabs, splits, scrollback — and adds the things you start wanting the moment a second agent is working: a place for each project, a status for each agent, a way to see what they changed, and a nudge when one of them needs you.

It is a native application written in Rust, rendered on the GPU. There is no Electron, no web view around the terminal, and no account to create.

## What it is for

Running one agent is easy: you start `claude` in a terminal and watch it. Running five is where terminals stop helping. Tabs all look the same, you lose track of which one is waiting for an answer, two agents edit the same file, and the cost of it all is invisible until the bill arrives.

Agentty is built for that second situation:

- **You always know who needs you.** Every workspace shows whether its agent is working, done, or waiting for input. The pane border lights up when it needs an answer, and a notification reaches you when you are somewhere else.
- **Agents stay out of each other's way.** A second session in the same project starts in its own git worktree on its own branch, so two agents never edit the same files.
- **Your work stays visible.** A GitHub Desktop–style git page, a files panel with every working tree, and a usage dashboard built from the transcripts already on your disk.

## Core ideas

### Workspaces → tabs → splits

A **workspace** is a project — a folder, a sidebar row, and everything you have open for it. Inside a workspace are **tabs**, and a tab can be **split** into panes. A pane is a shell, an AI agent, a file editor or a browser.

Workspaces can be grouped and reordered, and they come back the way you left them after a restart.

### Panes are programs, not wrappers

An agent pane runs the real command line tool — `claude`, `codex`, or another agent you have installed — in a real PTY. Agentty passes it a hook so it can report status, and reads the transcript files it writes anyway. It does not proxy the conversation, rewrite your config files, or sit between the agent and its provider.

That is why an agent behaves in Agentty exactly as it does in your own terminal, and why upgrading the agent CLI does not wait for an Agentty release.

### Everything is local

Agentty has no backend. Session lists, search, usage numbers, cost estimates and handoff documents are all built from files on your own computer. Your conversations, code and credentials never pass through a server of ours, because there is no server of ours.

Secrets you give Agentty go into the operating system's credential store — Keychain on macOS, Credential Manager on Windows, Secret Service on Linux.

> [!NOTE]
> The agent CLIs you run talk to their own providers — Anthropic for Claude Code, OpenAI for Codex — directly from your computer under your own account with them, exactly as in any other terminal. Agentty does not sit in the middle of that: it neither relays nor receives what they send, and that exchange is governed by each provider's own terms and privacy policy. What Agentty itself sends is described in [Telemetry](/docs/telemetry).

## What you get

| Area | What it does |
|---|---|
| **Agent awareness** | Live status per workspace, lit borders for panes that need an answer, notifications, a notification center, jump to the latest unread |
| **Sessions** | Browse and resume `~/.claude` and `~/.codex` sessions, search them by title or content, migrate a conversation from Claude to Codex and back |
| **Session Flow** | Drag a line from one agent to another to share its context; mark it live to forward every turn |
| **Git** | Switch repositories and branches, fetch, pull, push, review diffs, commit selected files, browse history, merge |
| **Working trees** | A worktree per session, a panel that shows every tree, who works in it and what changed |
| **Files & editor** | Project tree, a file editor with syntax colors and formatters, "Open in editor" for VS Code or Cursor |
| **Extensions** | Skills, subagents, commands, plugins and MCP servers for Claude Code and Codex in one place |
| **Monitoring** | Cost, tokens, cache hit rate and models from local transcripts; running AI processes; a capture proxy that lists what your tabs talk to |
| **Plugins** | Panels, buttons, palette commands and `agentty://` links from other apps — see [Plugins](/docs/plugins-overview) |

## Platforms

Agentty is built for macOS, Windows and Linux, and macOS is the reference platform. All three have a download on the releases page. The other two share every feature that does not depend on AppKit or WebKit; the differences are listed in [Platforms](/docs/platforms).

## License

Agentty is free to use, with no account and no subscription. It is open source under the Apache License 2.0, at [github.com/empty-user77/Agentty](https://github.com/empty-user77/Agentty).

## Where to go next

- [Installation](/docs/installation) — download it, or build from source
- [Quick start](/docs/quick-start) — from an empty window to two agents working in parallel
- [Plugins](/docs/plugins-overview) — install one, or write your own
