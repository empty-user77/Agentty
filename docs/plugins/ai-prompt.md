# Building an Agentty plugin — instructions for the AI agent

You are building a plugin for **Agentty**, a macOS terminal for AI coding agents (Claude Code,
Codex). This folder is the plugin. Reply to the user in the language they write in.

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

## Workflow

1. Read `PLUGIN_GUIDE.md` and `agentty-plugin.d.ts`.
2. If the goal is unclear, ask the user what the plugin should do and which app or service it
   connects to.
3. Update `agentty-plugin.json` (name, description, icon, permissions, commands, panel).
4. Implement `main.mjs`. Split into modules if it grows (`import './lib.mjs'`).
5. Check syntax with `node --check main.mjs` (and other files). Optionally test by piping messages:
   `printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"plugin":{"id":"x","dataDir":"/tmp"},"context":{}}}' '{"jsonrpc":"2.0","method":"panel/open","params":{"context":{}}}' | node main.mjs`
   should print a `ui/setPanel` request.
6. Tell the user to press **Restart** on the plugin in Agentty's **Plugins** page (or ↻ in its
   panel), then try it. Errors show under **Logs** on the plugin card.
