# Architecture

```
┌──────────────────────────── agentty-app (GPUI) ────────────────────────────┐
│ Workbench (root view)                                                      │
│  ├─ groups → workspaces → tabs → PaneNode tree → TerminalView (pane)       │
│  ├─ side bar: workspaces / local sessions                                  │
│  ├─ pages: Git · Session Flow · Monitoring · Extensions · Settings         │
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

## Working trees and the files panel

- `agentty_bridge::worktree` wraps `git worktree`. `workbench/worktrees.rs` hooks into `Workbench::launch`: a *new agent*
  session whose folder is a working tree another live agent pane already uses gets `git worktree add -b
  agentty/<name>` under `data_dir()/worktrees/<project>-<hash>/` and starts there. The branch starts from the
  project's default branch (`worktree::base_ref`: the local branch `origin/HEAD` names, else the remote one, else
  `main` / `master`; the checked-out commit only when there is none), never from whatever the project folder has
  checked out. Shells, resumed sessions and the first session stay in the project. Only trees under that folder are
  ever removed by Agentty on its own (the trash button in the files panel, never with `--force`). The panel's
  right-click menu can also remove a linked tree the user made (`worktree::remove_linked`: never the project's own
  tree, never `--force`, the branch only through `git branch -d`) and forget trees whose folder is gone (`prune`).
- Agents typed into a terminal take the same path before they start: `shell_integration` wraps `claude` and `codex`
  (zsh, bash, PowerShell) with a function that runs `agentty worktree-for <agent>`. That sends `worktree\t{cwd,label}`
  on the pane's socket connection; `answer_worktree_request` checks every window for another agent pane in that tree,
  creates one if so and answers its path, and the function `cd`s there before running the real program. `--version`,
  subcommands like `mcp` and resumed sessions (`-c`, `--resume`, `codex resume`) stay where they are. The wrappers
  only ask when the pane has `$AGENTTY_SHELL_API` (set by versions that answer), so wrapper files written by a newer
  Agentty never start an older one that lacks the command.
- A session tree of a project the user trusts in Claude Code inherits that trust (`claude_trust::inherit_trust` sets
  `hasTrustDialogAccepted` on the tree's entry in `~/.claude.json`, nothing else, atomically), so the session starts
  without the folder question. Projects nobody trusted (nor a folder above them) are still asked.

## Agents and Agentty

- `agent_guide.rs` writes a short English guide and a Claude Code plugin with the `agentty-parallel-tasks` skill to
  `data_dir()/agent-guide/` at startup. Agent panes started by Agentty pass them on the command line (Claude Code
  `--append-system-prompt-file` / `--plugin-dir`, Codex `-c developer_instructions=…` unless the user set their own);
  a `claude` typed into a pane gets them from the shell wrapper (`$AGENTTY_GUIDE_FILE`, `$AGENTTY_PLUGIN_DIR`) unless
  it is a subcommand or already has such options. Nothing is written into projects or agent settings. Settings →
  General → "Tell agents what Agentty offers" turns it off.
- `agentty tasks` (`tasks_cli.rs`) lets an agent hand pieces of work to new sessions: `tasks\t{cwd,tasks}` on the pane's
  socket connection (checked: 1–6 tasks, titles and prompts bounded, agent `claude` / `codex`). `workbench/tasks.rs`
  queues the request and shows one dialog per request; nothing starts without the user's click. On yes, every task
  gets `worktree::create` from the asking agent's project and a pane split off the asking pane (right, then below),
  started with its prompt (`Start::Prompt`, sent at once). The agent gets the titles, branches and folders back.
  Settings → General → "Agents can start parallel tasks" refuses requests without asking.
- Prompts from links and plugins that arrive while the "Send to…" dialog is open wait in a queue instead of
  replacing it.
- `workbench/files_panel.rs` is the last column of the terminal area. It follows the active pane (or a tree picked in
  the panel), reads folders lazily, colors entries from `git status`, and lists the repository's working trees with the
  panes working in each. Everything is read on a background thread every few seconds while the panel is open. A click
  on a file opens it in the file editor (below); a path can also be typed into the terminal or revealed. Nothing is
  ever run from it (a project file could be a script).
- `TerminalView::worktree` (set where the branch is) feeds the purple working-tree chip of the status bar.

## File editor

- `crates/agentty-app/src/editor/` is a small code editor for files opened from the files panel.
  `workbench/editor_host.rs` puts it in place of the terminals while one of its files is shown and adds a tab per open
  file after the terminal tabs; picking a terminal tab brings the terminals back, and the files stay open (unsaved
  changes included) until their tabs close. Closing a tab, the window or the app with unsaved files asks first
  (`crate::request_quit` for every way of quitting).
- `buffer.rs` holds the text as lines with the cursor, selection and undo history (typing and backspacing are joined
  into steps; an input method's composition becomes one step when committed). `element.rs` shapes and paints only the
  lines on screen and keeps their geometry for mouse hits and IME popups.
- `highlight.rs` colors lines with `syntect` and bat's grammars (`two-face`, pure-Rust regex engine) in a Dark+ palette.
  Lines are highlighted top-down with the parser state cached per line, a few milliseconds per frame, so an edit
  re-reads only from the changed line to the screen. Their licenses are in `assets/licenses/syntax-definitions.md`
  (kept in sync by a test).
- `file.rs` reads files (binary files and files over 20 MB are not opened; over 2 MB, with very long lines or not UTF-8
  they are read-only) and saves atomically: a temporary file in the same folder, flushed, given the original's
  permissions, renamed over it (a file with other hard links is rewritten in place). A link is written through to its
  target; a target outside the project is read-only until the user allows editing. Line endings and a BOM are kept.
  Open files are checked every 2 s: unchanged ones reload, ones with unsaved changes ask; saving over a file that
  changed on disk asks too.
- `format.rs` runs the installed formatter over the text (stdin → stdout, argument list, no shell): Prettier,
  google-java-format, ktlint, rustfmt, gofmt, Black. It never runs project code: formatters inside the project are
  ignored; Prettier always gets Agentty's own copy of the project's plain formatting options (read from JSON, TOML or
  flat YAML, allowlisted — `plugins`, `overrides`, `parser` are dropped), and settings that are code
  (`prettier.config.js`, a shared config package) or can't be read as plain data make it refuse; other data settings
  are passed by path; the formatter starts in `data_dir()/formatter` with only absolute `PATH` folders, so nothing
  picks a program by the project folder (rustup's toolchain file, version-manager shims).
- `external.rs` opens the file in VS Code / Cursor (`--goto file:line:col`, the CLI inside the app bundle when it is not
  on `PATH`) or the system's text editor (`open -t`, Notepad, an installed desktop text editor on Linux — never
  `xdg-open`, which can hand a script to the program that runs it).
- Saving resolves the path again (a folder may have become a link since), refuses a target outside the project unless
  editing it was allowed, and asks about files changed on disk, also when saving everything before quitting. An
  update's relaunch waits until no file has unsaved changes.

## Docker panel

- The panel (like the plugin panel) is resizable: `workbench/side_panels.rs` draws the handle at its left edge and
  keeps the width (`dockerPanelWidth`, `pluginPanelWidth`) between 240 px and whatever leaves the terminals 420 px.
- `agentty_bridge::docker` looks at the project (working-tree root) of the active pane: a compose file
  (`compose.yaml`, `compose.yml`, `docker-compose.yml`, `docker-compose.yaml`) or a `Dockerfile` at its root, and its
  containers from `docker ps --all --format json`. Containers whose compose labels
  (`com.docker.compose.project.working_dir`) point into the project count even when the compose file lives in a
  subfolder or was passed with `-f` — commands for them reuse the project name and config files from those labels.
  Without compose, containers whose image or name is the project folder's name are shown.
- Services come from `docker compose ps --all --format json` (one JSON object per line, or one array before compose
  2.21) merged with `docker compose config --services`, so a service without a container is listed as "not created".
  Plain `config` is never run: it prints the environment resolved, and compose files and `.env` often hold passwords.
  Only names, images, states, ports and Docker's status text are kept.
- Every command is an argv array (no shell). Names reach a command only after Docker reported them and they passed
  `docker::valid_name` (letters, digits, `.`, `_`, `-`, not starting with `-`). `down` has no `-v`, and the panel asks a
  second time before running it. Errors are cut to one line, and a line that mentions the environment or an
  interpolation is replaced by a generic note.
- The `docker` program is looked up on `PATH` and in the folders Docker Desktop, Homebrew, Rancher Desktop, OrbStack and
  snap install to (a GUI app on macOS doesn't get the login shell's `PATH`); commands get that `PATH` too, so the
  compose plugin and credential helpers are found. Queries time out after 15 s, actions after 10 minutes.
- `workbench/docker_panel.rs` asks once when the active pane's project changes and when the window comes to the front
  (this feeds the status bar chip), and every 4 s only while the panel is open — always on a background thread. The
  panel is a fixed-width column left of the files panel. Logs open in a new terminal tab running
  `docker compose logs -f --tail 200 <service>` (or `docker logs -f` for a plain container).
- Debug driver: `docker` toggles the panel; `docker refresh|up|down|start:<svc>|stop:<svc>|restart:<svc>|logs:<svc>`,
  and `probe` includes the panel's state.

## Local servers

`workbench/servers.rs` samples listening TCP ports under each pane's shell every few seconds (`procinfo::listeners`,
`lsof` + `ps`). A new port of the visible workspace is probed with `GET /` on loopback and, if HTML comes back and links
open in-app, opened in the in-app browser. When a pane closes, the shell's process tree is read *before* the pane is
dropped, and whatever in it still listens gets `SIGTERM`, then `SIGKILL` three seconds later.

## Monitoring → Proxy

`capture.rs` is a forward proxy on `127.0.0.1` (std threads, no TLS code). Panes started while capture is on get
`HTTPS_PROXY` / `HTTP_PROXY` of the form `http://pane-<id>:<token>@127.0.0.1:<port>`: the user name attributes a
connection to its pane, the per-run random token keeps every other local process out (407). `CONNECT` is tunnelled
untouched — no certificate, no decryption: host, bytes and timing only. Plain HTTP is forwarded one request per
connection and records method, path (query values masked) and status, never headers or bodies. Records are a bounded
in-memory ring. Once started the listener lives as long as the app, because panes keep pointing at it; stopping capture
only stops recording. An upstream proxy from the app's own environment is chained.

## Status bar layout and onboarding

`hud.rs` holds the order and visibility of the AI CLI status bar items (`settings.hud`, read leniently so an unknown
item never fails the settings file); `workbench/layout.rs` renders both the bar and split pane headers from it.
`workbench/onboarding.rs` shows a welcome and a settings step as a dialog on first run, then a tour card whose tasks
are ticked off by hooks in the features themselves (`onboarding_note`).

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
