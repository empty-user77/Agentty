---
title: Agent status
description: How Agentty knows what each agent is doing, and how it tells you.
---

Agentty watches the agents you start and shows, for each one, whether it is working, finished, or waiting for you.

## Where status appears

- **Sidebar** — each workspace row carries the status of its agents.
- **Pane border** — lights up when that agent asked a question or needs permission, until you click the pane.
- **Status bar** above each agent pane — model, context window, rate-limit usage, branch, folder.
- **Notifications** — when an agent finishes or needs an answer. Clicking one opens that pane.
- **⇧⌘U** — jump to the most recent unread pane.
- **Mini mode and the menu bar** — status without the window in front.

## The context window

The status bar's context figure is a meter: click it to open **Context memory**, which breaks the window down into system and tools, memory files, skills, the summary and the conversation, and says which files have been read or edited since the last compaction.

Beside that figure the panel carries a **compact** button. It runs the agent's own compaction command (Claude Code and Codex), whatever the number says, so you can make room before a long turn instead of after one. The status bar grows its own compact button from 70% — the point where the meter turns orange.

## The states

| State | Meaning |
|---|---|
| Working | A tool is running |
| Thinking | A turn is open, between tool calls |
| Waiting for input | The agent asked a question or for permission |
| Finished | The turn ended |
| Idle | Nothing running |

## How it works

Agentty passes the agent a status hook on the command line when the pane starts. The agent reports to a private local socket that only your user account can reach. Nothing is written into your projects or into the agent's own configuration files.

## Notifications

**Settings → Notifications** controls them:

- System notifications when an agent finishes or needs input.
- Whether to notify while Agentty is the front application.
- Questions and permission requests always notify, unless you are looking at that pane.

### To your phone

The same page connects **Slack**, **Discord** or **Telegram**, so a question reaches you when you are away from the machine. You choose whether a message is sent only when an agent needs you, or when one finishes as well, and whether to include what the agent asked.

Slack and Discord take either form: a webhook URL, or a bot token together with the channel to write to — Slack accepts `#general`, `general` or a channel id, Discord takes a channel id. Telegram is always a bot, with the chat to write to.

Webhook URLs and the bot token are stored in the operating system's credential store, never in a settings file.
