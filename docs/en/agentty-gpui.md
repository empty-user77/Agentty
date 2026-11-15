---
title: agentty-gpui
description: Agentty's own UI framework — a GPU-drawn Rust UI for macOS, Windows and Linux, published on crates.io as 20 crates.
---

agentty-gpui is the UI framework made for Agentty, based on Zed's gpui.

It holds everything Agentty needs to draw its interface on the GPU: elements and layout, windows and menus, keyboard and mouse input, text shaping and fonts, images and SVG, and the backends and renderers for macOS, Windows and Linux. Fixes and features Agentty needs go straight into it, and it is published on [crates.io](https://crates.io/crates/agentty-gpui) as 20 crates so any Rust app can use it the same way.

- Repository: [github.com/empty-user77/agentty-gpui](https://github.com/empty-user77/agentty-gpui)
- Crates: [crates.io/crates/agentty-gpui](https://crates.io/crates/agentty-gpui)
- API docs: [docs.rs/agentty-gpui](https://docs.rs/agentty-gpui)
- License: Apache License 2.0

## Getting started

Add two crates: `agentty-gpui`, the framework, and `agentty-gpui-platform`, which starts the backend for the OS it runs on. Their library names are `gpui` and `gpui_platform`, so code says `use gpui::...`.

```sh
cargo add agentty-gpui --rename gpui
cargo add agentty-gpui-platform --rename gpui_platform
```

```rust
fn main() {
    gpui_platform::application().run(|cx: &mut gpui::App| {
        // open windows, bind keys, ...
    });
}
```

Each platform has something to switch on:

| Platform | Setup |
|---|---|
| macOS | `runtime_shaders` on `agentty-gpui-platform` compiles the Metal shaders at startup, so the build doesn't need Xcode's Metal toolchain. Without it they compile at build time and the toolchain is required (`xcodebuild -downloadComponent MetalToolchain`). |
| Windows | Nothing extra. The default `windows-manifest` feature embeds the app manifest in the executable. |
| Linux | Turn on `wayland` and/or `x11` on both crates. |

## Features

The features of `agentty-gpui`. The defaults are `font-kit`, `wayland`, `x11` and `windows-manifest`.

| Feature | What it does |
|---|---|
| `font-kit` | Finds and loads system fonts (default) |
| `wayland`, `x11` | Linux display server support (default) |
| `windows-manifest` | Embeds the Windows app manifest in the executable (default) |
| `screen-capture` | Screen capture |
| `inspector` | An inspector for the elements of a running UI |
| `test-support` | The test platform and helpers |
| `leak-detection` | Tracks entity handles so tests can catch leaks; setting `GPUI_LEAK_DETECTION` at build time turns it on too |
| `bench-support` | Benchmarks and the profiler (`bench` is its older name) |
| `profiler` | Frame time histograms |
| `stacker` | Grows the stack for deep recursion |
| `hot-reload` | Hot reloading of code during development |

`agentty-gpui-platform` passes the features of the same name on to each backend (`font-kit`, `inspector`, `screen-capture`, `test-support`, `bench-support`, `wayland`, `x11`, `runtime_shaders`). Its default is `multithreaded`.

## The crates

All are published at the same version, and a build compiles only the crates for its target. Each link goes to the crate's crates.io page.

### Framework

| Crate | Library name | What it is | Depends on |
|---|---|---|---|
| [agentty-gpui](https://crates.io/crates/agentty-gpui) | `gpui` | The framework: elements, flexbox layout (Taffy), windows, input, actions and key bindings, text, images and SVG, the async executor | collections, http-client, macros, refineable, scheduler, shared-string, sum-tree, util, bench-metrics (optional) |
| [agentty-gpui-platform](https://crates.io/crates/agentty-gpui-platform) | `gpui_platform` | Picks the backend for the current OS and starts the app (`gpui_platform::application()`) | agentty-gpui, macos, windows, linux, wgpu (optional) |
| [agentty-gpui-macros](https://crates.io/crates/agentty-gpui-macros) | `gpui_macros` | The framework's derive and attribute macros | — |

### Platform backends and renderers

| Crate | Library name | What it is | Depends on |
|---|---|---|---|
| [agentty-gpui-macos](https://crates.io/crates/agentty-gpui-macos) | `gpui_macos` | macOS backend: AppKit windows and menus, input, notifications, screen capture (optional) | agentty-gpui, apple, collections, scheduler, util |
| [agentty-gpui-apple](https://crates.io/crates/agentty-gpui-apple) | `gpui_apple` | Metal renderer and Core Text text, shared by Apple platforms | agentty-gpui, collections, util |
| [agentty-gpui-windows](https://crates.io/crates/agentty-gpui-windows) | `gpui_windows` | Windows backend: Direct3D 11 renderer, DirectWrite text, DirectComposition | agentty-gpui, collections, scheduler, util |
| [agentty-gpui-linux](https://crates.io/crates/agentty-gpui-linux) | `gpui_linux` | Linux backend: Wayland and X11, keyboard layouts (xkbcommon), desktop portals | agentty-gpui, collections, http-client, util, wgpu (optional) |
| [agentty-gpui-wgpu](https://crates.io/crates/agentty-gpui-wgpu) | `gpui_wgpu` | wgpu renderer, used on Linux | agentty-gpui, collections, util |

### Foundations

| Crate | Library name | What it is | Depends on |
|---|---|---|---|
| [agentty-gpui-util](https://crates.io/crates/agentty-gpui-util) | `gpui_util` | Small shared helpers | — |
| [agentty-gpui-shared-string](https://crates.io/crates/agentty-gpui-shared-string) | `gpui_shared_string` | `SharedString`, a string shared without copying | — |
| [agentty-gpui-collections](https://crates.io/crates/agentty-gpui-collections) | `collections` | Hash map and hash set types | util |
| [agentty-gpui-sum-tree](https://crates.io/crates/agentty-gpui-sum-tree) | `sum_tree` | The concurrency-friendly B-tree under lists and text | ztracing |
| [agentty-gpui-scheduler](https://crates.io/crates/agentty-gpui-scheduler) | `scheduler` | Async task scheduler | — |
| [agentty-gpui-refineable](https://crates.io/crates/agentty-gpui-refineable) | `refineable` | The `Refineable` trait that lets a style be partly overridden | derive-refineable |
| [agentty-gpui-derive-refineable](https://crates.io/crates/agentty-gpui-derive-refineable) | `derive_refineable` | Its derive macro | — |
| [agentty-gpui-http-client](https://crates.io/crates/agentty-gpui-http-client) | `http_client` | HTTP client interface, for images loaded from URLs | — |
| [agentty-gpui-zlog](https://crates.io/crates/agentty-gpui-zlog) | `zlog` | Logging | collections |
| [agentty-gpui-ztracing](https://crates.io/crates/agentty-gpui-ztracing) | `ztracing` | Tracing, with optional Tracy profiler support | ztracing-macro, zlog (optional) |
| [agentty-gpui-ztracing-macro](https://crates.io/crates/agentty-gpui-ztracing-macro) | `ztracing_macro` | Its attribute macro | — |
| [agentty-gpui-bench-metrics](https://crates.io/crates/agentty-gpui-bench-metrics) | `bench_metrics` | Performance counters | — |

The iOS and web backends are left out, because Agentty ships on macOS, Windows and Linux. So are the `image` and `image_gallery` examples, which need the original's HTTP client.

## crates.io dependencies

A crate on crates.io can depend only on crates on crates.io. Where the original takes a dependency from Git or a fork, agentty-gpui uses the crates.io release:

| Crate | What the original's version adds |
|---|---|
| `async-task` | A fix for task waker vtable identity |
| `calloop` (Linux) | Idle callbacks capped at about 10 ms per loop |
| `khronos-egl` (Linux) | No panic when a failed EGL call reports success |
| `wgpu`, `naga` (Linux) | A newer API (`QueueDescriptor`, `has_srgb_suffix`) |
| `zed-font-kit`, `zed-scap`, `zed-xim` | Small fixes since |
| `oo7` (Linux) | An error when the prompt stream ends |
| `windows-capture` (Windows, `screen-capture`) | Nothing; held in `Cargo.lock` at the last release that builds with `zed-scap`, and a crate that turns on `screen-capture` must hold it too |

## How it is updated

There are two branches:

- **`main`**: the published code, with every change made for Agentty.
- **`upstream`**: the original code it is based on, unchanged, one commit per original release. `zed/SNAPSHOT.md` names the release.

Taking in a new original release:

```sh
scripts/sync-upstream.sh <tag>   # a new snapshot on upstream
git merge upstream                 # into main; only files changed here can conflict
python3 -I scripts/apply_agentty.py
```

If a crate manifest conflicts in the merge, take the original's side (`git checkout --theirs <file>`) and rerun the script.

## Tests

```sh
cargo nextest run --workspace --features agentty-gpui-apple/runtime_shaders
cargo test --workspace --doc --features agentty-gpui-apple/runtime_shaders
```

- Use nextest: a few tests read process-wide state and fail when `cargo test` runs them side by side.
- CI uses `--profile ci` (`.config/nextest.toml`), which retries a failed test twice, because a few bench tests time an 80 ms poll and can miss on a busy runner.
- Linux CI uses Mesa's software Vulkan driver for the wgpu renderer tests.

## Publishing

1. Bump `VERSION` in `scripts/apply_agentty.py`, rerun it and push. CI must pass, `cargo publish --workspace --dry-run` included.
2. Run the **Agentty** workflow in GitHub Actions. It builds and tests Agentty against this checkout on macOS, Windows and Linux.
3. `cargo publish --workspace` publishes the crates in dependency order.
4. Tag the release (`git tag v<version> && git push origin v<version>`).

## Reporting a problem

A bug in agentty-gpui, or a change you want in it, goes to [agentty-gpui's issues](https://github.com/empty-user77/agentty-gpui/issues). A bug in Agentty goes to [Agentty's](https://github.com/empty-user77/Agentty/issues). For how Agentty differs between operating systems, see [Platforms](/docs/platforms).
