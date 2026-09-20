# agentty-plugin (Rust SDK)

Write an Agentty plugin in Rust and ship it as one `.wasm` file that runs on macOS, Windows and
Linux.

Agentty runs the module itself, on an interpreter, and hands it three functions: send a message,
write a log line, read the clock. There is nothing else — no file, no socket, no environment
variable, no process — so a plugin reaches only what the protocol gives it, and only with the
permissions its `agentty-plugin.json` declares.

## A plugin

```toml
# Cargo.toml
[lib]
crate-type = ["cdylib"]

[dependencies]
agentty-plugin = { path = "../../sdk/rust" }

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

## Build and install

```sh
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/hello.wasm hello.wasm
```

Put the module next to `agentty-plugin.json`, then **Plugins → Install from Folder…** in Agentty
and pick the folder. **Restart** on the plugin's page picks up a new build.

## What the host gives you

| `Host` | |
|---|---|
| `set_panel(tree)` · `show_panel()` | the panel, built with `ui::` |
| `notify_user(kind, message)` · `set_badge(text)` | a toast, and up to 8 characters on the icon |
| `log(line)` | the plugin's log on the Plugins page |
| `open_url(url)` | a page in the user's browser |
| `fetch(request)` | an HTTP request — needs `net.request` |
| `prompt(text, target)` | a prompt for an agent — needs `prompt.inject` |
| `call(method, params)` · `notify(method, params)` | anything else in the protocol |
| `context()` · `now_ms()` | where the user is, and the clock |

`call` and `fetch` return a request id; the answer arrives in `Plugin::answer`.

## Limits

One message is handled at a time, with a budget of work behind it: a plugin that never returns is
stopped, and so is one that sends more than 256 messages while handling a single event. Memory is
capped at 64 MB and a message at 16 MB.

The protocol, including the module's ABI, is in `docs/plugins/protocol.md`.
