---
title: Sessions
description: Browse, search and resume the Claude Code and Codex sessions already on your computer, and move a conversation between agents.
---

The agent CLIs write their conversations to your disk. Agentty reads those files, so every session you have ever run is browsable — including the ones you started outside Agentty.

The files are JSONL — one line per event — written by Claude Code and Codex as they work. Agentty parses them to build the list, the search index and the usage numbers. It never writes to them.

## Browsing

**⇧⌘S** opens the session list: everything in `~/.claude` and `~/.codex`, newest first, with title, folder and agent.

- Click a session to open it, or resume it in a new tab.
- **⇧⌘O** searches by title, conversation content or path.
- Star a session to pin it to the top.

## Resuming where you left off

`cd` into a folder with earlier sessions and a bar above the terminal offers to continue them in place. Turn it off in **Settings → General** if you'd rather not see it.

## Moving a conversation between agents

A conversation can be handed from Claude Code to Codex, or back. Agentty writes a context document — what the conversation was about, what was decided, what is left — into `~/.agentty/handoffs/` and starts the other agent with it.

This is a summary, not a transcript replay: the new agent starts fresh with the context, in the same folder.

## Where the files are

| Path | What |
|---|---|
| `~/.claude` | Claude Code's own sessions |
| `~/.codex` | Codex's own sessions |
| `~/.agentty/handoffs/` | Context documents Agentty created |

Agentty only reads the first two. Deleting a session there removes it from the list.
