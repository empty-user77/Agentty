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
| `wasm` | none — `<main>` is a WebAssembly module Agentty runs itself (see [WebAssembly plugins](#webassembly-plugins)) |

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
| `host/copy` | | `{ text }` (up to 100,000 characters) | `null` |
| `host/revealPath` | `workspace.read` | `{ path }` (absolute, existing) | `null` |
| `prompt/inject` | `prompt.inject` | `{ text, title?, target?, paneId?, workspaceId?, agent?, cwd?, submit? }` | `{ status: "asked" }` or `{ status: "sent", paneId }` |
| `terminal/send` | `terminal.write` | `{ paneId?, text, submit? }` (focused pane without `paneId`) | `{ paneId }` |
| `session/get` | `session.read` | `{ paneId?, maxTurns? }` (default 200, max 2000) | `{ paneId, agent, sessionId, title, cwd, status, turnCount, turns: [{ role, text }] }` |
| `workspace/list` | `workspace.read` | `{}` | `[{ id, name, cwd, active, panes: [pane] }]` |
| `net/fetch` | `net.request` | `{ url, method?, headers?, body?, timeoutMs?, proxy? }` | `{ status, statusText, url, headers, body, truncated, binary, bytes, durationMs }` |
| `storage/get` | | `{ key }` | `{ key, value }` (`value` is null when unset) |
| `storage/set` | | `{ key, value }` (null removes it) | `null` |
| `storage/keys` | | `{}` | `[key]` |

Agentty drops `ui/notify` calls that arrive faster than one per 700 ms (answering them normally), and
stops a plugin that sends more than 240 messages a second. Context fields are limited by the
plugin's permissions (see the guide).

`net/fetch` is the only way a plugin reaches the network, and Agentty bounds it: `GET`, `HEAD`,
`POST`, `PUT`, `PATCH`, `DELETE` and `OPTIONS` over `http` or `https`; at most 32 headers, none of
them `Host`, `Content-Length`, `Transfer-Encoding`, `Connection`, `Upgrade` or `Expect`, and none
carrying a line break; a request body up to 1 MB; 4 MB of the response (`truncated` says when more
arrived); a timeout of 15 s by default and 60 s at most; four requests in flight per plugin.
`proxy` is an `http://` or `https://` address (with `user:password@` when the proxy asks for it)
the request goes through. Nothing of yours travels with the request — no cookie, no stored
credential — only what the plugin puts in it. Each call is written to the plugin's log with the URL
redacted.

Up to 3 redirects are followed, and each address they name goes through the same checks as the one
the plugin asked for: `http` or `https`, and not a link-local or metadata address — otherwise a
server could answer `302 Location: http://169.254.169.254/…` and walk past them. An `Authorization`
or `Cookie` header is not carried to another host, and a redirected `POST`, `PUT` or `PATCH`
becomes a `GET` without its body unless the answer was `307` or `308`. `url` in the response is the
address the answer came from.

`storage/*` is what a plugin remembers between runs: one JSON document in its own folder
(`<data dir>/plugin-data/<plugin>/storage.json`, created `0600`), read and written by key. It goes
when the plugin is uninstalled — what a plugin kept is what it was given — and an update leaves it
alone. A file that is no longer readable as JSON is moved aside as `storage.corrupt.json` rather
than written over, so nothing the plugin had is lost without trace. Keys are
lower-case letters, digits, `.`, `-` and `_`; at most 64 of them, and a megabyte in total. A plugin
that runs as a process can write its own files instead; a WebAssembly plugin has no files, so this
is how it keeps anything.

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

## WebAssembly plugins

`"runtime": "wasm"` in `agentty-plugin.json` makes `main` a `.wasm` module that Agentty runs inside
itself, on an interpreter — one file that works on macOS, Windows and Linux.

A module can call the functions Agentty hands it and nothing else. There is no file, no socket, no
environment variable, no process and no clock beyond a counter, so a WebAssembly plugin cannot read
`~/.agentty`, your projects or your credentials however it is written. What it wants from Agentty
it asks for with the same messages a process plugin writes to stdout, and the permissions in the
manifest are checked the same way.

The module exports:

| Export | Meaning |
|---|---|
| `memory` | its linear memory (the standard export of a Rust or C module) |
| `agentty_alloc(len: i32) -> i32` | a buffer of `len` bytes for Agentty to write a message into |
| `agentty_on_message(ptr: i32, len: i32)` | one UTF-8 JSON message from Agentty |

and imports, from the module named `agentty`:

| Import | Meaning |
|---|---|
| `send(ptr: i32, len: i32)` | one UTF-8 JSON message to Agentty |
| `log(ptr: i32, len: i32)` | one line for the plugin's log |
| `now_ms() -> i64` | milliseconds since the Unix epoch |

Messages are the same JSON-RPC objects as over stdio, one per call, without the newline. A module
that imports anything else does not load. Agentty also refuses a module larger than 64 MB, caps its
memory at 64 MB and gives each message a budget of work: a plugin that does not return is stopped
with "did not finish in time", and one that sends more than 256 messages while handling a single
one is stopped as well.

The Rust SDK hides all of this: it is in the [marketplace repository](https://github.com/empty-user77/Agentty-Marketplace/tree/main/sdk/rust),
with [`hello-rust`](https://github.com/empty-user77/Agentty-Marketplace/tree/main/src/hello-rust) as a working example.

## Surfaces and how a panel opens

`contributes.panel.surface` says where the plugin's icon goes. A plugin picks one:

| `surface` | Where |
|---|---|
| `pane` (default) | the tab strip above the terminals |
| `sidebar` | the activity bar down the left edge, with Agentty's own pages |
| `status` | the status bar along the bottom |

`contributes.panel.mode` says how the panel opens. The user can change it from the panel's layout
button, and their choice is kept; this is what it does first:

| `mode` | |
|---|---|
| `push` (default) | docked beside the terminals, which move over to make room |
| `overlay` | floating above the window at its right edge; nothing else moves |
| `window` | a window of its own, which can be moved and resized |
| `full` | the whole area the terminals and pages use |

A docked panel never takes so much room that the rest of the window is squeezed: dragged past what
can be docked, it becomes an overlay.

## UI tree

Every node is an object with a `type`:

```
column   { children, gap? }                 gap: none | small | medium | large
row      { children, gap?, wrap? }
section  { title, children }
text     { text, style? }                   style: body | title | muted | small | code | error | success
button   { id, label, icon?, variant?, disabled? }   variant: primary | secondary | ghost | danger
input    { id, placeholder?, value?, rows? }
                                            rows > 1: a text area that many lines tall (max 24);
                                            Enter adds a line and a paste keeps its line breaks
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
