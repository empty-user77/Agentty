---
title: AgentOS plugins
description: A plugin that runs a piece of work through Agentty's agents instead of doing it itself — the shape, the rules it is held to, and why it needs no browser.
---

An **AgentOS** is a plugin that runs work through Agentty's agents instead of doing it itself. It holds the skills and the rules for a trade — blogging, marketing, an influencer's week — and walks a piece of work through them, step by step, with the agents doing the writing in sessions you can read and take over.

It is a plugin like any other: one WebAssembly module, the same permissions, the same panel. What makes it an AgentOS is what it contains and what it does with `prompt.inject`.

```
┌─ the plugin (a .wasm module) ──────────────────────────────────┐
│  skills   the prompts for each step, written for this trade    │
│  rules    what a step may not do, and what "done" means        │
│  the run  which step is next, what each one produced, what     │
│           the user approved                                    │
└────────────────────────────────────────────────────────────────┘
          │ prompt/inject                       │ session/get
          ▼                                     ▼
   a Claude Code or Codex session          what it wrote back
```

A step is: send a prompt to an agent, wait for that agent to stop working, read what it produced, decide. The plugin keeps its state in `storage/*`, so a run survives a restart, and draws it in its panel — the steps, where the run is, what came back, the button that approves and moves on.

An AgentOS is not a new runtime and not a new permission. It is the shape of a plugin that happens to be mostly prompts.

## Why it needs no browser

The obvious way to build a plugin that posts would be to give plugins a browser. That would hand every plugin your logged-in sessions, and it is not what happens here.

**The plugin has no browser and no network.** It asks an agent, and Agentty already gives every agent session its own browser — the one you are signed in to — driven from the shell the agent is already allowed to use:

```bash
agentty browser navigate <url>        agentty browser text [selector]
agentty browser click <selector>      agentty browser type <selector> <text>
agentty browser elements              agentty browser screenshot <path.png>
```

So a workflow that posts is a prompt saying which page to open and what to type, sent to a session you are watching. A plugin can publish to a website without ever being handed your logged-in session.

The rules that matter — never sign in, never install anything, stop and say what you saw, do not act while you are reading — are part of that prompt, and a workflow puts them in one place with the glossary so no step can be written without them.

## What Agentty adds

A module has no clock and no loop: it only ever runs while it is handling a message. Two messages make the rest possible, and both need `apiVersion: 2`.

### `host/timer` — waiting

| Method | Permission | `params` | Result |
|---|---|---|---|
| `host/timer` | | `{ ms }` | `{ elapsedMs }`, once the time has passed |

A request answered later. 100 ms at the shortest, an hour at the longest, eight in flight per plugin; a plugin that is stopped or restarted loses the ones it was waiting for. It is not a way to run in the background — answering it is all the plugin gets.

### `pane/status` — hearing that an agent finished

| Message | Kind | `params` |
|---|---|---|
| `pane/status` | notification | `{ paneId, status, agent, title?, cwd?, running }` |

Sent when a pane **this plugin started** changes what it is doing. A plugin learns a pane id from `prompt/inject` (`{ status: "sent", paneId }`); Agentty remembers which plugin started which pane and tells only that plugin. It needs `workspace.read`, the permission that already means "see agent status". At most 32 panes are followed at a time.

Status arrives whether or not anything is being drawn: a window behind another, or one on a locked screen, is not drawn, and a plugin waiting for an agent must not be waiting for you to come back.

A prompt you placed yourself is followed too. `target: "ask"` answers `{ status: "asked" }` with no pane id, because there is none yet — the session you pick is watched all the same, and its first `pane/status` is where the plugin learns which pane it became.

## Writing one

The part every AgentOS has in common is in the Rust SDK, as `agentty_plugin::agentos`. A workflow is a list of steps; a step is a prompt, a check on what comes back, and whether the user is asked before the next one.

```rust
static BLOGGER: Workflow = Workflow {
    id: "blogger",
    title: "Blog post",
    agent: Some("claude"),
    // Pieces of prompt several steps share — the rules for driving a browser, a house style.
    glossary: &[],
    steps: &[
        Step { id: "outline", title: "Outline", prompt: OUTLINE, check: has_headings, approval: Approval::Auto },
        Step { id: "draft", title: "Draft", prompt: DRAFT, check: long_enough, approval: Approval::Auto },
        Step { id: "edit", title: "Edit", prompt: EDIT, check: no_placeholders, approval: Approval::Ask },
        Step { id: "save", title: "Save", prompt: SAVE, check: names_a_file, approval: Approval::Ask },
    ],
};
```

`{input}` is what the run was started with and `{step.<id>}` is what an earlier step produced; both are filled in before the prompt is sent, along with anything in the workflow's glossary.

`Approval::Auto` starts the next step on its own; `Approval::Ask` shows what came back and waits for **Continue**.

### The run

Every transition is a message the plugin already gets:

| It happens | The runner does |
|---|---|
| The user presses **Start** | `prompt/inject` with the first step's prompt, `target: "newTab"` → remembers `paneId` |
| `pane/status` says that pane is `working` | remembers that the prompt has been taken up |
| It says `finished` or `idle` after that | waits 2.5 s to see whether the stop lasts |
| It is still stopped | `session/get`, then the step's check |
| It goes back to work during those 2.5 s | what it had said was not its answer after all — back to waiting |
| The check passes | keeps what it produced, and shows it or sends the next step |
| The check fails | sends the agent what is missing — three times, then it stops and says why |
| The agent asks for something, or its pane goes | the run stops and says so |
| Agentty restarted | the run is read back from `storage` and asks its session again |

The 2.5 s wait is not a guess. An agent between two tool calls is idle for a moment, and a session read in that moment gives back half a sentence and the tool it was about to run. A step that took that for its answer would move on having read nothing.

The `working` row is there for the same reason. A pane is `idle` from the moment it opens, before the agent has picked the prompt up, so `idle` on its own never means finished — the run believes a stop only once it has seen that pane `working` at least once.

## The two rules

Two things a workflow does not get to decide, because an AgentOS is a plugin talking to agents on your behalf.

> [!IMPORTANT]
> **You are asked before the last step, whatever that step says.** The last step is the one that acts on the world — posts, pushes, sends — and what you are shown before it runs is what it will act on. A workflow that marked that step automatic would be a plugin writing to your accounts while you are not looking.

**A workflow that would publish on an answer nobody read is refused when the plugin starts.** `Workflow::checked()` says so in `init` — not at the moment it would have posted. It also refuses a workflow with fewer than two steps, duplicate step ids, or a glossary entry nothing uses.

And two the workflow itself has to get right:

- **The user stays in front of it.** A step's prompt goes into a session they can read, in a tab they can take over.
- **A link is not a run.** A plugin a link reached cannot type into terminals, and its prompts go through **Send to…** for as long as it keeps running. An AgentOS started that way asks first — and it still follows the session you placed, so asking costs it nothing but a turn.

## The examples

Both are in the [marketplace repository](https://github.com/empty-user77/Agentty-Marketplace), with their source and their checksums.

**Blogger AgentOS** — outline, draft, edit, save. About 180 lines, most of them prompts. The checks are real ones: three sections in the outline, three hundred words in the draft, nothing left saying `TODO`. It is what to read before writing your own. Permissions: `prompt.inject`, `session.read`, `workspace.read`.

**Social AgentOS** — three workflows, each driven through the browser you are signed in to:

| | Steps |
|---|---|
| **Post to X** | read what is being said → draft → **you read it** → post it |
| **Reply on X** | find conversations worth joining → draft the replies → **you read them** → send |
| **Instagram caption** | read how the account writes → caption → **you read it** → save it, and the caption goes on your clipboard |

It will not post anything you have not read, will not sign in (a prompt that meets a login or a captcha stops and tells you what it saw), will not like, repost, reply or follow while it is reading, and sends at most five replies in a run. Instagram needs the picture chosen by hand, so that last step writes the caption to a file, puts it on your clipboard and opens Instagram for you.

It asks for `prompt.inject`, `session.read` and `workspace.read`, and nothing else. There is nowhere for it to keep a password, because it never has one.

## What is not there yet

- **Skills live in the module.** A plugin that lets you edit a step's prompt keeps the edit in `storage`, which is enough for one machine; sharing a set of skills between people is a marketplace question, not a protocol one.
- **Several agents at once.** `prompt/inject` can open as many panes as it likes and `pane/status` names each one, but there is no way to say "these three are one step" — the plugin does it itself by remembering the ids.
- **Cost.** A run is several agent sessions. Agentty's usage page shows what they cost; a plugin cannot ask for it.
- **Running to a clock.** A run starts when you press **Start**. An AgentOS that posted every weekday morning would need Agentty to start one, and `host/timer` is answered only while the plugin is running — which is only while Agentty is open.

## Next

- [Rust and WebAssembly](/docs/plugin-rust) — the SDK an AgentOS is written against
- [Plugin protocol](/docs/plugin-protocol) · [Permissions](/docs/plugin-permissions)
