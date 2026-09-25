---
title: Plugins
description: What plugins add to Agentty, installing them from the marketplace or your own folder, and what to check before you trust one.
---

Plugins connect Agentty with other apps and add tools to your terminals. A plugin can put a **panel** next to your terminals, add **buttons** above agent panes, add entries to the **command palette**, and hand text to an agent as a **prompt**. Where that prompt goes is the plugin's choice unless it asks you, so the permissions on its card are worth reading.

Writing one? Start with the [plugin quick start](/docs/plugin-quickstart).

## The Plugins page

Open it from the activity bar (the puzzle icon), **View → Plugins**, or the command palette (⇧⌘P → "Plugins").

- **Installed** — every plugin you have, with what it adds and what it may do.
- **Marketplace** — the plugins Agentty offers, including its own. See below.
- **Available** — plugins that ship with Agentty. If the app a plugin integrates with is installed, it is marked **Recommended**.
- **Build your own** — create a plugin with AI, or install one from a Git repository or a folder.

Each installed plugin has **Enable / Disable**, **Open Panel**, **Restart** (after editing its code), **Update** (when Agentty ships a newer built-in version), **Logs** (what the plugin printed, crashes included), **Show in Finder**, and **Uninstall** (press twice to confirm).

Plugins start when you first use them, not when Agentty launches, so an idle plugin costs nothing.

## Installing

| From | How |
|---|---|
| The marketplace | **Install** on a card under *Marketplace* |
| Built in | **Install** on a card under *Available* |
| A Git repository | Paste an `https://` URL under *Build your own* → **Install from Git** |
| A folder | **Install from Folder…** copies it into `~/.agentty/plugins/` |
| A folder you are editing | **Link Folder for Development…** runs it where it is; uninstalling only unlinks it |

You can also copy a plugin folder into `~/.agentty/plugins/` yourself and press **Refresh**.

## What you are trusting

Before installing, the card lists what the plugin may do, as full sentences under **Permissions** — make HTTP requests, start agent sessions, type into terminals, read AI conversations, see workspaces.

It also says, under **About → Runs as**, which of two very different things the plugin is:

| | |
|---|---|
| **WebAssembly** | Agentty runs the module itself. It has no files, no processes and no network of its own, and reaches only what those permissions allow — **however it is written**. |
| **A program** (`node`, `python`, an executable) | Runs as you, with the same access as anything else you start. The permissions gate what it does through Agentty; nothing gates the program itself. |

> [!IMPORTANT]
> Install a program plugin only if you trust its author. Everything in the marketplace is the first kind, which is what makes it safe to install a binary from a list.

The whole model is in [Plugin permissions](/docs/plugin-permissions).

## The marketplace

**Plugins → Marketplace** lists what [Agentty-Marketplace](https://github.com/empty-user77/Agentty-Marketplace) offers. Plugins are added there by pull request, and what is offered is **a WebAssembly module whose source is public** — nothing else. Agentty's own plugins are there too, on the same footing: nothing is bundled into the application.

Installing one:

1. Agentty reads the list over HTTPS and checks every entry again here — its id, its text, its permissions, the host its module comes from. An entry that does not check out is left out of the list rather than shown.
2. The card shows what the plugin is, where its source is, its licence, the size of the module and its checksum, and what it may do.
3. On **Install**, Agentty downloads the module and weighs it against that checksum. **Nothing reaches the plugins folder before they match.**

An entry says which plugin protocol its module is built against. An Agentty that speaks an older one still lists the plugin, but says it needs a newer Agentty instead of offering to install it.

Plugins you keep to yourself never have to pass through that list — **Install from Folder…** and **Install from Git** take anything.

## Using a plugin

- **Panel** — plugins with a panel get a button, in the tab strip above the terminals, in the activity bar down the left edge, or in the status bar along the bottom — the plugin picks which. Click it to open the panel, click again to close. The ↻ in the panel header restarts the plugin; the gear opens the Plugins page.
- **Command palette** — ⇧⌘P lists every plugin command under *Plugin*.

### Where a panel opens

The panel's layout button changes how it opens, and your choice is kept:

| | |
|---|---|
| **Docked** | beside the terminals, which move over to make room |
| **Floating** | above the window at its right edge; nothing else moves |
| **Window** | a window of its own, which can be moved and resized |
| **Full** | the whole area the terminals and pages use |

A docked panel never takes so much room that the rest of the window is squeezed: dragged past what can be docked, it becomes a floating one.

### The "Send to…" dialog

When a plugin, or a link from another app, sends a prompt, Agentty shows what will be sent and lets you choose:

- **Claude Code / Codex / Terminal**
- **New workspace** (with a folder you can change), **New tab** in the current workspace, or one of your **open workspaces** — an idle agent there receives it, otherwise a new agent tab opens
- **Send right away**, or leave it unchecked to have the text typed in without pressing Enter

Terminals only ever get the text typed in — Enter is never pressed for a prompt sent this way. Nothing is sent until you press **Send**.

`terminal.write` is the other thing: a plugin holding it types into an open terminal **and presses Enter**, in a shell as well, where that runs the command. No dialog stands in front of that one, which is why the card names it separately.

## Links from other apps

Other apps can hand work to Agentty with `agentty://` links — for example Cosmica's "Continue in Agentty".

| Link | Effect |
|---|---|
| `agentty://plugin/<id>/<path>?key=value` | Starts plugin `<id>` and calls its URL handler |
| `agentty://prompt?text=…&title=…&agent=…&cwd=…` | Opens **Send to…** with the text (`file=` attaches an absolute `.md` / `.txt` path) |
| `agentty://plugins/<id>` | Opens the Plugins page at that plugin |

If a link needs a built-in plugin you don't have yet, Agentty offers to install it and then continues with the link. Prompts that arrive this way always go through **Send to…**.

> [!NOTE]
> Links work with the installed application. A build started from a development shell is not registered with the operating system as the handler for `agentty://`.

## Cosmica

[Cosmica](https://www.cosmica.ink/) is a notes app. Its plugin ships with Agentty and is marked *Recommended* when Cosmica is installed. It reads only the notes folder and local port from Cosmica's own settings.

1. **Plugins → Install** on the Cosmica card, then open its panel from the tab strip.
2. **Notes → prompt** — search your notes, then insert one into the focused terminal or continue it somewhere you choose.
3. **From Cosmica** — right-click a note → **Continue in Agentty**. The note is saved first, then **Send to…** opens in Agentty.
4. **Session → Cosmica** — with a Claude Code or Codex pane focused, **Save AI summary** asks the agent to write a structured summary (goal, what was done, files changed, decisions, next steps) into `Agentty/` in your notes. **Save conversation log** stores the raw conversation without asking the AI anything.

Busy agents are left alone; try again when the turn finishes. While Cosmica is running, notes are saved through its local API so they are indexed right away; otherwise the file is written and Cosmica picks it up on its next start.

## If something goes wrong

| Symptom | What to do |
|---|---|
| "Node.js was not found" | Plugins written in JavaScript need Node.js 18+ on your login shell's `PATH`. WebAssembly plugins need nothing installed |
| An install from the marketplace was refused | The module did not match the checksum in the listing. Nothing was written; report it to the plugin's author |
| A marketplace card says you need a newer Agentty | The plugin is built against a plugin protocol this version does not speak. Update Agentty |
| A card says the plugin stopped with an error | Open **Logs** on the card; after fixing the code press **Restart** |
| A plugin you copied in doesn't appear | Press **Refresh**, and check that the folder name matches the `id` in `agentty-plugin.json` |
| A link does nothing | Use the installed application, and make sure the plugin the link names is installed and enabled |
| A plugin behaves oddly after an update | **Restart** it, or **Disable** it and report the problem to its author |

## Where things are

| Path | Purpose |
|---|---|
| `~/.agentty/plugins/<id>/` | The plugin |
| `~/.agentty/plugins/state.json` | Which plugins are enabled, and where development links point |
| `~/.agentty/plugin-data/<id>/` | A plugin's own settings and caches |
| `~/.agentty/plugins/.sdk/` | The SDK and guides, unpacked by **Developer Guide** |
| `~/.agentty/prompts/` | Long prompts handed to agents as files |

Uninstalling removes the plugin; its data folder stays until you delete it.

## Next

- [Plugin quick start](/docs/plugin-quickstart) — write one in a few minutes
- [Rust and WebAssembly](/docs/plugin-rust) · [Node.js SDK](/docs/plugin-sdk)
- [AgentOS plugins](/docs/plugin-agentos) — plugins that run work through agents
- [Manifest reference](/docs/plugin-manifest) · [Protocol](/docs/plugin-protocol)
- [Permissions](/docs/plugin-permissions) · [Publishing](/docs/plugin-publishing)
