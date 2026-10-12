# Building an Agentty plugin — instructions for the AI agent

You are building a plugin for **Agentty**, a native terminal for AI coding agents (Claude Code,
Codex) on macOS, Windows and Linux. This folder is the plugin. Reply to the user in the language they write in.

## What you have

- `agentty-plugin.json` — the manifest. Keep it in sync with the code.
- `main.mjs` — the plugin program (Node.js 18+, ES modules). Start from the template in it.
- `agentty-plugin.mjs` — the SDK. **Do not edit it**; import from it.
- `agentty-plugin.d.ts` — SDK types: the exact API. Read it before writing code.
- `PLUGIN_GUIDE.md` — the full guide: manifest fields, UI elements, context, prompts, links,
  permissions. Read it first.

## How a plugin works

Agentty starts `node main.mjs` on first use and exchanges JSON-RPC messages over stdin/stdout. The
plugin can:

- fill a **panel** next to the terminals with a UI tree (`plugin.setPanel(ui.column([...]))`) and
  react to clicks, typing and list selections (`plugin.onEvent(id, …)`);
- add **commands** to the palette (`contributes.commands`, handled by `plugin.command(id, …)`);
- see where the user is (`context.pane.cwd`, `kind`, `status`, `sessionId`);
- **send prompts** to agents (`plugin.injectPrompt`, default `target: 'ask'` lets the user choose a
  new workspace, a tab or an open workspace);
- type into a pane (`plugin.sendToTerminal`), read an agent's conversation (`plugin.getSession`),
  list workspaces (`plugin.listWorkspaces`);
- handle **links** `agentty://plugin/<id>/<path>?…` from other apps (`plugin.onUrl(path, …)`).

## Rules

1. **Never write to stdout** (`console.log`, `process.stdout.write`) — it carries the protocol. Log
   with `plugin.log(...)` or `console.error(...)`.
2. Register every handler, then call `plugin.start()` once at the end of `main.mjs`.
3. Declare only the `permissions` you call: `prompt.inject` (injectPrompt), `terminal.write`
   (sendToTerminal), `session.read` (getSession), `workspace.read` (listWorkspaces).
4. The manifest `id` must equal the folder name; command ids should start with the plugin id
   (`myplugin.doThing`). Use icon names listed in PLUGIN_GUIDE.md.
5. Re-render the whole panel with `setPanel` whenever state changes; render on `onPanelOpen`. Keep
   state in module variables. Give every interactive element a unique `id`.
6. Treat link parameters and anything from other apps as untrusted: validate paths (stay inside the
   folder you expect), types and sizes. Prefer `target: 'ask'` for prompts.
7. Don't interrupt busy agents: check `context.pane.status` (`working`, `permission`, `question`)
   before `sendToTerminal`.
8. Never read, print or store credentials. If you read another app's config, take only the fields
   you need. Store plugin settings in `plugin.info.plugin.dataDir`.
9. Prefer Node's standard library (`node:fs/promises`, `node:path`, `fetch`). If you need a package,
   vendor it into the folder — Agentty doesn't run `npm install`.
10. Show user-facing text in the user's language when practical (`context.language`: en, ko, ja, zh).
11. Handle failures with `plugin.notify(message, 'error')` instead of crashing.

## Designing the panel

Agentty draws the panel from the tree you send, in its own colors and spacing, so a good-looking
panel comes from choosing the right element — there is no styling to do. `PLUGIN_GUIDE.md`
(**Panel UI → Making it look right**) has the full list; in short:

- things to pick → `list`; records with the same fields → `table`; one item's details →
  `keyValue` inside a `card`; a number that matters → `stat` (several in a `grid`); several views
  → `tabs`; a hint, warning or failure → `callout`; code or output → `code`; how far a job got →
  `progress`; settings → `input`, `select`, `checkbox`, `toggle` grouped in `section`s;
- one `primary` button per screen; `ghost` for Cancel; `danger` only for what cannot be undone;
- every `list` / `table` gets an `empty` text, every failure a `callout` with a way to retry, every
  slow action a `spinner` or `progress` right away;
- tones (`success`, `warning`, `error`, `info`) mean something — never use them as decoration;
- the panel starts 360 px wide: `grid` of 2 columns, `table` of 3–4 columns at most.

`card`, `grid`, `tabs`, `table`, `keyValue`, `stat`, `progress`, `callout`, `select`, `checkbox`
and `code` need `"apiVersion": 4` in `agentty-plugin.json`. The **UI Gallery** plugin (Plugins
page) shows every element and whole example screens; suggest it to the user when they want to see
the options.

## Workflow

1. Read `PLUGIN_GUIDE.md` and `agentty-plugin.d.ts`.
2. If the goal is unclear, ask the user what the plugin should do and which app or service it
   connects to.
3. Update `agentty-plugin.json` (name, description, icon, permissions, commands, panel, and
   `agents` — `["claude"]`, `["codex"]` or both — when the plugin is meant for particular AI
   agents; left out, it is shown as Claude Code).
4. Implement `main.mjs`. Split into modules if it grows (`import './lib.mjs'`).
5. Check syntax with `node --check main.mjs` (and other files). Optionally test by piping messages:
   `printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"plugin":{"id":"x","dataDir":"/tmp"},"context":{}}}' '{"jsonrpc":"2.0","method":"panel/open","params":{"context":{}}}' | node main.mjs`
   should print a `ui/setPanel` request.
6. Tell the user to press **Restart** on the plugin in Agentty's **Plugins** page (or ↻ in its
   panel), then try it. Errors show under **Logs** on the plugin card.
