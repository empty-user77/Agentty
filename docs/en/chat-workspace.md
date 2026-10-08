---
title: Chat workspace
description: Talk to one lead agent in a chat while it hands work to up to four workers you can watch below.
---

A chat workspace puts a conversation on top and a team of agents below it. You talk to one **lead agent** (Claude Code) in the chat. It plans the work and hands pieces of it to **workers**. Each worker runs in its own terminal in a grid under the chat, so you can watch the whole team at once.

## Opening one

- On the start page (or **New workspace**), pick **Chat · team**, then the project folder.
- Or open the **+** menu next to the tabs and choose **New chat workspace**.

The lead works in the project folder itself. Each worker gets its own git worktree on a new branch from the project's default branch, so workers never edit the same files.

## How it works

1. Write in the chat. **Enter** sends; **Shift+Enter** adds a line.
2. The lead answers small things itself. For bigger work it makes a plan and starts workers with `agentty tasks`. In a chat workspace they start at once, without the usual confirmation dialog: opening the chat is your yes.
3. Workers appear in the grid below the chat, two per row, with at most **4** at a time. When all four places are taken, the worker that finished longest ago closes to make room for the next one. Its branch and its session stay, so you can resume it from the session list.
4. When a worker ends a turn, Agentty sends the lead a short report: the worker's title, branch, folder and last reply. The lead reviews the work and tells you in the chat, or sends the worker a follow-up.

The bar above the chat shows the lead's status and a chip for each worker. Click a chip to jump to that worker's terminal. If a worker waits on you (an approval, a question), the chat says so too.

## The lead's terminal

The chat is a view over a real Claude Code session. **Terminal** in the chat's bar shows the lead's terminal; **Chat** goes back. The terminal also comes up by itself when only it can answer:

- the first-run question about trusting the folder, or a sign-in,
- a tool approval or a question from the lead.

Answer there and the chat comes back. Reports from workers that arrive meanwhile wait until the lead is free.

## Commands the lead uses

| Command | What it does |
|---|---|
| `agentty tasks --plan <plan.json>` / `--title … --prompt-file …` | Start workers (here without a dialog) |
| `agentty tasks status` | List the workers and what each is doing |
| `agentty tasks send --to <title> --prompt <text>` | Send a worker a follow-up message |

`status` and `send` only work for the lead of a chat workspace, and only on its own workers.

## Good to know

- Workers commit on their own branches. They push or open pull requests only if you ask the lead for that. The lead merges a branch only when you ask it to or approve a plan that includes merging.
- Every worker is a full agent session, and four of them working at once use your plan's limits faster than one.
- Drag the line between the chat and the grid to give either more room. The layout is saved with the workspace.
- Turning off **Agents can start parallel tasks** in Settings also stops a chat's lead from starting workers.
