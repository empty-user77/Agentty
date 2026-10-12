---
title: Chat workspace
description: Talk to one lead agent in a chat while it hands work to up to four workers and reviewers you can watch below.
---

A chat workspace puts a conversation on top and a team of agents below it. You talk to one **lead agent** (Claude Code) in the chat. It plans the work and hands pieces of it to **workers**, and can have their work checked by **reviewers**. Each of them runs in its own terminal in a grid under the chat, so you can watch the whole team at once.

## Opening one

- On the start page (or **New workspace**), pick **Chat · team**, then the project folder.
- Or open the **+** menu next to the tabs and choose **New chat workspace**.

## The chat's branch

In a git repository, every chat gets a branch of its own (`agentty/chat-…`, shown in the chat's bar) and the lead works in a worktree on it. Your project folder is left alone.

- Each worker gets its own worktree on a new branch that starts from the **latest commit of the chat's branch**. Work the lead has already merged is there for the next worker to build on.
- The lead merges a worker's branch into the chat's branch once it has looked at the work.
- The result reaches your project folder only when you ask for it: the lead opens a pull request if the repository has a remote, or merges into the project folder if that folder has no uncommitted changes.

A repository without any commit has nothing to branch from. There the lead works in the project folder, and workers start from the default branch once the lead has made the first commit.

## How it works

1. Write in the chat. **Enter** sends; **Shift+Enter** adds a line.
2. The lead answers small things itself. For bigger work it makes a plan and starts workers with `agentty tasks`. In a chat workspace they start at once, without the usual confirmation dialog: opening the chat is your yes.
3. Workers appear in the grid below the chat, two per row, with at most **4** at a time, reviewers included. When all four places are taken, the one that finished longest ago closes to make room for the next one. Its branch and its session stay, so you can resume it from the session list.
4. When a worker ends a turn, Agentty sends the lead a short report: the worker's title, branch, folder and last reply. The lead reviews the work, merges it, tells you in the chat, or sends the worker a follow-up.

The bar above the chat shows the lead's status, the chat's branch and a chip for each worker. Click a chip to jump to that worker's terminal. If a worker waits on you (an approval, a question), the chat says so too.

## Workers and reviewers

Two buttons in the chat's bar choose who does the work:

| Button | Choices | What it decides |
|---|---|---|
| **Workers** | Claude · Codex | The agent new workers run as, unless the lead names one for a task |
| **Reviewers** | off · Claude · Codex | Whether the lead is asked to have each worker's work reviewed before merging, and by which agent |

A **reviewer** starts in the worker's folder and reads its changes against the chat's branch. It can run the tests and is told not to change anything: Codex runs in a read-only sandbox; Claude Code runs without its editing tools (its shell still could write, so it relies on the instruction there). It ends with `Verdict: approve` or `Verdict: changes needed`, and its report goes to the lead like a worker's. The lead can also start a reviewer while reviews are off; the button then still picks the agent.

Your choices are saved with the chat.

## The lead's terminal

The chat is a view over a real Claude Code session. **Terminal** in the chat's bar shows the lead's terminal; **Chat** goes back. The terminal also comes up by itself when only it can answer:

- the first-run question about trusting the folder, or a sign-in,
- a tool approval or a question from the lead.

Answer there and the chat comes back. Messages that arrive meanwhile wait until the lead is free. Agentty never types into an agent while its screen shows an approval or a question.

## Commands the lead uses

| Command | What it does |
|---|---|
| `agentty tasks --plan <plan.json>` / `--title … --prompt-file …` | Start workers (here without a dialog) |
| `agentty tasks status` | List the workers, what each is doing, and the chat's choices |
| `agentty tasks send --to <title> --prompt <text>` | Send a worker a follow-up message |
| `agentty tasks result --to <title>` | Print a worker's last reply again |
| `agentty tasks stop --to <title>` | Close a worker (its branch and session stay) |
| `agentty tasks review --worker <title> [--prompt <focus>] [--agent claude\|codex]` | Start a reviewer of a worker's work |

These commands only work for the lead of a chat workspace, and only on its own workers.

## Plan usage

Every worker and reviewer draws on the same plans as the lead. The chat's bar shows how much of each plan the chat runs on is used: Claude, and Codex too when a worker or reviewer uses it. It shows the window closer to running out, the 5-hour one or the weekly one; hover it for when that window resets. The chip turns orange from 70 % and red from 85 %.

From 85 % a warning above the conversation says how full the plan is and how many agents are working on it now: reaching the limit stops them all midway. The lead hears the same when it starts workers, so it can tell you and start no more than needed.

## From your phone

With [remote access](/docs/remote-access) on, a chat workspace opens on the page as its chat: the conversation, the chat's branch, the worker and reviewer choices and plan usage, with the box at the bottom to write to the lead. **Terminal** at the top switches to the lead's terminal and **Chat** back; while the lead waits for an approval or an answer, its terminal shows by itself. The workers' terminals are in the list of split panes.

## Good to know

- Workers commit on their own branches. They push or open pull requests only if you ask the lead for that.
- Every worker and reviewer is a full agent session, and four of them working at once use your plan's limits faster than one.
- Drag the line between the chat and the grid to give either more room. The layout is saved with the workspace.
- Turning off **Agents can start parallel tasks** in Settings also stops a chat's lead from starting workers.
- Chats opened before chat branches existed keep working in the project folder.
