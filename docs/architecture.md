# Architecture

```
┌──────────────────────────── agentty-app (GPUI) ────────────────────────────┐
│ Workbench (root view)                                                      │
│  ├─ groups → workspaces → tabs → PaneNode tree → TerminalView (pane)       │
│  ├─ side bar: workspaces / local sessions                                  │
│  ├─ pages: Git · Session Flow · AI Usage · Extensions · Settings           │
│  └─ picker (folder), launcher, status bar                                  │
│                                                                            │
│ TerminalView ── Backend ── alacritty_terminal::Term + PTY I/O thread       │
│      ▲                                                                     │
│      └── agent signals ◀── ipc.rs socket (Unix 0600 / Windows loopback) ◀ hooks│
└───────────────────────────────┬────────────────────────────────────────────┘
                                │ links
┌──────────────────────── agentty-bridge (library + CLI) ────────────────────┐
│ claude.rs / codex.rs  transcript discovery and parsing                     │
│ usage.rs / pricing.rs token accounting and reports                         │
│ handoff.rs            context documents for migration and sharing          │
│ extensions.rs         skills / agents / commands / plugins / MCP discovery │
│ connectors.rs         API connectors (OS credential store, stdio MCP)      │
│ agent_auth.rs         sign-in methods for agent tabs (keys, Codex home)    │
│ secret_store.rs       Keychain / Credential Manager / Secret Service       │
│ git.rs                git CLI wrapper for the Git page                     │
│ plugins/              plugin manifest, store, UI schema, agentty:// links │
└────────────────────────────────────────────────────────────────────────────┘
```

## Terminal pipeline

1. `launch.rs` builds the command line. Everything runs through the user's login shell; agent panes append
   `; exec $SHELL -l` so the pane survives the agent exiting.
2. `terminal/backend.rs` spawns the PTY and alacritty's event loop thread, which parses output into the grid.
3. Events (wakeups, title, bell, exit) cross to the UI thread over a channel and are coalesced into one repaint.
4. `terminal/view.rs` paints the visible grid in `prepaint`: background spans, text runs grouped by style (wide and
   Nerd Font glyphs are isolated so they stay on the cell grid), cursor and IME pre-edit text.

## Agent status

- **Claude Code** is launched with `--session-id <uuid>` and `--settings '{"hooks": …}'` so `UserPromptSubmit`,
  `PreToolUse`, `Stop` and `Notification` hooks report to Agentty. User settings files are untouched.
- **Codex** is launched with `-c notify=[…]`; its turn-complete notification reports `stop`. Pressing Enter in a
  Codex pane marks it as working.
- Hooks write `<pane id>\t<kind>\t<json>` lines to `$AGENTTY_SOCKET`; `agent_signal.rs` parses them and the
  workbench updates the pane. `Stop` and `Notification` set the pane's attention flag until the user clicks it.
- The socket (a Unix socket, `0600`, in the user's private temp folder) only hears processes inside a pane: the
  kernel names the process on the other end (`LOCAL_PEERPID`), Agentty follows its parents up to the shell of a
  pane it started, and the connection may speak for that pane only — status, notifications and `agentty browser`
  alike. Plugins, other apps and scripts outside a pane are not heard. A `tmux` or `screen` server started in a
  pane detaches from it, so agents inside one are not heard either. The debug driver (`AGENTTY_DEBUG=1`) is the
  one exception.

## Persistence

`workbench/persist.rs` stores groups, workspaces, tabs and split trees. On launch only the active workspace spawns
processes; others stay dormant until opened. Agent panes resume their session when its transcript still exists.

## Session Flow

Dragging from a node's handle to another node creates an edge. The source transcript is converted by
`handoff::create_share` into a Markdown document under `~/.agentty/handoffs/`, and a prompt asking the target agent to
read it is submitted to the target pane (bracketed paste + Enter).

## Git page

`git_view.rs` resolves repository roots from the working directories of open panes (the active pane's repository is
selected when the page opens). `agentty_bridge::git` shells out to the user's `git` with `GIT_TERMINAL_PROMPT=0`, so
credentials, hooks and signing follow the user's configuration and nothing can block on a prompt. All commands run on
background threads; the page polls status every 3 seconds only while visible. Branch names are validated with
`git check-ref-format` and commit ids must be hex, so user input is never parsed as an option.

## Mini mode, menu bar and notifications

- `workbench/mini.rs` folds the main window into the top-right corner with AppKit frame animation (`native.rs`),
  orders it out, and opens a non-activating `PopUp` window with `MiniView`. The panel observes the workbench;
  notices arriving while it is open become speech bubbles. Restoring grows the main window back from the panel.
- `status_item.rs` owns an `NSStatusItem`. Menu clicks are queued from Objective-C and drained by a GPUI task that
  also advances the spinner (every 120 ms while an agent works, 700 ms otherwise).
- `notifications.rs` uses `UNUserNotificationCenter` inside the app bundle (click → `TrayAction::Focus(pane)`) and
  falls back to `osascript` for unbundled development builds.

## Live links

A live Session Flow edge remembers how many source turns it delivered. When the source agent finishes a turn, only the
new turns are written to an update document and submitted to the target (queued if the target is busy). The target's
next finished turn is its reply to that context and is not forwarded back, so two-way links cannot loop.

## Plugins

```
 other app ──agentty://──▶ main.rs (on_open_urls) ──▶ Workbench::open_agentty_link
                                                         │
 plugin process ◀─ stdin/stdout JSON-RPC ─▶ plugins/process.rs ──▶ plugins::handle (app thread)
   (node main.mjs)                                        │            │
                                   panel tree, badge, logs│            │ window calls
                                                          ▼            ▼
                                             PluginHost (global)   workbench/plugin_host.rs
                                                                   (prompts, terminals, sessions)
```

- `agentty-bridge::plugins` has no UI: the manifest (`agentty-plugin.json`), installed plugins and their enabled state
  (`~/.agentty/plugins/state.json`), built-in plugins embedded in the binary (`plugins/cosmica`, the template and the
  SDK are `include_str!`-ed), the panel UI schema with size limits, and `agentty://` link parsing.
- `plugins/mod.rs` (app) is a GPUI global shared by all windows. A plugin starts on first use (or at launch with
  `onStartup`); `plugins/process.rs` runs it with the login shell's PATH and reader/writer threads that forward events
  over a channel to the app loop. Events from an older process generation are ignored after a restart.
- Calls are checked against the manifest's permissions. Calls that need a window (`prompt/inject`, `terminal/send`,
  `session/get`, `workspace/list`, `ui/notify`) go to the frontmost workbench. After a link reached a plugin, and until
  the user interacts with that plugin, its prompts are forced through the "Send to…" dialog and `terminal/send` is
  refused, so a web page can't drive an agent through a plugin.
- Plugins never draw: `ui/setPanel` sends a tree (columns, rows, text, buttons, inputs, lists, …) that
  `workbench/plugin_panel.rs` renders natively in a column right of the terminals; text inputs are GPUI entities kept
  per plugin and element id. Pane-bar commands render in the agent status bar and split-pane headers.
- `workbench/prompt_dialog.rs` is the "Send to…" dialog; `deliver_prompt` starts new agent sessions with the prompt as
  their first message, types into idle agents with bracketed paste + Enter, and only ever types (never presses Enter)
  into plain shells.

See [docs/plugins](plugins/README.md) for the plugin developer guide and protocol.

## Build my idea and Launch

```
 idea page (idea_view.rs) ──Start──▶ workbench/idea.rs ──▶ agentty_bridge::idea::create_project
   messages, pasted plans,                │                  ~/AgenttyProjects/<slug>/
   attached files                         │                    docs/idea/IDEA.md, attachments/, BUILD_GUIDE.md
                                          │                    .claude/settings.json (npm/npx/node, browser tools)
                                          ▼
                              deliver_prompt(NewWorkspace, claude|codex, build prompt)
                                          │  agent plans, uses subagents, runs the dev server and
                                          │  opens it in the in-app browser (browser MCP tools)
                                          ▼
                              🚀 Launch plugin (plugins/launch, built in)
                                 gh / vercel / supabase CLIs (installed into the plugin data folder if missing)
                                 GitHub login → repo + push → Vercel login → [Supabase] → env vars → deploy → URL
```

- The idea page is a chat: Enter adds a message, a multi-line paste becomes a document message
  (`TextInput::keep_pasted_lines`), files are attached with the button or dropped on the page.
- `create_project` leaves the folder otherwise empty so `create-next-app` / `create vite` still accept
  it; the build guide tells the agent to scaffold into a subfolder if a scaffolder refuses.
- The build guide starts with the project's harness: the agent downloads ECC (github.com/affaan-m/ECC,
  MIT — agent, skill and rule definitions) into the new project with a shallow, sparse clone, installs
  the few Markdown definitions the idea needs under `.claude/`, records them in `docs/idea/HARNESS.md`
  and removes the download. Nothing of ECC ships with Agentty; scripts, hook definitions, MCP configs
  and settings are never installed.
- Claude Code starts in auto mode there (`--permission-mode auto`, only the command line can turn it
  on). The project's `.claude/settings.json` is the fallback: `acceptEdits` plus package-manager, node,
  browser-tool and the harness download commands; deleting files and pushing still ask.
- Launch is installed from the built-in catalog on first use (+ menu, or when an idea project starts)
  and adds a 🚀 button to agent panes. Logins run the official CLIs' browser flows; Agentty and the
  plugin never see or store the tokens. `.env` values are piped to `vercel env add` on stdin and only
  key names are shown.
- The Supabase step appears only for projects that use it (or on request). `supabase login` needs a
  terminal and a typed verification code, so it runs under `script` and the panel has a field for the
  code. Launch writes the project URL and the public (anon / publishable) key to `.env.local`, never
  the service_role key; the database password lives in `.env.local` only and reaches
  `supabase link` / `db push` through the environment. The build guide tells the agent which variable
  names to read and to put the schema, with row level security, in `supabase/migrations`.
