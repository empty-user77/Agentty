# Agentty plugin protocol (API version 1)

For writing plugins without the Node.js SDK. Read the [plugin guide](README.md) first; this page
only describes the wire format.

## Transport

Agentty starts the plugin with its folder as the working directory:

| `runtime` | command |
|---|---|
| `node` | `node <main>` (Node.js from the login shell PATH, Homebrew, Volta or nvm) |
| `python` | `python3 <main>` |
| `executable` | `<main>` |

Messages are [JSON-RPC 2.0](https://www.jsonrpc.org/specification) objects, **one per line**, UTF-8,
on stdin (Agentty → plugin) and stdout (plugin → Agentty). Lines longer than 16 MB are rejected.
Anything on stdout that isn't JSON is logged and ignored; stderr goes to the plugin log.

When stdin closes or `shutdown` arrives, exit. A plugin still running 1.5 s after `shutdown` gets
`SIGTERM`, and `SIGKILL` 1.5 s after that; when Agentty quits both follow immediately. Stderr is read
with the same 16 MB line limit as stdout.

## Agentty → plugin

| Message | Kind | `params` |
|---|---|---|
| `initialize` | request (answer it) | `{ apiVersion, agentty: { version }, plugin: { id, name, version, dir, dataDir }, language, context }` |
| `command/execute` | notification | `{ command, args, context }` |
| `panel/open` · `panel/close` | notification | `{ context }` |
| `ui/event` | notification | `{ element, event, value?, item?, action?, context }` |
| `context/changed` | notification | `{ context }` |
| `url/open` | notification | `{ path, query, url, context }` |
| `shutdown` | notification | `{}` |

`initialize` is sent first, followed immediately by whatever started the plugin (a command, the panel
opening, a link). Answer `initialize` with any result, e.g. `{}`.

## Plugin → Agentty

Send these as requests (with an `id`) to get a result or an error, or as notifications (no `id`)
when you don't care.

| Method | Permission | `params` | Result |
|---|---|---|---|
| `ui/setPanel` | | `{ tree }` | `null` |
| `ui/showPanel` | | `{}` | `null` |
| `ui/notify` | | `{ message, kind: "info" \| "success" \| "warning" \| "error" }` | `null` |
| `ui/setBadge` | | `{ text }` (max 8 characters) | `null` |
| `context/get` | | `{}` | context |
| `host/info` | | `{}` | `{ version, apiVersion, language }` |
| `host/openUrl` | | `{ url }` (http/https) | `null` |
| `host/revealPath` | `workspace.read` | `{ path }` (absolute, existing) | `null` |
| `prompt/inject` | `prompt.inject` | `{ text, title?, target?, paneId?, workspaceId?, agent?, cwd?, submit? }` | `{ status: "asked" }` or `{ status: "sent", paneId }` |
| `terminal/send` | `terminal.write` | `{ paneId?, text, submit? }` (focused pane without `paneId`) | `{ paneId }` |
| `session/get` | `session.read` | `{ paneId?, maxTurns? }` (default 200, max 2000) | `{ paneId, agent, sessionId, title, cwd, status, turnCount, turns: [{ role, text }] }` |
| `workspace/list` | `workspace.read` | `{}` | `[{ id, name, cwd, active, panes: [pane] }]` |

Agentty drops `ui/notify` calls that arrive faster than one per 700 ms (answering them normally), and
stops a plugin that sends more than 240 messages a second. Context fields are limited by the
plugin's permissions (see the guide).

Errors use these codes:

| Code | Meaning |
|---|---|
| `-32601` | unknown method |
| `-32602` | invalid parameters (bad UI tree, no such pane, …) |
| `-32001` | permission missing, or blocked for a minute after a link reached the plugin |
| `-32002` | unavailable (no window open, no session yet) |

## Example session

```
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"apiVersion":1,"plugin":{"id":"hello",…},"language":"en","context":{…}}}
→ {"jsonrpc":"2.0","method":"panel/open","params":{"context":{…}}}
← {"jsonrpc":"2.0","id":1,"result":{}}
← {"jsonrpc":"2.0","id":1,"method":"ui/setPanel","params":{"tree":{"type":"column","children":[{"type":"button","id":"go","label":"Go"}]}}}
→ {"jsonrpc":"2.0","id":1,"result":null}
→ {"jsonrpc":"2.0","method":"ui/event","params":{"element":"go","event":"click","context":{…}}}
← {"jsonrpc":"2.0","id":2,"method":"prompt/inject","params":{"text":"Hello","target":"ask"}}
→ {"jsonrpc":"2.0","id":2,"result":{"status":"asked"}}
```

(`→` Agentty to plugin, `←` plugin to Agentty. Request ids are per direction.)

## UI tree

Every node is an object with a `type`:

```
column   { children, gap? }                 gap: none | small | medium | large
row      { children, gap?, wrap? }
section  { title, children }
text     { text, style? }                   style: body | title | muted | small | code | error | success
button   { id, label, icon?, variant?, disabled? }   variant: primary | secondary | ghost | danger
input    { id, placeholder?, value? }
list     { id, items: [{ id, title, subtitle?, detail?, icon?, tone?, actions?: [{ id, label?, icon?, tooltip? }] }], empty? }
                                            item tone colors its icon (same values as badge)
choice   { id, options: [{ value, label }], value? }
toggle   { id, label, value? }
badge    { text, tone? }                    tone: neutral | info | success | warning | error
spinner  { text? }
divider  {}
```

Events: `button` → `click`; `input` → `change` / `submit` with `value`; `list` → `select` with
`item`, row buttons → `action` with `item` and `action`; `choice` → `change` with the option value;
`toggle` → `change` with the new boolean.
