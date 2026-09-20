# Hello Rust

Hello world for Agentty's WebAssembly plugins: a Rust program that draws a panel with a counter,
a name field and two buttons.

It asks for **no permissions at all**. Being a `wasm` plugin, it also has no way to reach anything
outside Agentty — no files, no network, no processes — whatever its code says.

## Build and install

```sh
./build.sh
```

Then in Agentty: **Plugins → Install from Folder…** and pick this folder. Its icon appears in the
activity bar on the left, because the manifest asks for the `sidebar` surface.

## What to look at

| File | |
|---|---|
| `agentty-plugin.json` | the manifest: `runtime: wasm`, the panel and its surface |
| `src/lib.rs` | the whole plugin, about 70 lines |
| `build.sh` | `cargo build --target wasm32-unknown-unknown`, then the module next to the manifest |

The SDK is in `sdk/rust`; the protocol is in `docs/plugins/protocol.md`.
