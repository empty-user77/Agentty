# Agentty plugin protocol (API version 3)

For writing plugins without the Node.js SDK. Read the [plugin guide](README.md) first; this page
only describes the wire format.

| Version | What it added |
|---|---|
| 1 | the panel, commands, links, `storage/*`, `net/fetch`, `prompt/inject`, `session/get` |
| 2 | `host/timer` and `pane/status` — what a plugin needs to walk work through agents |
| 3 | `browser/*` — the in-app browser on the sites a plugin names (`browser.control`) |

A plugin that uses something a version added says so, with `apiVersion` in its manifest and in its
marketplace entry. An Agentty that speaks less than that says to update rather than installing a
module it cannot run; a manifest that leaves the field out is read as version 1, which is what it
was before the field existed.

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
| `pane/status` | notification | `{ paneId, status, running, agent, title, cwd }` — a pane this plugin started changed what it is doing (`workspace.read`) |
| `browser/hidden` | notification | `{ tabId }` — the user took one of the plugin's pages out of the browser panel; it keeps running out of sight (`browser.control`) |
| `instance/open` | notification | `{ instance, title }` — an automation (a tab of the plugin's workspace) exists: each one when the plugin starts, then every new one |
| `instance/close` | notification | `{ instance }` — the user closed an automation's tab; its pages are closed already |
| `shutdown` | notification | `{}` |

`initialize` is sent first, followed immediately by whatever started the plugin (a command, the panel
opening, a link). Answer `initialize` with any result, e.g. `{}`.

## Plugin → Agentty

Send these as requests (with an `id`) to get a result or an error, or as notifications (no `id`)
when you don't care.

| Method | Permission | `params` | Result |
|---|---|---|---|
| `ui/setPanel` | | `{ tree, instance? }` — with `instance`, the panel of that automation | `null` |
| `workspace/instances` | | `{}` | `[{ instance, title, active }]` — the automations of the plugin's workspace |
| `workspace/setInstanceTitle` | | `{ instance, title }` | `null` — what the automation's tab is called |
| `workspace/setInstanceStatus` | | `{ instance?, state: "working" \| "idle" \| "error", text? }` — `instance` omitted or empty is the plugin's panel outside a workspace; `text` is a short one-line status (max 120 characters), e.g. `"Collecting @sama (3/6)"` | `null` — what the automation (or the plugin, with no workspace) is doing, shown next to its workspace card and in the menu bar popover without opening it |
| `workspace/closeInstance` | | `{ instance }` | `null` — closes that automation's tab (one of the plugin's own, open); it is not offered among the recently closed tabs |
| `ui/showPanel` | | `{}` | `null` |
| `ui/notify` | | `{ message, kind: "info" \| "success" \| "warning" \| "error" }` | `null` |
| `ui/setBadge` | | `{ text }` (max 8 characters) | `null` |
| `context/get` | | `{}` | context |
| `host/info` | | `{}` | `{ version, apiVersion, language, utcOffsetMinutes, uiFeatures }` — `utcOffsetMinutes`: the user's time zone, minutes east of UTC, for showing times and cutting days the way the user reads them; `uiFeatures`: panel elements added since the first API version (`flow`, `popover`), missing on older Agentty |
| `host/openUrl` | | `{ url }` (http/https) | `null` |
| `host/timer` | | `{ ms }` | `{ elapsedMs }`, once the time has passed |
| `host/copy` | | `{ text }` (up to 100,000 characters) | `null` |
| `host/revealPath` | `workspace.read` | `{ path }` (absolute, existing) | `null` |
| `prompt/inject` | `prompt.inject` | `{ text, title?, target?, paneId?, workspaceId?, agent?, model?, cwd?, submit?, tools? }` — `model`: one `agent/list` named for that agent — `target`: `ask` · `active` · `newWorkspace` · `newTab` · `split` · `pane` · `workspace` · `own` (a new tab in the plugin's own workspace) | `{ status: "asked" }` or `{ status: "sent", paneId }` |
| `terminal/send` | `terminal.write` | `{ paneId?, text, submit? }` (focused pane without `paneId`) | `{ paneId }` |
| `agent/list` | `prompt.inject` | `{}` | `[{ id, name, version?, models: [{ id, label }] }]` — the agents `prompt/inject` can start here (installed): `claude`, `codex`, with the models each was seen using (its default first) |
| `terminal/close` | `prompt.inject` | `{ paneId }` — a terminal `prompt/inject` opened for this plugin (`newTab`, `newWorkspace`, `split`, `own`); any other is refused with `-32001` | `null` |
| `session/get` | `session.read` | `{ paneId?, maxTurns? }` (default 200, max 2000) | `{ paneId, agent, sessionId, title, cwd, status, turnCount, turns: [{ role, text }] }` |
| `workspace/list` | `workspace.read` | `{}` | `[{ id, name, cwd, active, panes: [pane] }]` |
| `net/fetch` | `net.request` | `{ url, method?, headers?, body?, timeoutMs?, proxy? }` | `{ status, statusText, url, headers, body, truncated, binary, bytes, durationMs }` |
| `storage/get` | | `{ key }` | `{ key, value }` (`value` is null when unset) |
| `storage/set` | | `{ key, value }` (null removes it) | `null` |
| `storage/keys` | | `{}` | `[key]` |
| `files/write` | `files` | `{ path, text? \| base64?, append? }` (8 MB a call) | `{ size }` |
| `files/read` | `files` | `{ path, offset?, length?, as?: "base64" }` (4 MB a call) | `{ text \| base64, size, eof }` |
| `files/list` | `files` | `{ path }` (`""` is the folder itself) | `[{ name, dir, size, modifiedMs }]` |
| `files/stat` | `files` | `{ path }` | `{ name, dir, size, modifiedMs }` or `null` |
| `files/remove` | `files` | `{ path }` (a folder with everything in it) | `{ removed }` |
| `files/rename` · `files/copy` | `files` | `{ from, to }` | `null` · `{ size }` |
| `files/path` | `files` | `{ path }` | `{ path }` — where it is on disk (for `prompt/inject`'s `cwd`) |
| `files/reveal` | `files` | `{ path }` | `null` — shows it in Finder / Explorer |
| `media/svgToPng` | `files` | `{ from, to, width? }` | `{ width, height, size }` — draws the SVG `from` into the PNG `to` (both in the plugin's folder), `width` pixels wide (at most 4096 a side), text in the system's fonts |
| `files/pick` | `files` | `{ into?, multiple? }` | `[{ path, name, size }]` — the system's open panel; what the user picks is copied into the folder `into` (default `picked`) of the plugin's own; `[]` when cancelled |
| `files/download` | `files` + `net.request` | `{ url, path, maxBytes?, headers?, timeoutMs?, proxy? }` | `{ status, url, contentType, bytes, durationMs }` |

**Files-only agents.** `tools: "files"` starts the agent with nothing but reading and writing
files in `cwd`, which must be a folder of the plugin's own data: no shell, no web, no MCP servers
(so not the in-app browser, which is signed in as the user); Claude Code runs in `acceptEdits`, Codex
in its `workspace-write` sandbox without network. Give an agent text that is not the user's — web
pages, posts, mail — only this way, and check what it writes before acting on it.

**Whose terminals.** A plugin types freely only into terminals of its own: ones `prompt/inject`
started for it (`newTab`, `newWorkspace`, `split`, `own`) and any in its own workspace. Another
terminal — the user's own agents and shells, `active`, `pane`, `workspace` — takes a prompt or
`terminal/send` only within 10 seconds of the user using the plugin (a click in its panel, one of
its commands), and never while that terminal waits for the user to approve or answer something. A
`prompt/inject` outside that goes to the `ask` dialog instead (`{ status: "asked" }`, not sent until
the user sends it); `terminal/send` fails with `-32001`.

Agentty drops `ui/notify` calls that arrive faster than one per 700 ms (answering them normally), and
stops a plugin that sends more than 240 messages a second. `host/openUrl` is metered the same way —
one address every 700 ms, the rest refused with `-32002` — because it needs no permission and 240
messages a second would otherwise be 240 browser tabs. It is also the one thing a plugin an
`agentty://` link reached can still do outside Agentty, so treat what a link hands you as text from
a stranger and do not open an address from it unexamined. Context fields are limited by the
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

`host/timer` is how a plugin waits: a request answered once the time has passed. 100 ms at the
shortest, an hour at the longest, eight at a time. A module runs only while it is handling a
message, so this is the whole of how it comes back to something later — answering it is all the
plugin gets, which is why it is not a way to run in the background.

`pane/status` is how a plugin hears that an agent it started has finished. A plugin learns a pane
id from `prompt/inject` (`{ status: "sent", paneId }`); Agentty remembers which plugin started
which pane and tells only that plugin, when that pane's status changes — `working`, `idle`,
`finished`, `permission`, `question`, `interrupted`, `exited`, or `closed` once. It needs
`workspace.read`, the permission that already means "see agent status". At most 32 panes are
followed at a time. [AgentOS plugins](agentos.md) are built on this and `host/timer`.

A prompt the user placed themselves is followed too: `target: "ask"` answers `{ status: "asked" }`
with no pane id, because there is none yet, and the session the user picks is watched all the same
— its first `pane/status` is where the plugin learns which pane it became. A plugin that has more
than one question outstanding tells them apart by `title`, which is the one it gave the prompt.

Status arrives whether or not anything is being drawn: a window behind another, or one on a locked
screen, is not drawn, and a plugin waiting for an agent must not be waiting for the user to come
back. While any pane is watched, Agentty looks every 400 ms of its own accord, and stops looking
when the last one is done.

A pane is `idle` from the moment it opens, before the agent has picked the prompt up, so `idle`
alone does not mean finished — wait until that pane has been `working` at least once. And an agent
between two tool calls is idle for a moment, so a stop is worth giving a second or two before its
session is read as the answer.

`storage/*` is what a plugin remembers between runs: one JSON document in its own folder
(`<data dir>/plugin-data/<plugin>/storage.json`, created `0600`), read and written by key. It goes
when the plugin is uninstalled — what a plugin kept is what it was given — and an update leaves it
alone. A file that is no longer readable as JSON is moved aside as `storage.corrupt.json` rather
than written over, so nothing the plugin had is lost without trace. Keys are
lower-case letters, digits, `.`, `-` and `_`; at most 64 of them, and a megabyte in total. A plugin
that runs as a process can write its own files instead; a WebAssembly plugin has no files, so this
is how it keeps anything.

`files/*` is a folder of the plugin's own (`<data dir>/plugin-data/<plugin>/files`, `0700`) for
what it keeps that is more than settings: pictures, videos, drafts, logs. Paths are relative to it
and never leave it: no absolute paths, no `..`, and a link inside is never followed. Files are
created `0600`; a write goes to a temporary file first, so a file is either the old one or the new
one, never half of each. A name is at most 128 characters, a path 16 folders deep, a listing 5000
entries, a file 1 GB. `text` is UTF-8; anything else is read and written as `base64`. Like
`storage/*`, the folder goes when the plugin is uninstalled and stays through an update.

`files/download` is `net/fetch` into a file, for what is too large for a message: the same checks
on the address and its redirects, streamed to disk, at most `maxBytes` (1 GB), and up to 15
minutes. The file appears only once it is whole.

### The browser (`browser.control`, API version 3)

A plugin with `browser.control` drives pages in Agentty's in-app browser — the one the user is
signed in to — on the sites its manifest names, and nowhere else:

```json
"apiVersion": 3,
"permissions": ["browser.control"],
"browser": {
  "sites": [
    { "host": "x.com", "aliases": ["twitter.com"], "home": "https://x.com/home",
      "signIn": "https://x.com/i/flow/login", "signedInCookie": "auth_token" }
  ]
}
```

`host` and `aliases` cover their subdomains; pages are `https` (plain `http` only for `localhost`
and `127.0.0.1`). `home` and `signIn` must be pages of the site. `signedInCookie` names a cookie the
site sets only while the user is signed in: Agentty checks that it is there — never what it holds —
to say whether the site is signed in and until when. At most 8 sites; the permission and the list
come together.

| Method | `params` | Result |
|---|---|---|
| `browser/sites` | `{ profile? }` | `[{ host, signedIn: true \| false \| null, expiresAt: ms \| null }]` |
| `browser/open` | `{ url, mode?: "auto" \| "background" \| "visible", profile?, instance? }` | `{ tabId }` |
| `browser/navigate` | `{ tabId, url }` | `null` |
| `browser/eval` | `{ tabId, script, args?, timeoutMs? }` | `{ value }` |
| `browser/wait` | `{ tabId, timeoutMs? }` | `{ url, title }` once the page has loaded |
| `browser/info` | `{ tabId }` | `{ tabId, url, title, loading, visible, site }` |
| `browser/show` · `browser/hide` · `browser/close` | `{ tabId, message? }` | `null` |
| `browser/signIn` | `{ host, message?, profile?, instance? }` | `{ tabId, signedIn, expiresAt?, reason?: "closed" \| "timeout" }` |
| `browser/profiles` | `{}` | `{ supported, profiles: [name] }` |
| `browser/removeProfile` | `{ profile }` | `{ removed }` — the profile's store, sign-ins included, is deleted |

- **The user decides first.** The first browser call a plugin makes shows a dialog naming the
  plugin and its sites, and saying that what it does there is done in the user's name. Until the
  user allows it, calls wait for the answer; after "Don't allow" they fail with `-32001` until the
  plugin restarts. An update that names a new site asks again. The plugin's page in the Plugins
  view shows the sites and takes the permission back.
- **Profiles** are sign-ins of their own (macOS 14 and later): a page opened with `profile: "brand-b"`
  keeps its cookies and site data apart from the in-app browser and from the plugin's other
  profiles, so each can be signed in to another account of the same site. The name is the plugin's
  (lower-case letters, digits, `-`, `_`); the store it means is Agentty's, made the first time the
  name is used, deleted by `browser/removeProfile` or with the plugin. No `profile`, or `"default"`,
  is the in-app browser's own sign-in.
- **Pages** run out of sight (parked outside the window: never drawn, never throttled, so a page
  that loads more as it scrolls keeps working with Agentty minimized) or as a tab of the browser
  panel. `auto` runs them out of sight and shows them when the user is needed; the user can pin a
  plugin to one or the other. Three pages per plugin; they close when the plugin stops or restarts.
- **Scripts** (`browser/eval`) are the body of an async function with `args`, run in a content
  world of their own — the page's scripts share the DOM but cannot see or replace the plugin's.
  They run only while the page is on one of the plugin's sites, checked when the call arrives and
  again inside the page. The value comes back as JSON, 4 MB at most; `timeoutMs` is 15 s by default
  and 60 s at most.
- **Signing in is the user's.** `browser/signIn` opens the site's sign-in page as a tab with a line
  above it saying which plugin asks, and a Cancel; if Agentty is not in front, a notification says
  so too. It answers once `signedInCookie` appears, the user closes the tab, or ten minutes pass. A
  plugin never gets a password, a cookie or a way to type one in on the user's behalf.
- **Staying signed in is Agentty's.** For every site of an enabled `browser.control` plugin,
  Agentty keeps session cookies (the ones without an expiry, which end with the browser) in the
  Keychain and puts them back at start, and every six hours visits a signed-in site's `home` out of
  sight, which keeps sessions that end when unused going. "Clear cookies, cache and history" in the
  browser settings forgets them; private mode turns all of it off (and `browser/open` refuses).
- **It reaches the network too.** A script running in a page can make requests from that page, and
  a page can be sent anywhere; so `browser.control` lets a plugin carry what it reads out of Agentty
  even without `net.request`. The dialog and the Plugins view say the plugin acts in the user's
  name; allow it only for plugins you trust.
- A plugin that a link reached has no browser until it restarts (`-32001`). Windows and Linux have
  no in-app browser yet: every `browser/*` call answers `-32002`.

Errors use these codes:

| Code | Meaning |
|---|---|
| `-32601` | unknown method |
| `-32602` | invalid parameters (bad UI tree, no such pane, …) |
| `-32001` | permission missing, or refused because a link reached the plugin (until it restarts) |
| `-32002` | unavailable (no window open, no session yet) |

## Example session

```
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"apiVersion":2,"plugin":{"id":"hello",…},"language":"en","context":{…}}}
→ {"jsonrpc":"2.0","method":"panel/open","params":{"context":{…}}}
← {"jsonrpc":"2.0","id":1,"result":{}}
← {"jsonrpc":"2.0","id":1,"method":"ui/setPanel","params":{"tree":{"type":"column","children":[{"type":"button","id":"go","label":"Go"}]}}}
→ {"jsonrpc":"2.0","id":1,"result":null}
→ {"jsonrpc":"2.0","method":"ui/event","params":{"element":"go","event":"click","context":{…}}}
← {"jsonrpc":"2.0","id":2,"method":"prompt/inject","params":{"text":"Hello","target":"ask"}}
→ {"jsonrpc":"2.0","id":2,"result":{"status":"asked"}}
```

(`→` Agentty to plugin, `←` plugin to Agentty. Request ids are per direction.)

**Permissions are asked once.** The first time a plugin that is not part of Agentty would start,
Agentty shows the user what its manifest asks for (the permissions and, for `browser.control`, its
sites) and starts it only when the user allows. Until then it does not run at all: no
`initialize`, no activation. An update that asks for nothing new starts without asking again; one
that asks for more asks for the new set. Refused, the plugin stays stopped until the user asks
again from its page or panel. Uninstalling forgets the answer.

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
one is stopped as well. A line written to the plugin's log counts as one of those 256 — it crosses
the same channel to the same thread — and is cut at 2 000 characters where it is read, not where
it is shown.

The Rust SDK hides all of this: it is in the [marketplace repository](https://github.com/empty-user77/Agentty-Marketplace/tree/main/sdk/rust),
with [`hello-rust`](https://github.com/empty-user77/Agentty-Marketplace/tree/main/src/hello-rust) as a working example.

## Surfaces and how a panel opens

`contributes.panel.surface` says where the plugin's mark goes — its logo when it ships one, else
its `icon` name. A plugin picks one place:

| `surface` | Where |
|---|---|
| `pane` (default) | the tab strip above the terminals |
| `sidebar` | the activity bar down the left edge, with Agentty's own pages |
| `status` | the status bar along the bottom |

`contributes.panel.mode` says how the panel opens. The user can change it from the panel's layout
button, and their choice is kept; this is what it does first:

| `mode` | |
|---|---|
| `push` | docked beside the terminals, which move over to make room |
| `overlay` | floating above the window at its right edge; nothing else moves |
| `window` | a window of its own, which can be moved and resized |
| `full` | the whole area the terminals and pages use |
| `workspace` | a workspace of the plugin's own: its terminals as tabs, the browser with its pages beside them, the panel docked right of those |

A manifest that names no `mode` gets `push` when the plugin works in terminals (it has
`prompt.inject` or `terminal.write`) and `full` otherwise: a plugin that does its work out of sight
gets the whole area.

A docked panel never takes so much room that the rest of the window is squeezed: dragged past what
can be docked, it becomes an overlay.

**`workspace`** is for plugins whose work runs in agents and pages the user watches. Its icon
switches to the plugin's workspace (made the first time, in the plugin's data folder, kept across
restarts, and placed in the sidebar's Plugins group): the panel docked at the right, the browser with the plugin's pages (they open
visible there unless the plugin asks otherwise), and the terminals as tabs. `prompt/inject` with
`target: "own"` opens a new tab there for each job, so several run side by side. The icon is a
place like the other items of the activity bar, and only one is lit at a time: pressed again it
stays, and anything else (the workspace list, a page, the home tab, the panel's ✕) takes its place
and puts the window back as it was — the start page when the user has no workspace of their own.
Out of sight the plugin and its automations keep running. With every tab closed, the panel offers a
new automation.

In that workspace **a tab is an automation**. Each has its own panel (`ui/setPanel` with its
`instance`; UI events from it carry the same `instance`), its own browser pages (`browser/open`
with `instance`: the browser shows the pages of the tab in front, the others keep running out of
sight) and its own terminals (`prompt/inject` with `target: "own"` and `instance` opens the job
beside that tab's terminals without bringing it to the front). The tab strip's "+", ⌘T and the
browser's "+" make a new automation: the plugin hears `instance/open` and sets it up; closing the
tab sends `instance/close`. Five tabs are five automations running side by side. Nothing of this
is specific to a site: a plugin for any site gets it by declaring `mode: "workspace"`.

Since automations keep running out of sight, `workspace/setInstanceStatus` is how a plugin says
what one of them is doing without the user opening its workspace: `working` while it does
something, `idle` between runs, `error` when it stopped on one. Agentty shows this on the
workspace's sidebar card (spinning while any automation is `working`, with the working one's
`text` as the card's second line) and as a row in the menu bar popover, with an elapsed time next
to `working`. A status is kept until it changes, the plugin stops or restarts, or that
automation's tab closes.

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
flow     { id, steps: [{ id, title, subtitle?, icon?, state?, selected?, side? }] }
                                            cards joined top to bottom, for what an automation does
                                            in order; state: off (dimmed) | on | active (spinner) |
                                            done | error; selected draws it highlighted; side draws
                                            an optional step indented, off the main line
popover  { id, title, children }            a card beside the panel, over the page next to it
                                            (hidden while it is open); the first one in the tree
                                            is shown, none: closed
```

Events: `button` → `click`; `input` → `change` / `submit` with `value`; `list` → `select` with
`item`, row buttons → `action` with `item` and `action`; `choice` → `change` with the option value;
`toggle` → `change` with the new boolean; `flow` → `select` with the step's id as `item`; `popover` → `close` from its close button.
