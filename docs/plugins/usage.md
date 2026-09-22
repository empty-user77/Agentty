# Using plugins

Plugins connect Agentty with other apps and add tools to your terminals. A plugin can put a panel
next to your terminals, add buttons above agent panes and entries in the command palette, and hand
text to an agent as a prompt. Where that prompt goes is the plugin's choice unless it asks you,
which is why the permissions on a plugin's card are worth reading before you install it.

Writing one? See the [plugin developer guide](README.md).

## The Plugins page

Open it from the activity bar (the puzzle icon), the **View → Plugins** menu, or the command palette
(⇧⌘P → "Plugins").

- **Installed** — every plugin you have, with what it adds and what it may do.
- **Available** — plugins that ship with Agentty. If the app a plugin integrates with is installed,
  it is marked **Recommended**.
- **Build your own** — create a plugin with AI, or install one from a Git repository or a folder.

Each installed plugin has: **Enable / Disable**, **Open Panel**, **Restart** (after editing its
code), **Update** (when Agentty ships a newer built-in version), **Logs** (what the plugin printed,
crashes included), **Show in Finder**, and **Uninstall** (press twice to confirm).

Plugins start when you first use them, not when Agentty launches, so an idle plugin costs nothing.

## Installing

| From | How |
|---|---|
| Built in | **Install** on a card under *Available* |
| A Git repository | paste an `https://` URL into the field under *Build your own* → **Install from Git** |
| A folder | **Install from Folder…** copies it into `~/.agentty/plugins/` |
| A folder you are editing | **Link Folder for Development…** runs it from where it is; uninstalling only unlinks it |

You can also copy a plugin folder into `~/.agentty/plugins/` yourself and press **Refresh**.

Before installing, the card lists what the plugin may do: open agent sessions and send them
prompts, type into terminals and press Enter, read AI conversations, see workspaces, make HTTP
requests. That list is asked again when it grows: an update wanting more than the version you have
says what it is adding and takes a second press, and **Update all** leaves those for you to answer
one at a time.

How much that list is worth depends on how the plugin runs, which the card also says under
**About → Runs as**. A **WebAssembly** plugin reaches nothing but what the list allows, whatever
its code says. A **Node.js, Python or executable** plugin is a program running as you, with
everything you can reach on this computer — for those the list is only what it can ask *Agentty*
for, and you are trusting the author with the rest.

## Using a plugin

- **Panel** — plugins with a panel get a button in the tab strip (right of the tab bar), marked with
  the plugin's own logo when it ships one. Click it to open their panel next to your terminals;
  click again to close it. The ↻ in the panel header
  restarts the plugin, the gear opens this page.
- **Command palette** — ⇧⌘P lists every plugin command under *Plugin*. That is where commands are:
  the bar above a pane no longer takes plugin buttons, since it had no room to spare.

### Where a prompt goes, and who chose

The "Send to…" dialog is not a boundary, and it is worth knowing that before you read the rest of
this. A plugin with `prompt.inject` can open an agent session of its own and send it text without
asking — and that is deliberate: an [AgentOS](agentos.md) walks a piece of work through several
sessions, and it could not if a dialog stood in front of every one. A plugin may instead ask you
where a prompt should go; a plugin that an `agentty://` link reached always has to, and cannot
press Enter for you.

When a plugin does ask — or when a link from another app sends a prompt — Agentty shows what will
be sent and lets you choose:

- **Claude Code / Codex / Terminal**
- **New workspace** (with a folder you can change), **New tab** in the current workspace, or one of
  your **open workspaces** — an idle agent there receives it, otherwise a new agent tab opens
- **Send right away**, or leave it unchecked to have the text typed in without pressing Enter

Terminals only ever get the text typed in here; Agentty never runs it for you.

That last line is about this dialog. The `terminal.write` permission is the other thing: a plugin
that has it can type into any open terminal and press Enter, a shell included, where that runs the
command. The two are a page apart in the permission list and only one of them is quiet.

Nothing is sent until you press **Send**. If the preview is cut off, a line tells you the whole text
is still sent.

## Links from other apps

Other apps can hand work to Agentty with `agentty://` links — for example Cosmica's "Continue in
Agentty". If the link needs a built-in plugin you don't have yet, Agentty offers to install it and
then continues with the link. Prompts that arrive this way always go through the "Send to…" dialog.

Links work with the installed app (the DMG). A build started with `cargo run` isn't registered with
macOS as the handler for `agentty://`.

## Cosmica

[Cosmica](https://www.cosmica.ink/) is a notes app; the plugin ships with Agentty and is
marked *Recommended* when Cosmica is installed. It reads only the notes folder and local port from
Cosmica's settings.

1. **Plugins → Install** on the Cosmica card, then open its panel from the tab strip.
2. **Notes → prompt** — search your notes, then use a row's buttons: insert the note into the
   focused terminal, or continue it somewhere you choose.
3. **From Cosmica** — right-click a note (or use the note's ⋯ menu) → **Continue in Agentty**. The
   note is saved first, then the "Send to…" dialog opens in Agentty.
4. **Session → Cosmica** — with a Claude Code or Codex pane focused:
   - **Save AI summary** (⇧⌘P → *Cosmica: Save AI summary of this session*) asks the agent to write a
     structured summary — goal, what was done, files changed, decisions, next steps — into
     `Agentty/` in your Cosmica notes. A notification appears once the note exists. Busy agents are
     left alone; try again when the turn finishes.
   - **Save conversation log** stores the raw conversation without asking the AI anything.
   - The folder the notes go into can be changed at the bottom of the panel.

While Cosmica is running, notes are saved through its local API so they are indexed and synced right
away; otherwise the file is written and Cosmica picks it up the next time it lists notes.

## If something goes wrong

| Symptom | What to do |
|---|---|
| "Node.js was not found" | Plugins written in JavaScript need Node.js 18+ on your login shell's PATH (`brew install node`, nvm, …). |
| A plugin card says it stopped with an error | Open **Logs** on the card. After fixing the code, press **Restart**. |
| A plugin you copied in doesn't appear | Press **Refresh**, and check the folder name matches the `id` in `agentty-plugin.json`. |
| A link does nothing | Make sure you are using the installed app, and that the plugin the link names is installed and enabled. |
| A plugin behaves oddly after an update | **Restart** it, or **Disable** it and report the problem to its author. |

## Where things are

| Path | Purpose |
|---|---|
| `~/.agentty/plugins/<id>/` | The plugin |
| `~/.agentty/plugins/state.json` | Which plugins are enabled, and where development links point |
| `~/.agentty/plugin-data/<id>/` | A plugin's own settings and caches |
| `~/.agentty/plugins/.sdk/` | The SDK and guides, unpacked by **Developer Guide** |
| `~/.agentty/prompts/` | Long prompts handed to agents as files |

Removing a plugin's folder (or pressing **Uninstall**) removes the plugin; its data folder stays
until you delete it.
