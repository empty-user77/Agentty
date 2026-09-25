---
title: Node.js SDK
description: Handlers, calls, the panel UI builders and the context object — the full surface of agentty-plugin.mjs.
---

`agentty-plugin.mjs` is a single file with no dependencies. Type definitions live next to it in `agentty-plugin.d.ts`. Press **Developer Guide** on the Plugins page to unpack both into `~/.agentty/plugins/.sdk/`.

> [!NOTE]
> A plugin written this way runs as a program on the user's machine, with Node.js 18+ on their `PATH`, and is installed from a folder or a Git repository. The [marketplace](/docs/plugin-publishing) lists WebAssembly modules only — for one of those, see [Rust and WebAssembly](/docs/plugin-rust).

```js
import { createPlugin, ui } from './agentty-plugin.mjs';

const plugin = createPlugin();
// register handlers…
plugin.start();
```

Register every handler before calling `start()`.

## Handlers

All handlers may be async. Errors are logged and shown to the user as a notification.

| Handler | Called when |
|---|---|
| `onActivate(info => …)` | The plugin started. `info` has `plugin.dataDir`, `language` and `context` |
| `command(id, ({ context, args }) => …)` | A command runs, from the command palette |
| `onPanelOpen(context => …)` | The panel became visible — render here |
| `onPanelClose(context => …)` | The panel was hidden |
| `onEvent(elementId, (event, context) => …)` | A UI element with that id was used |
| `onAnyEvent((event, context) => …)` | Any UI event not handled by `onEvent` |
| `onContextChange(context => …)` | The focused pane, its status or its folder changed |
| `onUrl(path, ({ path, query, url }) => …)` | `agentty://plugin/<id>/<path>?…` was opened |
| `onShutdown(() => …)` | Agentty is stopping the plugin |

## Calls

Every call returns a promise.

| Call | Permission |
|---|---|
| `setPanel(tree)` — replace the panel content | |
| `showPanel()` — open this plugin's panel | |
| `notify(message, kind)` — `info`, `success`, `warning`, `error` | |
| `setBadge(text)` — up to 8 characters on the tab-strip button | |
| `getContext()` | |
| `openUrl(url)` — http/https | |
| `copy(text)` — put text on the clipboard | |
| `revealPath(path)` — show a file in the file manager | `workspace.read` |
| `injectPrompt(request)` | `prompt.inject` |
| `sendToTerminal({ paneId, text, submit })` | `terminal.write` |
| `getSession({ paneId, maxTurns })` | `session.read` |
| `listWorkspaces()` | `workspace.read` |
| `fetch(request)` — an HTTP request | `net.request` |
| `log(...)` — writes to the plugin log (stderr) | |

The protocol also has `storage/get`, `storage/set` and `storage/keys` — a JSON document in the plugin's own folder, no permission needed — and, from API version 2, `host/timer` and `pane/status`. A Node plugin can write its own files instead, but storage works the same for both kinds. See [the protocol](/docs/plugin-protocol).

`plugin.info` holds the data from `initialize`; `plugin.context` is the latest context.

## The panel

The panel is 360 px wide and scrolls vertically. The plugin describes it as a tree and Agentty draws it natively, so it matches the application and needs no web view. Send a new tree whenever something changes — text fields keep what the user typed unless you send a different `value`.

| Builder | Element | Events |
|---|---|---|
| `ui.column(children, { gap })` / `ui.row(children, { gap, wrap })` | Layout. `gap`: `none`, `small`, `medium`, `large` | |
| `ui.section(title, children)` | Titled group | |
| `ui.text(text, style)` | `body`, `title`, `muted`, `small`, `code`, `error`, `success` | |
| `ui.button(id, label, { icon, variant, disabled })` | `primary`, `secondary`, `ghost`, `danger` | `click` |
| `ui.input(id, { placeholder, value, rows })` | Single-line field, or a text area with `rows` > 1 (max 24) | `change` after a pause, `submit` on Enter; `event.value` is the text |
| `ui.list(id, items, { empty })` | Rows `{ id, title, subtitle, detail, icon, tone, actions }` | `select` with `event.item`; row buttons send `action` with `event.item` and `event.action` |
| `ui.choice(id, [{ value, label }], value)` | Segmented choice | `change` with the value |
| `ui.toggle(id, label, value)` | Switch | `change` with the new boolean |
| `ui.badge(text, tone)` | `neutral`, `info`, `success`, `warning`, `error` | |
| `ui.spinner(text)` | | |
| `ui.divider()` | | |

Null and false children are skipped, so `condition && ui.text('…')` works.

```js
plugin.onPanelOpen(async (context) => {
  const notes = await search('');
  plugin.setPanel(
    ui.column([
      ui.input('q', { placeholder: 'Search notes' }),
      ui.list('notes', notes.map((n) => ({
        id: n.path,
        title: n.title,
        subtitle: n.folder,
        icon: 'notebook',
        actions: [{ id: 'insert', icon: 'send', tooltip: 'Insert into the focused pane' }],
      })), { empty: 'No notes yet' }),
    ]),
  );
});

plugin.onEvent('notes', (event, context) => {
  if (event.event === 'action' && event.action === 'insert') {
    return plugin.injectPrompt({ text: read(event.item), target: 'ask' });
  }
});
```

> [!NOTE]
> Limits: 2 000 elements, 12 levels deep, 20 000 characters per string — a `choice`'s options and a list item's buttons count as elements too. Panel updates are drawn at most every 50 ms and notifications at most one per 700 ms. A plugin sending more than 240 messages a second is stopped as a runaway.

## Context

Every command, event and panel call carries the context of the focused window:

```json
{
  "workspace": { "id": 3, "name": "agentty", "cwd": "/Users/me/agentty", "active": true },
  "pane": {
    "id": 12,
    "kind": "claude",
    "tool": "claude",
    "title": "Claude Code",
    "cwd": "/Users/me/agentty",
    "sessionId": "…",
    "status": "idle",
    "running": true
  },
  "language": "ko"
}
```

What a plugin sees depends on what it declared. Folder and name fields (`workspace.cwd`, `workspace.name`, `pane.cwd`, `pane.title`) need `workspace.read`, and `pane.sessionId` needs `session.read`. Without them the context still carries ids, `kind`, `tool`, `status`, `running` and the language — enough to know which pane is focused, not where the user works.

`kind` is `claude`, `codex` or `shell`; other agent CLIs run in `shell` panes and `tool` names them.

| `status` | Meaning |
|---|---|
| `idle` | Waiting for input |
| `working` | A tool is running |
| `thinking` | A turn is open, between tool calls |
| `finished` | The turn ended |
| `permission` | Asking permission |
| `question` | Asking the user something |
| `interrupted` | Stopped by the user |
| `shell` | A plain shell |
| `exited` | The program ended |

Treat `working` and `thinking` the same when deciding not to interrupt.

## Sending prompts

```js
await plugin.injectPrompt({
  text: 'Continue the release checklist.',
  title: 'Release',          // workspace name for new sessions, and the dialog heading
  target: 'ask',             // ask | active | newWorkspace | newTab | pane | workspace
  agent: 'claude',           // claude | codex | shell, for new sessions
  cwd: '/Users/me/project',  // folder for new sessions
  submit: true,              // press Enter (agents only)
});
```

- `ask` (the default) shows **Send to…**, where the user sees the prompt and picks the destination.
- `active` types into the focused pane, `pane` into `paneId`, `workspace` into `workspaceId`, and `newWorkspace` / `newTab` start a new session with the prompt.
- Terminals only ever get the text typed in — Enter is never pressed for `injectPrompt`.
- Prompts over 60 000 bytes are saved to `~/.agentty/prompts/` and the agent is asked to read the file.

Prefer `ask` for anything the user starts from another app or from a link.

## Sessions and terminals

```js
const session = await plugin.getSession({ paneId: context.pane.id, maxTurns: 200 });
// { agent, sessionId, title, cwd, status, turnCount, turns: [{ role: 'user' | 'assistant', text }] }

await plugin.sendToTerminal({
  paneId: context.pane.id,
  text: 'Summarize what we did.',
  submit: true,
});
```

Check `pane.status` before typing into an agent: do not interrupt `working`, `permission` or `question`.

## Next

- [Protocol](/docs/plugin-protocol) — the same surface without the SDK
- [Permissions](/docs/plugin-permissions)
