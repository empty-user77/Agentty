---
title: Plugin protocol
description: The JSON-RPC wire format behind the SDKs, the WebAssembly module ABI, and every message, limit and error code.
---

The wire format, for writing a plugin without an SDK. The [Node.js SDK](/docs/plugin-sdk) and the [Rust SDK](/docs/plugin-rust) wrap all of it; read the [quick start](/docs/plugin-quickstart) first either way.

API version **1** is everything on this page except `host/timer` and `pane/status`, which are version **2**.

## Transport

Agentty starts the plugin with the plugin folder as the working directory:

| `runtime` | Command |
|---|---|
| `node` | `node <main>` — Node.js from the login shell `PATH`, Homebrew, Volta or nvm |
| `python` | `python3 <main>` |
| `executable` | `<main>` |
| `wasm` | none — `<main>` is a WebAssembly module Agentty runs itself, see [WebAssembly plugins](#webassembly-plugins) |

Messages are [JSON-RPC 2.0](https://www.jsonrpc.org/specification) objects, **one per line**, UTF-8, on stdin (Agentty → plugin) and stdout (plugin → Agentty). Lines longer than 16 MB are rejected. Anything on stdout that is not JSON is logged and ignored; stderr goes to the plugin log.

When stdin closes or `shutdown` arrives, exit. A plugin still running 1.5 seconds after `shutdown` gets `SIGTERM`, and `SIGKILL` 1.5 seconds after that. When Agentty quits, both follow immediately.

## Agentty → plugin

| Message | Kind | `params` |
|---|---|---|
| `initialize` | request — answer it | `{ apiVersion, agentty: { version }, plugin: { id, name, version, dir, dataDir }, language, context }` |
| `command/execute` | notification | `{ command, args, context }` |
| `panel/open` · `panel/close` | notification | `{ context }` |
| `ui/event` | notification | `{ element, event, value?, item?, action?, context }` |
| `context/changed` | notification | `{ context }` |
| `url/open` | notification | `{ path, query, url, context }` |
| `pane/status` | notification | `{ paneId, status, running, agent, title, cwd }` — a pane this plugin started changed what it is doing (`workspace.read`, API 2) |
| `shutdown` | notification | `{}` |

`initialize` is sent first, followed immediately by whatever started the plugin — a command, the panel opening, or a link. Answer `initialize` with any result, for example `{}`.

## Plugin → Agentty

Send these as requests (with an `id`) to get a result or an error, or as notifications (no `id`) when you do not care.

| Method | Permission | `params` | Result |
|---|---|---|---|
| `ui/setPanel` | | `{ tree }` | `null` |
| `ui/showPanel` | | `{}` | `null` |
| `ui/notify` | | `{ message, kind }` — `info`, `success`, `warning`, `error` | `null` |
| `ui/setBadge` | | `{ text }`, max 8 characters | `null` |
| `context/get` | | `{}` | The context |
| `host/info` | | `{}` | `{ version, apiVersion, language }` |
| `host/openUrl` | | `{ url }` — http/https | `null` |
| `host/copy` | | `{ text }` — up to 100,000 characters | `null` |
| `host/timer` | | `{ ms }` — API 2 | `{ elapsedMs }`, once the time has passed |
| `host/revealPath` | `workspace.read` | `{ path }` — absolute, existing | `null` |
| `prompt/inject` | `prompt.inject` | `{ text, title?, target?, paneId?, workspaceId?, agent?, cwd?, submit? }` | `{ status: "asked" }` or `{ status: "sent", paneId }` |
| `terminal/send` | `terminal.write` | `{ paneId?, text, submit? }` — focused pane without `paneId` | `{ paneId }` |
| `session/get` | `session.read` | `{ paneId?, maxTurns? }` — default 200, max 2000 | `{ paneId, agent, sessionId, title, cwd, status, turnCount, turns }` |
| `workspace/list` | `workspace.read` | `{}` | `[{ id, name, cwd, active, panes }]` |
| `net/fetch` | `net.request` | `{ url, method?, headers?, body?, timeoutMs?, proxy? }` | `{ status, statusText, url, headers, body, truncated, binary, bytes, durationMs }` |
| `storage/get` | | `{ key }` | `{ key, value }` — `value` is null when unset |
| `storage/set` | | `{ key, value }` — null removes it | `null` |
| `storage/keys` | | `{}` | `[key]` |

Agentty drops `ui/notify` calls arriving faster than one per 700 ms, answering them normally, and stops a plugin that sends more than 240 messages a second. Context fields are limited by the plugin's permissions.

### Reaching the network

`net/fetch` is the only way a plugin reaches the network. [Permissions](/docs/plugin-permissions#what-net-request-is-allowed) has the bounds Agentty puts on it — methods, headers, sizes, timeouts, redirects — and the short version is that nothing of yours travels with the request: no cookie, no stored credential, only what the plugin put in it.

### Waiting, and hearing that an agent finished

`host/timer` is how a plugin waits: a request answered once the time has passed. 100 ms at the shortest, an hour at the longest, eight at a time. A module runs only while it is handling a message, so this is the whole of how it comes back to something later — answering it is all the plugin gets, which is why it is not a way to run in the background.

`pane/status` is how a plugin hears that an agent it started has finished. A plugin learns a pane id from `prompt/inject` (`{ status: "sent", paneId }`); Agentty remembers which plugin started which pane and tells only that plugin, when that pane's status changes — `working`, `idle`, `finished`, `permission`, `question`, `interrupted`, `exited`, or `closed` once. At most 32 panes are followed at a time.

A prompt the user placed themselves is followed too: `target: "ask"` answers `{ status: "asked" }` with no pane id, because there is none yet, and the session the user picks is watched all the same — its first `pane/status` is where the plugin learns which pane it became. A plugin with more than one question outstanding tells them apart by `title`, which is the one it gave the prompt.

Status arrives whether or not anything is being drawn: a window behind another, or one on a locked screen, is not drawn, and a plugin waiting for an agent must not be waiting for the user to come back. [AgentOS plugins](/docs/plugin-agentos) are built on these two.

### Remembering things

`storage/*` is what a plugin remembers between runs: one JSON document in its own folder (`<data dir>/plugin-data/<plugin>/storage.json`, created `0600`), read and written by key. Keys are lower-case letters, digits, `.`, `-` and `_`; at most 64 of them, and a megabyte in total. A plugin that runs as a process can write its own files instead; a WebAssembly plugin has none, so this is how it keeps anything.

### Error codes

| Code | Meaning |
|---|---|
| `-32601` | Unknown method |
| `-32602` | Invalid parameters — bad UI tree, no such pane, … |
| `-32001` | Permission missing, or refused because a link reached the plugin — until it restarts |
| `-32002` | Unavailable — no window open, no session yet |

## An example exchange

```
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"apiVersion":1,"plugin":{"id":"hello"},"language":"en","context":{}}}
→ {"jsonrpc":"2.0","method":"panel/open","params":{"context":{}}}
← {"jsonrpc":"2.0","id":1,"result":{}}
← {"jsonrpc":"2.0","id":1,"method":"ui/setPanel","params":{"tree":{"type":"column","children":[{"type":"button","id":"go","label":"Go"}]}}}
→ {"jsonrpc":"2.0","id":1,"result":null}
→ {"jsonrpc":"2.0","method":"ui/event","params":{"element":"go","event":"click","context":{}}}
← {"jsonrpc":"2.0","id":2,"method":"prompt/inject","params":{"text":"Hello","target":"ask"}}
→ {"jsonrpc":"2.0","id":2,"result":{"status":"asked"}}
```

`→` is Agentty to plugin, `←` is plugin to Agentty. Request ids are counted per direction.

## WebAssembly plugins

`"runtime": "wasm"` makes `main` a `.wasm` module that Agentty runs inside itself, on an interpreter — one file that works on macOS, Windows and Linux.

A module can call the functions Agentty hands it and nothing else. There is no file, no socket, no environment variable, no process and no clock beyond a counter, so a WebAssembly plugin cannot read `~/.agentty`, the user's projects or their credentials however it is written. What it wants from Agentty it asks for with the same messages a process plugin writes to stdout, and the permissions in the manifest are checked the same way.

The module exports:

| Export | Meaning |
|---|---|
| `memory` | its linear memory — the standard export of a Rust or C module |
| `agentty_alloc(len: i32) -> i32` | a buffer of `len` bytes for Agentty to write a message into |
| `agentty_on_message(ptr: i32, len: i32)` | one UTF-8 JSON message from Agentty |

and imports, from the module named `agentty`:

| Import | Meaning |
|---|---|
| `send(ptr: i32, len: i32)` | one UTF-8 JSON message to Agentty |
| `log(ptr: i32, len: i32)` | one line for the plugin's log |
| `now_ms() -> i64` | milliseconds since the Unix epoch |

Messages are the same JSON-RPC objects as over stdio, one per call, without the newline. **A module that imports anything else does not load.** Agentty also refuses a module larger than 64 MB, caps its memory at 64 MB, and gives each message a budget of work: a plugin that does not return is stopped with "did not finish in time", and one that sends more than 256 messages while handling a single one is stopped as well — `send` and `log` count together, so a loop that only logs is not free.

The [Rust SDK](/docs/plugin-rust) hides all of this.

## UI tree

Every node is an object with a `type`:

```
column   { children, gap? }                gap: none | small | medium | large
row      { children, gap?, wrap? }
section  { title, children }
text     { text, style? }                  style: body | title | muted | small | code | error | success
button   { id, label, icon?, variant?, disabled? }   variant: primary | secondary | ghost | danger
input    { id, placeholder?, value?, rows? }
         rows > 1: a text area that many lines tall (max 24); Enter adds a
         line and a paste keeps its line breaks
list     { id, items, empty? }
         items: [{ id, title, subtitle?, detail?, icon?, tone?, actions?: [{ id, label?, icon?, tooltip? }] }]
choice   { id, options: [{ value, label }], value? }
toggle   { id, label, value? }
badge    { text, tone? }                   tone: neutral | info | success | warning | error
spinner  { text? }
divider  {}
```

Events: `button` sends `click`; `input` sends `change` and `submit` with `value`; `list` sends `select` with `item`, and row buttons send `action` with `item` and `action`; `choice` sends `change` with the option value; `toggle` sends `change` with the new boolean.

A list item's `tone` colors its icon, using the same values as `badge`.

## Testing without Agentty

A `node`, `python` or `executable` plugin is an ordinary program reading stdin and writing stdout, so you can drive it from a test: write an `initialize` request, then the notifications you want to exercise, and assert on the JSON the plugin writes back.

A `wasm` module is driven the same way by any WebAssembly runtime that can supply the three imports above.
