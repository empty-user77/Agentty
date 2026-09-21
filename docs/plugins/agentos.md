# AgentOS plugins

*Draft. The protocol additions in "What the host has to give it" are implemented on this branch;
the example is a skeleton, not a product.*

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

```rust
struct Step {
    id: &'static str,
    title: &'static str,
    /// The skill: what this step asks an agent to do. English, like every prompt that ships in
    /// code; the plugin tells the agent which language to answer in.
    prompt: &'static str,
    /// What the plugin checks before calling the step done.
    rule: fn(&str) -> Result<(), String>,
}

const BLOGGER: &[Step] = &[
    Step { id: "outline", title: "Outline", prompt: OUTLINE, rule: has_three_sections },
    Step { id: "draft",   title: "Draft",   prompt: DRAFT,   rule: long_enough },
    Step { id: "edit",    title: "Edit",    prompt: EDIT,    rule: no_placeholders },
];
```

The run is a small state machine, and every transition is a message the plugin already gets:

| It happens | The plugin does |
|---|---|
| The user presses **Start** | `prompt/inject` with the first step's prompt, `target: "newTab"` → remembers `paneId` |
| `pane/status` says that pane is `finished` or `idle` | `session/get` on it, run the step's rule |
| The rule passes | show what came back, and the button for the next step |
| The rule fails | send the agent what is missing, and wait again |
| The user presses **Approve** | keep it in `storage`, move to the next step |
| Nothing has happened for a while | `host/timer` to look again |

Two things worth being strict about, because an AgentOS is a plugin that talks to agents on the
user's behalf:

- **The user stays in front of it.** A step's prompt goes into a session the user can read, in a
  tab they can take over. An AgentOS that runs unattended is a plugin that writes to the user's
  repositories through an agent while they are not looking.
- **A link is not a run.** A plugin that a link reached cannot type into terminals and its prompts
  go through the "Send to…" dialog (`link_guarded`). An AgentOS started that way asks first.

## The example

`src/blogger-agentos` in the [marketplace repository](https://github.com/empty-user77/Agentty-Marketplace/tree/main/src/blogger-agentos)
is the skeleton: three steps, the state machine above, the panel that shows it. It is there to be
read and to be run — not to be a product. A real one is mostly its prompts, and those are written
by whoever knows the trade.

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
