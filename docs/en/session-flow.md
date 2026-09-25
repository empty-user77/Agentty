---
title: Session Flow
description: Share context between two running agents by drawing a line from one to the other.
---

**⇧⌘F** opens Session Flow: every agent session you have open, as nodes you can connect.

Drag a line from one session to another and the target receives the source's context — what it is working on, what it has decided, where it is. It works across Claude Code and Codex.

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
