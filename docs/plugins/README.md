# Agentty plugin guide

Plugins connect Agentty with other apps and add tools to the terminal: a panel next to your
terminals, buttons above agent panes, command palette entries, and `agentty://` links other apps
can open. The first plugin, **Cosmica**, turns notes into prompts and files session summaries back
into Cosmica.

A plugin is a folder with a manifest and a program. Agentty starts the program when the plugin is
first used and talks to it over stdin/stdout (JSON-RPC 2.0, one JSON object per line). Plugins can be
written in any language; the Node.js SDK makes it a few lines.

- [Quick start](#quick-start)
- [Manifest](#manifest-agentty-pluginjson)
- [Node.js SDK](#nodejs-sdk)
- [Rust and WebAssembly](#rust-and-webassembly)
- [Panel UI](#panel-ui)
- [Context](#context)
- [Sending prompts](#sending-prompts)
- [Sessions and terminals](#sessions-and-terminals)
- [Links from other apps](#links-from-other-apps)
- [Permissions and safety](#permissions-and-safety)
- [Developing, debugging and sharing](#developing-debugging-and-sharing)
- [Protocol for other languages](protocol.md)
- [How people use plugins](usage.md)

## Quick start

**With AI** — open **Plugins** (puzzle icon in the activity bar), enter a name and what the plugin
should do under *Build your own*, and press **Create and Build with Claude Code**. Agentty creates the
plugin from a template (SDK, types, this guide and a `CLAUDE.md` / `AGENTS.md` with instructions) and
opens Claude Code in its folder. You can also copy the prompt with **Copy AI Prompt** and use it with
any agent.

**By hand** — a plugin folder looks like this:

```
~/.agentty/plugins/hello/
├── agentty-plugin.json   manifest
├── main.mjs              the plugin program
└── agentty-plugin.mjs    the SDK (copy from ~/.agentty/plugins/.sdk or sdk/node in the repo)
```

```json
{
  "id": "hello",
  "name": "Hello",
  "version": "0.1.0",
  "main": "main.mjs",
  "permissions": ["prompt.inject"],
  "contributes": {
    "panel": { "title": "Hello", "icon": "sparkles" },
    "commands": [{ "id": "hello.explain", "title": "Hello: Explain this folder", "icon": "bot", "paneBar": true }]
  }
}
```

```js
import { createPlugin, ui } from './agentty-plugin.mjs';

const plugin = createPlugin();

plugin
  .onPanelOpen((context) =>
    plugin.setPanel(
      ui.column([
        ui.text('Hello', 'title'),
        ui.text(context.pane ? `You are in ${context.pane.cwd}` : 'No terminal focused', 'muted'),
        ui.button('explain', 'Explain this folder', { icon: 'bot', variant: 'primary' }),
      ]),
    ),
  )
  .onEvent('explain', (_event, context) => explain(context))
  .command('hello.explain', ({ context }) => explain(context))
  .start();

function explain(context) {
  return plugin.injectPrompt({ text: 'Give me a short tour of this project.', cwd: context.pane?.cwd, target: 'ask' });
}
```

Open **Plugins → Refresh** (or reopen the page): the plugin shows up as installed. Its panel button
appears in the tab strip, the command in the palette (⇧⌘P) and as a button above terminal panes.

## Manifest (`agentty-plugin.json`)

| Field | | |
|---|---|---|
| `id` | required | 2–40 characters `a-z 0-9 -`; must equal the folder name |
| `name`, `version` | required | shown in the store; `version` is `major.minor.patch` |
| `main` | required | entry point, relative to the plugin folder |
| `runtime` | `node` | `node` (Node.js 18+ from the login shell PATH), `python` (`python3 main`), `executable`, or `wasm` (see [Rust and WebAssembly](#rust-and-webassembly)) |
| `apiVersion` | `1` | plugin API version the plugin was written for |
| `description`, `publisher`, `homepage`, `keywords` | | store listing; `homepage` must be `https://` |
| `links` | `[]` | up to 6 `{ "label", "url" }` (https) shown as buttons on the store card — project site, docs, source |
| `requires` | | `{ "name", "url", "note" }`: the app or service the plugin is for. The card says whether it was found (see `detect`) and offers the link when it wasn't |
| `icon` | | icon name (see [Icons](#icons)) |
| `permissions` | `[]` | see [Permissions](#permissions-and-safety) |
| `activationEvents` | `[]` | `["onStartup"]` starts the plugin with Agentty; otherwise on first use |
| `detect` | `[]` | paths (`~` allowed) of an app the plugin integrates with; found → "Recommended" in the store |
| `contributes.panel` | | `{ "title", "icon", "surface", "mode" }` — the panel the plugin fills with UI. `surface` picks where its icon sits: `pane` (default, the tab strip above the terminals), `sidebar` (the activity bar on the left) or `status` (the status bar at the bottom). `mode` picks how it opens: `push` (default, docked beside the terminals), `overlay` (floating over them), `window` (a window of its own) or `full` (the whole area). The user can change the mode and their choice is kept |
| `contributes.commands[]` | | `{ "id", "title", "description", "icon", "paneBar", "when", "palette" }` |

Commands appear in the command palette (unless `"palette": false`). With `"paneBar": true` they also
get an icon button in the status bar above Claude Code / Codex panes and in the header of split panes;
`when` limits that to `agent` panes, `shell` panes, or `always`.

## Node.js SDK

`agentty-plugin.mjs` has no dependencies. Types are in `agentty-plugin.d.ts`.

```js
const plugin = createPlugin();
plugin.start(); // after registering handlers
```

**Handlers** (all may be async; errors are logged and shown as a notification):

| | called when |
|---|---|
| `onActivate(info => …)` | the plugin started; `info` has `plugin.dataDir`, `language`, `context` |
| `command(id, ({ context, args }) => …)` | a command runs (palette, pane bar) |
| `onPanelOpen(context => …)` / `onPanelClose` | the panel is shown / hidden — render in `onPanelOpen` |
| `onEvent(elementId, (event, context) => …)` | a UI element with that id was used |
| `onAnyEvent((event, context) => …)` | any UI event not handled by `onEvent` |
| `onContextChange(context => …)` | the focused pane, its status or folder changed |
| `onUrl(path, ({ path, query, url }) => …)` | `agentty://plugin/<id>/<path>?…` was opened |
| `onShutdown(() => …)` | Agentty stops the plugin |

**Calls** (return promises):

| | permission |
|---|---|
| `setPanel(tree)` — replace the panel content | |
| `showPanel()` — open this plugin's panel | |
| `notify(message, kind)` — `info` · `success` · `warning` · `error` | |
| `setBadge(text)` — short text on the plugin's tab-strip button | |
| `getContext()` | |
| `openUrl(url)` (http/https) | |
| `revealPath(path)` — show a file in Finder | `workspace.read` |
| `injectPrompt(request)` — see [Sending prompts](#sending-prompts) | `prompt.inject` |
| `sendToTerminal({ paneId, text, submit })` | `terminal.write` |
| `getSession({ paneId, maxTurns })` | `session.read` |
| `listWorkspaces()` | `workspace.read` |
| `log(...)` — writes to the plugin log (stderr) | |

`plugin.info` holds the `initialize` data; `plugin.context` is the latest context. Environment
variables: `AGENTTY_PLUGIN_ID`, `AGENTTY_PLUGIN_DIR`, `AGENTTY_PLUGIN_DATA` (private folder for
settings and caches), `AGENTTY_VERSION`, `AGENTTY_LANGUAGE`, `AGENTTY_BIN`.

> Never write to stdout yourself (`console.log`): stdout carries the protocol. Use `plugin.log()` or
> `console.error()`.

## Rust and WebAssembly

A plugin can also be a compiled program. `"runtime": "wasm"` makes `main` a `.wasm` module that
Agentty runs inside itself, so one file works on macOS, Windows and Linux and the plugin reaches
nothing of yours: no files, no processes, no network of its own. Everything goes through the
protocol, where the permissions above are checked.

The Rust SDK is `sdk/rust` in the Agentty repository:

```toml
# Cargo.toml
[lib]
crate-type = ["cdylib"]

[dependencies]
agentty-plugin = { path = "…/sdk/rust" }
```

```rust
use agentty_plugin::{export_plugin, ui, Host, Plugin, UiEvent};

#[derive(Default)]
struct Hello {
    clicks: u32,
}

impl Plugin for Hello {
    fn panel_open(&mut self, host: &Host) {
        host.set_panel(ui::column(vec![
            ui::text(format!("Clicked {} times", self.clicks)),
            ui::button("go", "Click me"),
        ]));
    }

    fn ui_event(&mut self, host: &Host, event: UiEvent) {
        if event.element == "go" {
            self.clicks += 1;
            self.panel_open(host);
        }
    }
}

export_plugin!(Hello);
```

```sh
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/hello.wasm hello.wasm   # next to agentty-plugin.json
```

Then **Plugins → Install from Folder…** and pick the folder. Two are in the repository:
`plugins/hello-rust` (a panel and a counter, no permissions at all) and `plugins/agent-rest-client`
(an HTTP client with environments, a collection and a proxy, `net.request`). The second ships
inside Agentty — its module is committed and installs from the Plugins page in one click — so its
folder is also the worked example of a plugin that is more than a demo. The wire format and the
module's ABI are in [the protocol](protocol.md#webassembly-plugins).

## Panel UI

The panel is the area a plugin owns (360 px wide, scrolls vertically). Plugins describe it as a tree
and Agentty draws it natively, so it matches the app and needs no web view. Send a new tree whenever
something changes; text fields keep what the user typed unless you send a different `value`.

| Builder | Element | Events |
|---|---|---|
| `ui.column(children, { gap })` / `ui.row(children, { gap, wrap })` | layout; gap `none` `small` `medium` `large` | |
| `ui.section(title, children)` | titled group | |
| `ui.text(text, style)` | `body` `title` `muted` `small` `code` `error` `success` | |
| `ui.button(id, label, { icon, variant, disabled })` | `primary` `secondary` `ghost` `danger` | `click` |
| `ui.input(id, { placeholder, value })` | single-line field | `change` (after typing pauses), `submit` (Enter); `event.value` is the text |
| `ui.list(id, items, { empty })` | rows `{ id, title, subtitle, detail, icon, actions: [{ id, icon, label, tooltip }] }` | `select` (row, `event.item`), `action` (`event.item`, `event.action`) |
| `ui.choice(id, [{ value, label }], value)` | segmented choice | `change` with the value |
| `ui.toggle(id, label, value)` | switch | `change` with the new boolean |
| `ui.badge(text, tone)` | `neutral` `info` `success` `warning` `error` | |
| `ui.spinner(text)` · `ui.divider()` | | |

Limits: 2 000 elements, 12 levels, 20 000 characters per text. Null/false children are skipped, so
`cond && ui.text(…)` works. Panel updates are drawn at most every 50 ms, notifications at most one
per 700 ms, and a plugin that sends more than 240 messages a second is stopped as a runaway.

### Icons

Use these names for `icon` fields (anything else shows a puzzle piece):
`app-window arrow-down arrow-left arrow-right arrow-up arrow-up-right at-sign bell bell-dot blocks book-open bookmark bot brain bug calendar chart-column check chevron-down chevron-right chevron-up circle-check circle-dot circle-pause circle-x clipboard clipboard-paste clock cloud code columns-2 command container copy database download ellipsis external-link eye file-input file-plus file-text folder folder-open folder-plus git-branch git-commit-horizontal git-pull-request globe grip-vertical hammer hash history house image info key-round layout-panel-left lightbulb link list list-tree loader-circle mail maximize-2 message-circle-question message-square minimize-2 minus network notebook notebook-pen package panel-left-close panel-left-open pencil picture-in-picture-2 play plug plus power puzzle refresh-cw rocket rotate-cw rows-2 save scroll-text search send settings shield-alert sparkles square square-plus square-terminal star sticky-note tag terminal trash-2 undo-2 unlink upload users wand-sparkles workflow wrench x zap git-fork file lock graduation-cap x-twitter`.

## Context

Every command, event and panel call carries the context of the focused window:

```json
{
  "workspace": { "id": 3, "name": "agentty", "cwd": "/Users/me/agentty", "active": true },
  "pane": {
    "id": 12, "kind": "claude", "tool": "claude", "title": "Claude Code",
    "cwd": "/Users/me/agentty", "sessionId": "…", "status": "idle", "running": true
  },
  "language": "ko"
}
```

What a plugin sees depends on what it declared: folders and names (`workspace.cwd`, `workspace.name`,
`pane.cwd`, `pane.title`) need `workspace.read`, and `pane.sessionId` needs `session.read`. Without
them the context still has ids, `kind`, `tool`, `status`, `running` and the language — enough to know
which pane is focused, not where the user works.

`kind` is `claude`, `codex` or `shell` (other CLIs run in `shell` panes; `tool` names them).
`status` is `idle`, `working` (a tool is running), `thinking` (a turn is open, the agent is between
tool calls), `finished`, `permission`, `question`, `interrupted`, `shell` or `exited`. Treat
`working` and `thinking` the same when deciding not to interrupt. Pane-bar commands get the context of the pane whose button was pressed.

## Sending prompts

```js
await plugin.injectPrompt({
  text: 'Continue the release checklist.',
  title: 'Release',          // workspace name for new sessions, dialog heading
  target: 'ask',             // ask | active | newWorkspace | newTab | pane | workspace
  agent: 'claude',           // claude | codex | shell (new sessions)
  cwd: '/Users/me/project',  // folder for new sessions
  submit: true,              // press Enter (agents only)
});
```

- `ask` (default) shows **Send to…**: the user sees the prompt and picks a new workspace, a new tab
  in the current workspace or an open workspace (an idle agent there gets it, otherwise a new agent
  tab opens), and Claude Code / Codex / Terminal.
- `active` types into the focused pane, `pane` into `paneId`, `workspace` into `workspaceId`,
  `newWorkspace` / `newTab` start a new session with the prompt.
- Terminals (`shell`) only ever get the text typed in — Enter is never pressed for `injectPrompt`.
- Prompts over 60 000 bytes are saved to `~/.agentty/prompts/` and the agent is asked to read the file.

Prefer `ask` for anything a user starts from another app or a link.

## Sessions and terminals

```js
const session = await plugin.getSession({ paneId: context.pane.id, maxTurns: 200 });
// { agent, sessionId, title, cwd, status, turnCount, turns: [{ role: 'user' | 'assistant', text }] }

await plugin.sendToTerminal({ paneId: context.pane.id, text: 'Summarize what we did.', submit: true });
```

Check `pane.status` before typing into an agent: don't interrupt `working`, `permission` or
`question`. To have the agent produce something (a summary, a report), ask it to write a file at a
path you choose and watch for that file — this is how the Cosmica plugin saves AI summaries.

## Links from other apps

Agentty registers `agentty://`:

| Link | Effect |
|---|---|
| `agentty://plugin/<id>/<path>?key=value` | starts plugin `<id>` and calls its `onUrl(path)` handler with the query |
| `agentty://prompt?text=…&title=…&agent=…&cwd=…` | opens **Send to…** with the text (`file=` attaches an absolute `.md`/`.txt` path) |
| `agentty://plugins/<id>` | opens the Plugins page at that plugin |

From a shell: `open "agentty://plugin/cosmica/continue?path=%2FUsers%2Fme%2FNote%2Fa.md"`. From
Electron: `shell.openExternal(url)`; from Swift: `NSWorkspace.shared.open(url)`. Encode every value
with `encodeURIComponent`. If the plugin is a built-in one that isn't installed, Agentty offers to
install it and then continues with the link.

Links can come from anywhere, including web pages. Validate every parameter (the Cosmica plugin only
opens `.md` files inside the Cosmica notes folder). For a minute after a link reaches a plugin,
Agentty routes that plugin's `injectPrompt` calls through **Send to…** and refuses `sendToTerminal` —
clicks in the panel the link opened don't lift this, so a link can't turn one click into typing
inside a terminal.

## Permissions and safety

| Permission | Allows |
|---|---|
| `prompt.inject` | `injectPrompt` |
| `terminal.write` | `sendToTerminal` — typing into open panes without asking |
| `session.read` | `getSession` — reading AI conversations |
| `workspace.read` | `listWorkspaces` |
| `net.request` | `net/fetch` — HTTP requests to addresses the plugin chooses |

`storage/get`, `storage/set` and `storage/keys` need no permission: they are the plugin's own
folder (`<data dir>/plugin-data/<id>/storage.json`, `0600`), up to 64 keys and a megabyte. A
WebAssembly plugin has no files of its own, so that is how it remembers anything.

The store shows these before installing. A call without its permission fails with code `-32001`.

A plugin with `runtime` `node`, `python` or `executable` runs as your user, with the same file and
network access as any program you start, so only install those if you trust them. A `wasm` plugin
does not: it reaches only what this protocol gives it, whatever its code says. The Plugins page
names which of the two a plugin is, under **About → Runs as**.

When writing one:

- Ask only for the permissions you use.
- Never read or send credentials. If you read another app's config, pick just the fields you need
  (the Cosmica plugin reads only `notes.path` and the server port from `~/.cosmica/config.json`).
- Keep data in `AGENTTY_PLUGIN_DATA`; create files other apps read with care (no overwrites).
- Talk to local services on `127.0.0.1` only unless the user configured otherwise.

## Developing, debugging and sharing

- **Restart** on the plugin card (or the ↻ button in its panel) picks up code changes.
- **Logs** on the card shows stderr, protocol errors, crashes and exit codes.
- **Link Folder for Development…** loads a plugin from its own folder (for example a Git checkout)
  without copying it; **Uninstall** only unlinks it.
- **Edit with Claude Code** opens a workspace in the plugin folder.
- Test the plugin without Agentty by driving its stdin — see `plugins/cosmica/test/plugin.test.mjs`
  in the Agentty repository for a fake host.
- Share a plugin as a Git repository with `agentty-plugin.json` at its root (users paste the
  `https://` URL into **Install from Git**) or as a folder (**Install from Folder…**). Bundle the SDK
  and any `node_modules` you need; Agentty does not run `npm install`.
- Built-in plugins ship inside Agentty and update with it.
