---
title: Session Flow
description: Share context between two running agents by drawing a line from one to the other.
---

**⇧⌘F** opens Session Flow: every agent session you have open, as nodes you can connect.

Drag a line from one session to another and the target receives the source's context — what it is working on, what it has decided, where it is. It works across Claude Code and Codex.

Between two Claude Code sessions the link is direct, whether you draw it here or pick a pane with a terminal's link button: the sessions message each other. The one you drew from is told to hand work for the other session's project over to it instead of doing it itself, and the other reports back when it is done. A session that has just started or is still resuming is waited for a few seconds; if the two still can't message each other, the conversation is handed over one way and Agentty says so.

With many sessions, move around the chart: scroll, or drag the empty space, to move the view; **⌘** + scroll zooms around the pointer. The buttons at the bottom right zoom out and in, go back to 100%, and fit every session in view.

## Link modes

| Mode | Behavior |
|---|---|
| Once | The context is sent at the moment you draw the line |
| **Live** | Every new turn in the source is forwarded to the target as it happens |
| Two-way | Both sides forward to each other — a context tunnel |

A live link keeps a second agent informed without you copying anything. A two-way link lets two agents work on halves of the same problem while each knows what the other just did.

## What is sent

A summary of the conversation, written to `~/.agentty/handoffs/` and handed to the target agent as a file. Prompts and responses are not replayed turn by turn.

## When to use which

- **Once** — you are starting a second agent on related work and want it to begin informed.
- **Live** — one agent implements while another reviews, writes docs or watches tests.
- **Two-way** — two agents split a feature and must not diverge.

Turning a link off stops the forwarding; what was already sent stays in the target's conversation.
