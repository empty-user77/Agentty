---
title: Rust and WebAssembly
description: Write a plugin in Rust, ship it as one .wasm file that runs everywhere, and reach only what the protocol gives it.
---

`"runtime": "wasm"` makes `main` a WebAssembly module that Agentty runs **inside itself**, on an interpreter. One file works on macOS, Windows and Linux, and there is nothing to install on the user's machine — no Node.js, no Python.

It is also the only kind of plugin the [marketplace](/docs/plugin-publishing) lists, and the reason it can: a module reaches only what the protocol hands it.

## What the module can touch

Agentty gives the module three functions and nothing else:

| Import from `agentty` | |
|---|---|
| `send(ptr, len)` | one UTF-8 JSON message to Agentty |
| `log(ptr, len)` | one line for the plugin's log |
| `now_ms() -> i64` | milliseconds since the Unix epoch |

There is no file, no socket, no environment variable, no process and no clock beyond that counter. A WebAssembly plugin cannot read `~/.agentty`, your projects or your credentials **however it is written** — not because it promises not to, but because the functions to do so were never handed to it. A module that imports anything else does not load.

Everything else it wants, it asks Agentty for with the same JSON-RPC messages a process plugin writes to stdout, and the [permissions](/docs/plugin-permissions) in `agentty-plugin.json` are checked the same way.

> [!NOTE]
> A plugin with `runtime` `node`, `python` or `executable` is the opposite: it runs as your user, with the same access as any program you start. The Plugins page names which of the two a plugin is, under **About → Runs as**.

## The SDK

The Rust SDK is `sdk/rust` in the [marketplace repository](https://github.com/empty-user77/Agentty-Marketplace/tree/main/sdk/rust), beside the plugins written with it.

```toml
# Cargo.toml
[lib]
crate-type = ["cdylib"]

[dependencies]
agentty-plugin = { path = "../../sdk/rust" }   # or a git dependency on that repository

[profile.release]
opt-level = "z"
lto = true
panic = "abort"
strip = true
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

## What `Host` gives you

| | |
|---|---|
| `set_panel(tree)` · `show_panel()` | the panel, built with `ui::` |
| `notify_user(kind, message)` · `set_badge(text)` | a toast, and up to 8 characters on the icon |
| `copy(text)` | puts text on the clipboard |
| `log(line)` | the plugin's log on the Plugins page |
| `open_url(url)` | a page in the user's browser |
| `fetch(request)` | an HTTP request — needs `net.request` |
| `prompt(text, target)` | a prompt for an agent — needs `prompt.inject` |
| `call(method, params)` · `notify(method, params)` | anything else in the protocol |
| `context()` · `now_ms()` | where the user is, and the clock |

`call` and `fetch` return a request id; the answer arrives in `Plugin::answer`.

A module has no files of its own, so `storage/get`, `storage/set` and `storage/keys` are how it remembers anything between runs — one JSON document in its own folder, up to 64 keys and a megabyte. They need no permission.

## Build and install

```bash
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/hello.wasm hello.wasm
```

Put the module next to `agentty-plugin.json`, then **Plugins → Install from Folder…** and pick the folder. **Restart** on the plugin's page picks up a new build.

## Logo

A module has no folder of files to ship a picture in, so it carries one itself, in a WebAssembly custom section called `agentty.logo`. The engine ignores the section, and the checksum in the marketplace entry already covers it — the logo is reviewed bytes, like the rest of the module. In Rust that is one static:

```rust
#[used]
#[link_section = "agentty.logo"]
static LOGO: [u8; 1234] = *include_bytes!("logo.png");
```

`#[used]` is not optional: a release build drops a static nothing refers to, and the section goes with it. The array's length has to be the file's length.

The bytes have to be a PNG, JPEG, GIF or WebP of 512 KB at most, and are checked against that on install. An SVG is refused however it is named — see [Logo](/docs/plugin-manifest#logo). Agentty writes the picture out when the plugin is installed, so drawing a row never parses the module.

## Limits

| | |
|---|---|
| Module size | 64 MB (8 MB for a marketplace entry) |
| Memory | 64 MB |
| One message | 16 MB |
| Work per message | a budget — a plugin that does not return is stopped with "did not finish in time" |
| Messages while handling one | 256 — `send` and `log` together — then the plugin is stopped |

One message is handled at a time. A module runs **only** while it is handling one, so there is no background loop: `host/timer` is a request answered once the time has passed, and answering it is all the plugin gets.

## Worked examples

Both are in the marketplace repository, source and all, and install from **Plugins → Marketplace**:

- [`hello-rust`](https://github.com/empty-user77/Agentty-Marketplace/tree/main/src/hello-rust) — a panel and a counter, no permissions at all.
- [`agent-rest-client`](https://github.com/empty-user77/Agentty-Marketplace/tree/main/src/agent-rest-client) — an HTTP client with environments, saved requests and a proxy, `net.request`.

## Next

- [AgentOS plugins](/docs/plugin-agentos) — a plugin that runs work through Agentty's agents
- [Manifest reference](/docs/plugin-manifest) · [Plugin protocol](/docs/plugin-protocol) — including the module's ABI
- [Permissions](/docs/plugin-permissions) · [Publishing](/docs/plugin-publishing)
