# AgentOS plugins

An **AgentOS** is a plugin that runs work through Agentty's agents instead of doing it itself: it
holds the skills and the rules for a trade — marketing, blogging, an influencer's week — and walks
a piece of work through them, step by step, with the agents doing the writing and the user watching
and stepping in.

It is a plugin like any other: one WebAssembly module, the same permissions, the same panel. What
makes it an AgentOS is what it contains and what it does with `prompt.inject`.

## What it is

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

A step is: send a prompt to an agent, wait for that agent to stop working, read what it produced,
decide. The plugin keeps its state through `storage/*`, so a run survives a restart, and draws it
in its panel — the steps, where the run is, what came back, the button that approves and moves on.

An AgentOS is not a new runtime or a new permission. It is the shape of a plugin that happens to
be mostly prompts, and Agentty's part is small: give a plugin a way to wait, and a way to hear that
an agent it started has finished.

## What the host has to give it

A module has no clock and no loop: it only ever runs while it is handling a message. Two things it
cannot do without.

### `host/timer` — waiting

| Method | Permission | `params` | Result |
|---|---|---|---|
| `host/timer` | | `{ ms }` | `{ elapsedMs }`, once the time has passed |

A request that is answered later. 100 ms at the shortest, an hour at the longest, eight in flight
per plugin; a plugin that is stopped or restarted loses the ones it was waiting for. Polling
something every few seconds is what this is for; it is not a way to run in the background, because
answering it is all the plugin gets.

### `pane/status` — hearing that an agent finished

| Message | Kind | `params` |
|---|---|---|
| `pane/status` | notification | `{ paneId, status, agent, title?, cwd?, running }` |

Sent when a pane **this plugin started** changes what it is doing — `working`, `idle`,
`finished`, `permission`, `question`, `interrupted`, `exited`. A plugin learns a pane id from
`prompt/inject` (`{ status: "sent", paneId }`); Agentty remembers which plugin started which pane
and tells only that plugin. It needs `workspace.read`, the permission that already means "see
agent status", and `title` and `cwd` come with it.

Without this a plugin would have to poll `session/get` and guess. With it, a step ends when the
agent says it has ended.

### What it already has

- `prompt.inject` to start a session or send the next prompt, with `target` choosing a new tab, a
  split, or the pane a step is already running in.
- `session.read` to read what the agent wrote — the step's output.
- `storage/*` to keep the run, the skills the user edited and what was approved.
- `net.request` for a plugin that also publishes somewhere, if it asks for it.
- `ui/setPanel` for the run's own view: steps, state, what came back, what to press.

## Writing one

The part every AgentOS has in common is in the Rust SDK, as `agentty_plugin::agentos`. A workflow
is a list of steps, and a step is a prompt, a check on what comes back, and whether the user is
asked before the next one.

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

`{input}` is what the run was started with and `{step.<id>}` is what an earlier step produced;
both are filled in before the prompt is sent, along with anything in the workflow's glossary.

The run is a small state machine, and every transition is a message the plugin already gets:

| It happens | The runner does |
|---|---|
| The user presses **Start** | `prompt/inject` with the first step's prompt, `target: "newTab"` → remembers `paneId` |
| `pane/status` says that pane is `working` | remembers that the prompt has been taken up |
| It says `finished` or `idle` after that | waits 2.5 s to see whether the stop lasts |
| It is still stopped | `session/get`, then the step's check |
| The check passes | keeps what it produced, and shows it or sends the next step |
| The check fails | sends the agent what is missing — three times, then it stops and says why |
| The agent asks for something, or its pane goes | the run stops and says so |
| Agentty restarted | the run is read back from `storage` and asks its session again |

Two things the runner does not let a workflow decide, because an AgentOS is a plugin that talks to
agents on the user's behalf:

- **The user is asked before the last step**, whatever that step says. The last step is the one
  that acts on the world — posts, pushes, sends — and what the user is shown before it runs is
  what it will act on. A workflow that marked that step automatic would be a plugin writing to the
  user's accounts while they are not looking.
- **A workflow that would publish on an answer nobody read is refused when the plugin starts.**
  `Workflow::checked()` says so in `init`, not at the moment it would have posted.

And two the workflow itself has to get right:

- **The user stays in front of it.** A step's prompt goes into a session the user can read, in a
  tab they can take over.
- **A link is not a run.** A plugin that a link reached cannot type into terminals and its prompts
  go through the "Send to…" dialog (`link_guarded`). An AgentOS started that way asks first — and
  it still follows the session the user placed, so asking costs it nothing but a turn.

## Why it needs no permission for a browser

The obvious way to make an AgentOS that posts would be to give plugins a browser. That would hand
every plugin the user's logged-in sessions, and it is not what happens here.

The plugin asks an agent, and Agentty already gives every agent session its own browser, driven
from the shell it is already allowed to use:

```
agentty browser navigate <url>        agentty browser text [selector]
agentty browser click <selector>      agentty browser type <selector> <text>
agentty browser elements              agentty browser screenshot <path.png>
```

So a workflow that posts is a prompt that says which page to open and what to type, sent to a
session the user is watching. The rules that matter — never sign in, never install anything, stop
and say what you saw, do not act while you are reading — are part of that prompt, and a workflow
puts them in one place with the glossary so no step can be written without them.

## The examples

Both are in the [marketplace repository](https://github.com/empty-user77/Agentty-Marketplace),
with their source and their checksums:

- **Blogger AgentOS** — outline, draft, edit, save. About 180 lines, most of them prompts. It is
  what to read before writing one.
- **Social AgentOS** — a post for X, replies to a conversation, an Instagram caption, each one
  driven through the browser the user is signed in to. It asks for `prompt.inject`,
  `session.read` and `workspace.read`, and nothing else.

## Open questions

- **Where do skills live?** In the module today. A plugin that lets the user edit a step's prompt
  keeps the edit in `storage`, which is enough for one machine; sharing a set of skills between
  people is a marketplace question, not a protocol one.
- **Several agents at once.** `prompt/inject` can open as many panes as it likes, and
  `pane/status` names each one. What is missing is a way to say "these three are one step", which
  the plugin can do itself by remembering the ids.
- **Cost.** A run is several agent sessions; Agentty's usage page shows what they cost, but a
  plugin cannot ask for it. `usage.read` would be a new permission, and it can wait until a real
  AgentOS wants it.
- **Running to a clock.** A run starts when the user presses Start. An AgentOS that posts every
  weekday morning would need Agentty to start one, and that is a different thing from a plugin
  waiting — `host/timer` is answered only while the plugin is running, and a plugin runs only
  while Agentty is open.
