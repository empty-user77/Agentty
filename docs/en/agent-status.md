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

The window's size is the one the agent itself reports — Claude Code through its status line, Codex in its session log — never a guess from the model's name. Until it has reported one, the meter shows the tokens in use without a percentage.

Beside that figure the panel carries a **compact** button. It runs the agent's own compaction command (Claude Code and Codex), whatever the number says, so you can make room before a long turn instead of after one. The status bar grows its own compact button from 70% — the point where the meter turns orange.

## The states

| State | Meaning |
|---|---|
| Working | A tool is running |
| Thinking | A turn is open, between tool calls |
| Waiting for input | The agent asked a question or for permission |
| Finished | The turn ended |
| Idle | Nothing running |

## Subagents

While an agent has subagents, its pane's header shows how many are running out of how many (`1/3`). Click that to open the **Subagents** window. A ⌘-click on a subagent listed on a workspace card in the sidebar opens it too, and so does a subagent's card in the Orchestration page's **Workflow**, which opens it at that subagent. The window moves by its title bar and resizes by its corner; **Open in its own window** takes it out of the Agentty window.

The window has two views of the same subagents:

- **Office** — each running subagent sits at a desk and says what it is doing in a speech bubble; the session's own agent works at the console at the bottom. Finished ones stand in the **Break room** with the first line of their report. **Send home** takes one out of the view, **Send everyone home** all of them; their transcripts stay. More subagents than a floor holds go on more floors.
- **Timeline** — a bar for each subagent on one time axis, coloured by the kind of tool running, beside a list with each one's current step.

Click a subagent to read its log: each tool call as a short sentence ("Reading app.ts"), and its task, remarks and report formatted from their Markdown. **Stop this subagent** ends one running subagent at its next tool call; the session and its other subagents go on.

### Scheduled work

While a Claude Code session has something scheduled, its pane's header shows a clock with a count: the session's cron jobs (what `/loop 5m` makes) and the next turn of a self-paced `/loop`. Click it for the list — each job's prompt, whether it **Repeats**, runs **Once** or is a **Loop**, and when it runs next (a job only runs while the session is idle, so one past its time says "when idle"). The bin icon cancels one: Agentty asks the session to delete it, which it does on its next turn, so answer a waiting question first. Codex panes have no clock; Codex's scheduled tasks belong to its desktop app.

## How it works

Agentty passes the agent a status hook on the command line when the pane starts. The agent reports to a private local socket that only your user account can reach. Nothing is written into your projects or into the agent's own configuration files.

## Notifications

**Settings → Notifications** controls them:

- System notifications when an agent finishes or needs input.
- Whether to notify while Agentty is the front application.
- Questions and permission requests always notify, unless you are looking at that pane.
- One question from an agent gives one notification, even though the agent reports it more than once. A new question after you answered notifies again.

### Allow or deny from the notification

A permission request from Claude Code or Codex shows what it asks (the tool and, for a command, its lines) with **Allow** and **Deny** buttons, on macOS and in the mini window. Allow answers "Yes" once; Deny answers "No" (Esc), and the agent waits for what to do instead. Neither brings Agentty to the front. "Yes, and don't ask again" stays in the terminal.

Agentty types the answer only while that same request still waits on screen. If you already answered it in the terminal, or the agent moved on, the button opens the pane instead. On macOS, Allow needs the Mac to be unlocked. With the banner style, the buttons show when you point at the notification; choose the **Alerts** style for Agentty in System Settings → Notifications to keep them on screen. Questions (a choice between answers) are answered in the terminal.
- "In front" means Agentty is the active application at that moment.

### To your phone

The same page connects **Slack**, **Discord** or **Telegram**, so a question reaches you when you are away from the machine. You choose whether a message is sent only when an agent needs you, or when one finishes as well, and whether to include what the agent asked.

Slack and Discord take either form: a webhook URL, or a bot token together with the channel to write to — Slack accepts `#general`, `general` or a channel id, Discord takes a channel id. Telegram is always a bot, with the chat to write to.

Webhook URLs and the bot token are stored in the operating system's credential store, never in a settings file.
