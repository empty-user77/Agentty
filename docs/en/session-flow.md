---
title: Orchestration
description: Watch an agent hand work to its subagents in waves, and link sessions so they share context.
---

**⇧⌘F** opens the Orchestration page. (In the file editor, ⇧⌘F searches the project's files instead; the page is also in the **View** menu.) It has two tabs: **Workflow** shows how the work of a session is split up and how far it got, **Session links** connects sessions so they share context.

## Starting an orchestration

Write a goal and an orchestrator agent does the rest: it plans a team of agents for it, shows you the plan, runs it in waves and has the result checked.

Start one from the home page (**Orchestration**), from a tab's right-click menu (**New orchestration tab**), from a new workspace's start page, or with **New orchestration** at the top right of this page. In the dialog:

- **Goal** — what the team should get done. The examples under it fill it in: market research, a tutorial from using a web app, building a feature with review, fixing failing tests.
- **Models** — Balanced (Sonnet works, Opus reviews), Best quality (Opus throughout) or Economical (Haiku works, Sonnet reviews). The plan names a model for every agent. Shown when Claude Code leads; with Codex every CLI uses its own default.
- **Folder** — where the team works.
- **AI on the team** — the lead always, and the other agents installed here (Claude Code, Codex, Gemini CLI, Antigravity CLI), each on or off. Claude Code leads when it is installed, Codex when it is not; without either, the orchestration can't start. The plan gives each part to the agent whose strength fits — Codex for precise code changes and review, Gemini for very large inputs — and they never run with their permission checks off.

A Codex lead has no subagents: it runs every agent of a wave as a process of an agent CLI and tells Agentty when each starts and finishes (`agentty plan --start` / `--done`), so the page follows it the same way. Codex asks before running those commands outside its sandbox.

A new workspace goes in the **Orchestration** group, made the first time; move it to any group you like. The page then shows its workflow. When the plan is ready, a bar says so: **View plan** opens the orchestrator's pane, **Ask for changes** sends it a message, and **Start** runs it. Agents that need their own branches go to parallel sessions (`agentty tasks`); a reviewer checks the result against the goal at the end, and the orchestrator finishes with a report.

## Following, resuming and stopping

The orchestrator's pane has a strip across its top with the same words as this page: planning, the plan ready (with **Start**), running with how many agents are at work and how many are done, done, interrupted or stopped. **Open workflow** comes here; the sidebar and the menu bar mark its workspace with the orchestration icon, in the colour of its stage.

Orchestrations are kept across restarts. If Agentty quits (or crashes) while one runs, its agents stop with it: the strip and this page say **Interrupted**, and **Resume** opens the session again if needed and has the orchestrator check what is already done and run only what is left. **Stop all** (after a confirmation) ends the orchestrator's turn and every agent still running; **Resume** carries on later. **Delete** takes an orchestration off the list and closes its terminal, with its workspace when that holds nothing else; the files it made stay. Deleting that workspace deletes the orchestration too.

Its settings are under **Settings → AI → Orchestration**: on or off, the models a new one starts with, the other AI agents it uses by default, whether the plan waits for **Start**, and whether a new one opens as a workspace or a tab.

## Workflow

On the left are the threads: every agent session running in Agentty, with a dot in its state's colour, and the parallel runs agents started with `agentty tasks`. Pick one to see its workflow:

- At the top, what you last asked it (`$ …`) and the session's name.
- The orchestrator's line: the main agent's state, how many waves, how many subagents are done, and how many models are at work. **Open** goes to its pane, **Message** sends it a line as if typed there, and **Interrupt** stops its current turn (as Esc does in its pane).
- The graph: the orchestrator first, then its subagents in waves, each wave a column. An orchestration's waves come from its plan: every agent of it is on the graph from the moment the plan is ready, those that have not started as pending, with lines from the agents each one waits for, so you see the work pass from wave to wave. For another session, subagents that ran at the same time form a wave. The wave at work is underlined. Each card says whether it is running, completed or pending and for how long, what kind of agent it is and on which model, and its current step or its last word. Lines into a running wave move. Click a card to open that subagent's log.
- At the bottom, the latest steps of its subagents as they happen. Drag its top edge to make it taller or shorter; the list on the left is made wider or narrower by its right edge.

A parallel run shows its tasks as one wave, each with its branch (or folder) and its pane's state; click one to go to its pane.

Drag the empty space of the graph (or use the wheel) to move it; ⌘ or Ctrl with the wheel, or the buttons at the bottom right, zoom, and **Fit to view** shows it whole.

## Session links

Drag a line from one session to another and the target receives the source's context — what it is working on, what it has decided, where it is. It works across Claude Code and Codex.

Between two Claude Code sessions the link is direct, whether you draw it here or pick a pane with a terminal's link button: the sessions message each other. The one you drew from is told to hand work for the other session's project over to it instead of doing it itself, and the other reports back when it is done. A session that has just started or is still resuming is waited for a few seconds; if the two still can't message each other, the conversation is handed over one way and Agentty says so.

With many sessions, move around the chart: scroll, or drag the empty space, to move the view; **⌘** + scroll zooms around the pointer. The buttons at the bottom right zoom out and in, go back to 100%, and fit every session in view.

### Link modes

| Mode | Behavior |
|---|---|
| Once | The context is sent at the moment you draw the line |
| **Live** | Every new turn in the source is forwarded to the target as it happens |
| Two-way | Both sides forward to each other — a context tunnel |

A live link keeps a second agent informed without you copying anything. A two-way link lets two agents work on halves of the same problem while each knows what the other just did.

### What is sent

A summary of the conversation, written to `~/.agentty/handoffs/` and handed to the target agent as a file. Prompts and responses are not replayed turn by turn.

### When to use which

- **Once** — you are starting a second agent on related work and want it to begin informed.
- **Live** — one agent implements while another reviews, writes docs or watches tests.
- **Two-way** — two agents split a feature and must not diverge.

Turning a link off stops the forwarding; what was already sent stays in the target's conversation.
