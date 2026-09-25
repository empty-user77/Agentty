<p align="center">
  <img src="docs/assets/banner.png" alt="Agentty" width="100%">
</p>

<p align="center"><b>The orchestration terminal for AI-native workflows.</b></p>

<p align="center">
  <a href="https://www.agentty.run">agentty.run</a> ·
  <a href="https://www.agentty.run/docs">Documentation</a> ·
  <a href="https://github.com/empty-user77/Agentty/releases">Download</a> ·
  <a href="https://x.com/agentty_run">@agentty_run</a>
</p>

<p align="center">
  <b>English</b> ·
  <a href="README.ko.md">한국어</a> ·
  <a href="README.ja.md">日本語</a> ·
  <a href="README.zh.md">中文</a>
</p>

<p align="center">
  <a href="LICENSE"><img alt="License: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-blue.svg"></a>
  <img alt="Rust 1.98" src="https://img.shields.io/badge/rust-1.98-orange.svg">
  <img alt="Platform: macOS | Windows | Linux" src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey.svg">
</p>

Agentty is a native, GPU-rendered terminal for running **Claude Code**, **Codex** and your shells side by side. It
shows which agent is working, which one has finished and which one is waiting for you, and gives each of them what the
work needs: the project's files, its Git history, its containers and its databases.

Written in Rust with [GPUI](https://gpui.rs) and `alacritty_terminal`. No Electron.

## Features

- **Every agent in one window.** Workspaces, tabs and split panes, restored after a restart. A terminal, Claude Code,
  Codex and a dozen other CLIs (Gemini, Copilot, Cursor, OpenCode, Amp, Aider, local Ollama models and more) start from
  the same menu.
- **Status at a glance.** Each pane and workspace shows whether its agent is working, finished or waiting for you. The
  pane that needs you lights up, and notifications reach you on the desktop, in the menu bar, or in Slack, Discord or
  Telegram when you are away.
- **Shared context.** Drag a line between two agents to hand a conversation over, keep it live in both directions, or
  move a session from Claude Code to Codex and back.
- **Parallel work without collisions.** A second agent in the same project can start in its own git worktree on a new
  branch, or join the one in use; you choose. An agent can also split work into several sessions once you confirm.
- **More than a terminal.** A files panel with an editor, a Git page, Docker and database panels, an in-app browser with
  phone and tablet sizes, usage and cost monitoring, every git worktree on the computer, a disk page that clears build
  output and caches, and a page for skills, subagents and MCP servers.
- **Careful with your data.** Agents read databases inside a read-only transaction, and every write waits for you to
  approve the exact statement. Secrets live in the OS credential store. Transcripts and prompts never leave the machine.

## Install

### macOS (Apple silicon, macOS 13+)

The easiest way is [Homebrew](https://brew.sh):

```sh
brew install --cask empty-user77/agentty/agentty
```

That's it — Agentty is in your Applications folder. If you don't have Homebrew yet, install it first with the one-line
command on [brew.sh](https://brew.sh), then run the command above.

| | Command |
|---|---|
| Update | Agentty updates itself; `brew upgrade --cask agentty` also works |
| Uninstall | `brew uninstall --cask agentty` |

Prefer a download? Get `Agentty-X.Y.Z-…-arm64.dmg` from the
[latest release](https://github.com/empty-user77/Agentty/releases/latest), open it and drag **Agentty** into
**Applications**. The app is signed and notarized by Apple, so it opens without a warning. There is no Intel Mac build.

### Windows (10 version 1809+, x64)

1. Download `Agentty-X.Y.Z-windows-x64-setup.exe` from the
   [latest release](https://github.com/empty-user77/Agentty/releases/latest). If your browser blocks the `.exe`, take
   `Agentty-X.Y.Z-windows-x64-setup.zip` and unzip it: it holds the same installer.
2. Run it. It installs for your user only and needs no administrator rights.
3. The installer is not code-signed yet. If SmartScreen appears, choose **More info → Run anyway**.

To uninstall: Settings → Apps → Agentty → Uninstall.

### Linux (x86_64)

Download the package for your distribution from the
[latest release](https://github.com/empty-user77/Agentty/releases/latest), then:

```sh
# Debian 12+ / Ubuntu 22.04+
sudo apt install ./Agentty-*-linux-amd64.deb

# RHEL 9+ / Fedora
sudo dnf install ./Agentty-*-linux-x86_64.rpm
```

To uninstall: `sudo apt remove agentty` or `sudo dnf remove agentty`.

### Updates

Agentty checks for new versions at launch and every hour. On macOS and Windows it downloads the update, checks it
against the release checksums, installs it and restarts. On Linux it tells you a new version is out and links to the
packages.

### Before you start

Agentty runs your agents; it doesn't bundle them. Install the CLIs you want to use, such as
[Claude Code](https://docs.anthropic.com/en/docs/claude-code) (`claude`) and [Codex](https://github.com/openai/codex)
(`codex`), and make sure they are on your `PATH`. Settings → System in Agentty can install what's missing.

### Build from source

```sh
git clone https://github.com/empty-user77/Agentty.git
cd Agentty
cargo run --release -p agentty-app
```

You need Rust 1.98 (pinned in `rust-toolchain.toml`) and your platform's build tools; see
[docs/platforms.md](docs/platforms.md) for Windows and Linux.

## Documentation

| | |
|---|---|
| [agentty.run/docs](https://www.agentty.run/docs) | What every part of the app does |
| [docs/en](docs/en/introduction.md) | The same user documentation as Markdown ([한국어](docs/ko/introduction.md) · [日本語](docs/ja/introduction.md) · [中文](docs/zh/introduction.md)) |
| [docs/configuration.md](docs/configuration.md) | `settings.json`, agent sign-in, harnesses, databases, themes, and the files Agentty keeps |
| [docs/platforms.md](docs/platforms.md) | What differs on Windows and Linux, and how to build there |
| [docs/plugins](docs/plugins/README.md) | Writing a plugin (Node.js SDK or [the protocol](docs/plugins/protocol.md)) and [using one](docs/plugins/usage.md) |
| [docs/architecture.md](docs/architecture.md) | How the crates, the terminal, the agent socket and the panels fit together |
| [docs/metrics.md](docs/metrics.md) | Every event an official build may send, and how to turn it off |
| [docs/release.md](docs/release.md) | Packaging, signing, notarization and the release channel |

## Shortcuts

| Action | Shortcut |
|---|---|
| New terminal / Claude Code / Codex tab | ⌘T / ⌥⌘C / ⌥⌘X |
| Split right / down | ⌘D / ⇧⌘D |
| Next / previous pane · tab · workspace | ⌘] ⌘[ · ⇧⌘] ⇧⌘[ · ⌥⌘↓ ⌥⌘↑ |
| Command palette · jump to unread | ⇧⌘P · ⇧⌘U |
| Git · Session Flow · Monitoring · Settings | ⇧⌘G · ⇧⌘F · ⌥⌘U · ⌘, |
| Files panel · in-app browser · mini mode | ⌥⌘B · ⇧⌘B · ⌃⌘M |

The full list is in **Settings → Keyboard Shortcuts**. On Windows and Linux, ⌘ becomes Ctrl+Shift, ⇧⌘ becomes
Ctrl+Alt+Shift and ⌥⌘ becomes Ctrl+Alt, so Ctrl+letter stays with the shell.

## Plugins

A plugin can fill a panel next to your terminals, add buttons above agent panes and entries to the command palette, and
send prompts, always to a place you pick in a "Send to…" dialog. The Plugins page shows the permissions a plugin asks
for before you install it; they limit what it can ask Agentty to do. A WebAssembly plugin can reach nothing beyond
them. A plugin that runs as a Node.js, Python or native program also has your own access to files and the network
outside Agentty, so install only the ones you trust. Plugins are published through the
[Agentty Marketplace](https://github.com/empty-user77/Agentty-Marketplace).

The built-in **Cosmica** plugin turns [Cosmica](https://www.cosmica.ink/) notes into prompts and saves session summaries
back into it. To write your own, see [docs/plugins](docs/plugins/README.md); the Plugins page can also create one and
let Claude Code build it.

## Privacy

Agent transcripts are read **locally** to list sessions, compute usage and build handoffs. Prompts, output, paths and
repository data are never uploaded, and the agent status hooks talk to a socket private to your account.

Official release builds send anonymous usage statistics: which features were used, the app and system version, and a
random install ID. Turn it off in Settings → General or with `DO_NOT_TRACK=1`. Builds from source send nothing. Every
event is listed in [docs/metrics.md](docs/metrics.md).

## Contributing

Contributions are welcome. Please read [CONTRIBUTING.md](CONTRIBUTING.md) and the [Code of Conduct](CODE_OF_CONDUCT.md).
Report security issues as described in [SECURITY.md](SECURITY.md).

## License

```
Copyright 2026 LEE YONGBEOM

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

   http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
```

The full text is in [LICENSE](LICENSE). Bundled fonts and third-party components keep their own licenses; see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
