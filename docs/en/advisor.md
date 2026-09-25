---
title: Claude advisor
description: Let Claude Code consult a stronger model at the moments that matter, set per tab from Agentty.
---

Claude Code can ask a stronger model for advice at key moments in a turn — before a large refactor, when a plan needs judgment, when it is stuck. Agentty exposes that as a setting, so you don't have to remember a flag.

> [!NOTE]
> This is an experimental Claude Code capability and it uses extra tokens. It is off unless you choose otherwise.

## How it works

Agentty does not run the advisor itself. When it starts a Claude Code pane it adds `--advisor <model>` to the command line, and Claude Code does the consulting. That means:

- The advisor uses **your** Claude Code sign-in and your provider account, like the rest of the session.
- Nothing is written into your configuration files. The choice lives in Agentty's settings and is passed per launch.
- Choosing **Off** passes a switch that overrides an `advisorModel` you may have set in Claude Code, so "off" really is off for that pane.

## Choosing a model

**Settings → General → Claude advisor** applies to **new** Claude tabs:

| Choice | Effect |
|---|---|
| Claude Code setting | Agentty passes nothing; whatever Claude Code is configured with applies |
| Off | The advisor is disabled for that pane, overriding Claude Code's own setting |
| Opus | Consults Opus |
| Fable | Consults Fable |

A pane that is already running can be changed from its status bar, without restarting the session.

## When it is worth it

- **Worth it:** architectural decisions, a refactor that touches many files, debugging where the first three hypotheses failed, reviewing a plan before the agent commits to it.
- **Not worth it:** routine edits, tests, formatting, anything where the work is mechanical. The extra tokens buy nothing.

A reasonable default is to leave it off and switch it on from the status bar when a session reaches a decision you care about.
