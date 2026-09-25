---
title: Plugin permissions
description: The five permissions, what a plugin sees without them, and why a WebAssembly plugin is a different proposition from a program.
---

What a plugin may do is declared in advance, in `agentty-plugin.json`, and shown to the user before they install it. A call made without its permission fails with error `-32001`.

## The five permissions

| Permission | What the user is told | Method |
|---|---|---|
| `net.request` | Make HTTP requests to the addresses you give it | `net/fetch` |
| `prompt.inject` | Start agent sessions and send them prompts | `prompt/inject` |
| `terminal.write` | Type into terminals and press Enter | `terminal/send` |
| `session.read` | Read the conversation of AI sessions open in Agentty | `session/get` |
| `workspace.read` | See open workspaces, tabs, folders and agent status | `workspace/list`, `host/revealPath`, `pane/status` |

A plugin's own storage and its panel need no permission. `storage/get`, `storage/set` and `storage/keys` are its own folder (`<data dir>/plugin-data/<id>/storage.json`, created `0600`), up to 64 keys and a megabyte. A WebAssembly plugin has no files of its own, so that is how it remembers anything.

> [!IMPORTANT]
> `net.request` together with `session.read` or `workspace.read` means the plugin can read your work and send it somewhere. A plugin that asks for both should say plainly, in its description, why it needs them — and the marketplace refuses one that does not.

## Two kinds of plugin

This is the distinction that matters most, and the Plugins page names it under **About → Runs as**.

| | |
|---|---|
| **`wasm`** | Agentty runs the module itself. It has no file, no socket, no environment variable and no process — it reaches only what the protocol gives it, **however it is written**. |
| **`node`, `python`, `executable`** | A program running as your user, with the same file and network access as anything else you start. The permissions above still gate the protocol, but nothing gates the program. |

So the permission list means something different in each case. For a WebAssembly plugin it is the whole of what the plugin can do. For a program it is the whole of what the plugin can do *through Agentty* — it could always open a file itself.

Only install a program plugin if you trust its author. The [marketplace](/docs/plugin-publishing) lists WebAssembly modules with public source and nothing else, for exactly this reason.

## What a plugin sees without permissions

Context is filtered by what the plugin declared. Without `workspace.read`, folder and title fields are removed; without `session.read`, the session id is removed. What remains are ids, `kind`, `tool`, `status`, `running` and the language — enough to know which pane is focused and whether it is busy, not where you work or what you are doing.

```json
{
  "pane": { "id": 12, "kind": "claude", "tool": "claude", "status": "working", "running": true },
  "language": "en"
}
```

## Where a prompt goes, and who chose

The dialog is not a boundary, and it is worth being plain about that. `prompt.inject` lets a plugin **open an agent session of its own and send it text, without asking**. That is deliberate: an [AgentOS](/docs/plugin-agentos) is a plugin whose whole job is running steps through sessions it started, and it could not do that through a dialog.

A plugin **may** hand the choice to the user instead. `target: "ask"` opens **Send to…**: they see the text, pick the agent and the destination, and nothing happens until they press **Send**. Every other target — `newTab`, `newWorkspace`, `active`, `pane`, `workspace` — skips it. So a plugin acting on anything that came from outside should always use `ask`, and one a link reached has no say: Agentty routes it through the dialog whatever it asked for.

The two calls differ on Enter, and the difference is the whole of `terminal.write`:

- `injectPrompt` never presses Enter in a terminal. It types the text and leaves it to the user.
- `sendToTerminal` is the other thing, and it **does** press Enter — in a shell as well, where that runs the command.

## Links are not trusted

`agentty://` links can come from anywhere, including a web page. Once a link has reached a plugin, and for as long as that plugin keeps running, Agentty

- routes that plugin's `prompt/inject` through **Send to…** whatever target it asked for, and never presses Enter, and
- refuses `terminal/send` outright.

Nothing lifts it while the plugin runs. A click in the panel the link opened is not consent to type into a terminal, and neither is waiting — a plugin can wait as easily as a user can click. **Restarting the plugin is what clears it.**

The one thing a link-reached plugin can still do outside Agentty is `host/openUrl`, one address at a time. Agentty cannot tell a good address from a bad one, so that part is yours: treat whatever a link hands you as text from a stranger, and never open an address it gave you unexamined.

> [!IMPORTANT]
> If your plugin handles links, validate every parameter. The Cosmica plugin only opens `.md` files inside the Cosmica notes folder and refuses paths outside it.

## What `net.request` is allowed

`net/fetch` is the only way a plugin reaches the network, and Agentty bounds it:

| | |
|---|---|
| Methods | `GET`, `HEAD`, `POST`, `PUT`, `PATCH`, `DELETE`, `OPTIONS` over `http` or `https` |
| Headers | at most 32, none of them `Host`, `Content-Length`, `Transfer-Encoding`, `Connection`, `Upgrade` or `Expect`, and none carrying a line break |
| Body | 1 MB |
| Response | 4 MB kept (`truncated` says when more arrived) |
| Timeout | 15 s by default, 60 s at most |
| In flight | four per plugin |

**Nothing of yours travels with the request** — no cookie, no stored credential — only what the plugin puts in it. Each call is written to the plugin's log with the URL redacted.

Up to 3 redirects are followed, and each address they name goes through the same checks as the original: `http` or `https`, and not a link-local or metadata address — otherwise a server could answer `302 Location: http://169.254.169.254/…` and walk past them. An `Authorization` or `Cookie` header is not carried to another host, and a redirected `POST`, `PUT` or `PATCH` becomes a `GET` without its body unless the answer was `307` or `308`.

## Rate limits and runaways

Agentty protects itself from a plugin behaving badly, whether or not it meant to.

| Limit | What happens |
|---|---|
| Panel redraws | at most one every 50 ms |
| Notifications | at most one every 700 ms; the rest are answered normally and dropped. A message is cut at 300 characters |
| Opening a URL | at most one every 700 ms — `host/openUrl` is throttled like a notification |
| Messages | more than 240 a second stops the plugin as a runaway |
| UI tree | 2,000 elements, 12 levels deep, 20,000 characters per string. A `choice`'s options and a list item's buttons each count as elements |
| Line length | 16 MB on stdout and stderr |
| WebAssembly | 64 MB module, 64 MB memory, 16 MB per message, 256 messages while handling one — `send` and `log` together |

## Writing a plugin that deserves it

- **Ask only for the permissions you use.** Every one is shown to the user, and one you do not use only costs you trust.
- **Never read or send credentials.** If you read another app's config, take only the fields you need — the Cosmica plugin reads `notes.path` and a port.
- **Keep data in the plugin's own storage.** Create files other apps read with care, and do not overwrite the user's files unasked.
- **Talk to `127.0.0.1`** unless the user configured something else.
- **Check `pane.status` before typing.** Do not interrupt `working`, `permission` or `question`.
- **Validate everything from outside** — links, files, server responses.

## Before you install someone else's

- Read the permissions on the card. A notes plugin asking for `terminal.write` should explain itself.
- Check **Runs as**. A WebAssembly module is a much smaller thing to trust than a program.
- Prefer plugins whose source you can read; the card's links usually point at the repository.
- **Logs** after installing shows what the plugin actually does.
- **Disable** takes effect at once; **Uninstall** removes it. The data folder stays until you delete it.
