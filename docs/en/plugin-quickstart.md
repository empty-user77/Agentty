---
title: Plugin quick start
description: Build a working Agentty plugin — with an AI agent, in Rust, or in JavaScript — and install it.
---

A plugin is a folder with a manifest and a program. It can put a **panel** next to your terminals, add **buttons** above agent panes, add entries to the **command palette**, and hand text to an agent as a **prompt**.

There are two kinds, and the choice is worth making first:

| | |
|---|---|
| **WebAssembly** (`runtime: "wasm"`) | One `.wasm` file, usually built from Rust. Agentty runs it itself, so it reaches only what the protocol gives it and needs nothing installed on the user's machine. The only kind the [marketplace](/docs/plugin-publishing) lists. |
| **A program** (`node`, `python`, `executable`) | Runs as the user, with the same access as anything else they start. Quick to write, installed from a folder or a Git repository the user chose. |

## With AI

Most plugins are written this way. Open **Plugins** (the puzzle icon in the activity bar), enter a name and what the plugin should do under *Build your own*, and press **Create and Build with Claude Code**.

Agentty creates the plugin from a template — the SDK, its type definitions, the developer guide and a `CLAUDE.md` / `AGENTS.md` with instructions — and opens Claude Code in its folder. **Copy AI Prompt** gives you the same prompt to use with any other agent.

### What to tell the agent

The template already carries the guide, so the useful prompt is the plugin, not the platform:

```text
Read PLUGIN_GUIDE.md and agentty-plugin.d.ts first.

Build a panel that lists the Git branches in the focused pane's folder.
Each row: branch name, and how far ahead or behind main it is.
Clicking a row asks the agent to summarize what changed on that branch.

Ask for prompt.inject and workspace.read, and nothing else.
```

Then tell it to check the code (`node --check main.mjs`, or `cargo build --release --target wasm32-unknown-unknown`), and press **Restart** on the plugin card in Agentty to pick it up. Errors are under **Logs** on the same card.

> [!TIP]
> If you are writing the prompt for an agent that is not working inside the template folder, point it at [the manifest reference](/docs/plugin-manifest), [the UI tree](/docs/plugin-protocol#ui-tree) and [the protocol](/docs/plugin-protocol). Those three pages are the whole surface.

## By hand, in Rust

```
hello/
├── agentty-plugin.json   manifest
├── hello.wasm            the compiled module
└── src/lib.rs            its source
```

```rust
use agentty_plugin::{export_plugin, ui, Host, Plugin, UiEvent};

#[derive(Default)]
struct Hello {
    clicks: u32,
}

impl Plugin for Hello {
    fn panel_open(&mut self, host: &Host) {
        host.set_panel(ui::column(vec![
            ui::text(format!("Clicked {} times", self.clicks)),
            ui::button("go", "Click me"),
        ]));
    }

    fn ui_event(&mut self, host: &Host, event: UiEvent) {
        if event.element == "go" {
            self.clicks += 1;
            self.panel_open(host);
        }
    }
}

export_plugin!(Hello);
```

```json
{
  "id": "hello",
  "name": "Hello",
  "version": "0.1.0",
  "runtime": "wasm",
  "main": "hello.wasm",
  "contributes": { "panel": { "title": "Hello", "surface": "sidebar" } }
}
```

```bash
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/hello.wasm hello.wasm
```

[Rust and WebAssembly](/docs/plugin-rust) has the SDK in full.

## By hand, in JavaScript

```
~/.agentty/plugins/hello/
├── agentty-plugin.json   manifest
├── main.mjs              the plugin program
└── agentty-plugin.mjs    the SDK
```

The SDK is a single dependency-free file. Press **Developer Guide** on the Plugins page and Agentty unpacks it, with its types and guides, into `~/.agentty/plugins/.sdk/` — copy `agentty-plugin.mjs` from there next to your `main.mjs`.

```json
{
  "id": "hello",
  "name": "Hello",
  "version": "0.1.0",
  "main": "main.mjs",
  "permissions": ["prompt.inject"],
  "contributes": {
    "panel": { "title": "Hello", "icon": "sparkles" },
    "commands": [{ "id": "hello.explain", "title": "Hello: Explain this folder", "icon": "bot" }]
  }
}
```

```js
import { createPlugin, ui } from './agentty-plugin.mjs';

const plugin = createPlugin();

plugin
  .onPanelOpen((context) =>
    plugin.setPanel(
      ui.column([
        ui.text('Hello', 'title'),
        ui.text(context.pane ? `You are in ${context.pane.cwd}` : 'No terminal focused', 'muted'),
        ui.button('explain', 'Explain this folder', { icon: 'bot', variant: 'primary' }),
      ]),
    ),
  )
  .onEvent('explain', (_event, context) => explain(context))
  .command('hello.explain', ({ context }) => explain(context))
  .start();

function explain(context) {
  return plugin.injectPrompt({ text: 'Give me a short tour of this project.', cwd: context.pane?.cwd, target: 'ask' });
}
```

> [!WARNING]
> Never write to stdout (`console.log`) — it carries the protocol. Log with `plugin.log()` or `console.error()`.

[Node.js SDK](/docs/plugin-sdk) has the rest.

## Install it

**Plugins → Install from Folder…** and pick the folder, or **Link Folder for Development…** to run it where it is. Then **Refresh** (or reopen the page): the plugin shows up as installed, its panel button appears where its `surface` says, and its commands appear in the palette (⇧⌘P).

## Test it

A `node`, `python` or `executable` plugin is a program that reads stdin and writes stdout, so you can drive it without Agentty:

```bash
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"plugin":{"id":"hello","dataDir":"/tmp"},"context":{}}}' \
  '{"jsonrpc":"2.0","method":"panel/open","params":{"context":{}}}' \
  | node main.mjs
```

It should answer `initialize` and then send a `ui/setPanel` request.

## Next

- [Manifest reference](/docs/plugin-manifest) — every field
- [Rust and WebAssembly](/docs/plugin-rust) · [Node.js SDK](/docs/plugin-sdk)
- [AgentOS plugins](/docs/plugin-agentos) — running work through agents
- [Permissions](/docs/plugin-permissions) · [Publishing](/docs/plugin-publishing)
