<p align="center">
  <img src="docs/assets/banner.png" alt="Agentty" width="100%">
</p>

<p align="center"><b>The orchestration terminal for AI-native workflows.</b></p>

<p align="center">
  <a href="https://www.agentty.run">agentty.run</a> ·
  <a href="https://www.agentty.run/docs">Guide</a> ·
  <a href="https://github.com/empty-user77/agentty-releases/releases">Download</a> ·
  <a href="https://x.com/agentty_run">@agentty_run</a>
</p>

<p align="center">
  <b>English</b> ·
  <a href="README.ko.md">한국어</a> ·
  <a href="README.ja.md">日本語</a> ·
  <a href="README.zh.md">中文</a>
</p>

<p align="center">
  <a href="LICENSE"><img alt="License: GPL-3.0-or-later" src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue.svg"></a>
  <img alt="Rust 1.98" src="https://img.shields.io/badge/rust-1.98-orange.svg">
  <img alt="Platform: macOS | Windows | Linux" src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey.svg">
</p>

Agentty is a native, GPU-rendered terminal for running **Claude Code**, **Codex** and your shells side by side. It
shows which agent is working, which one finished and which one is waiting for you, and gives each one what the work
needs: the project's files, its Git history, its containers and its databases.

Written in Rust with [GPUI](https://gpui.rs) and `alacritty_terminal`. No Electron.

## Why Agentty

- **Every agent in one window** — workspaces, tabs and split panes, restored after a restart. Terminal, Claude Code,
  Codex and a dozen other CLIs (Gemini, Copilot, Cursor, OpenCode, Amp, Aider, local Ollama models…) start from the
  same menu.
- **It can tell them apart** — live status per pane and workspace: working, finished, or waiting for you. The pane
  that needs you lights up, and notifications reach you on the desktop, in the menu bar, or in Slack, Discord or
  Telegram when you are away.
- **Sessions share context** — drag a line between two agents to hand a conversation over, keep it live in both
  directions, or migrate a session from Claude Code to Codex and back.
- **Parallel work without collisions** — a second agent in the same project can start in its own git worktree on a new
  branch (or join the one in use — you are asked), and an agent can split work into several sessions at once once you confirm.
- **More than a terminal** — files panel with an editor, a GitHub Desktop-style Git page, a Docker panel, a database
  page, an in-app browser with a responsive mode for phone and tablet sizes, usage and cost monitoring, a list of every git worktree on the computer (with what is
  uncommitted or merged, and removing several at once), a disk page that clears build output and tool caches, and an
  extensions page for skills, subagents and MCP servers.
- **Careful with your data** — agents read databases inside a read-only transaction and every write waits for your
  approval of the exact statement; secrets live in the OS credential store; transcripts and prompts never leave the
  machine.

The full feature guide lives at **[agentty.run/docs](https://www.agentty.run/docs)**.

## Install

Download from [agentty-releases](https://github.com/empty-user77/agentty-releases/releases):

| Platform | File |
|---|---|
| macOS 13+ (Apple silicon) | `Agentty-X.Y.Z-…-arm64.dmg` — signed and notarized |
| Windows 10 1809+ (x64) | `Agentty-X.Y.Z-windows-x64-setup.exe` — per user, no admin rights |
| Debian 12+ / Ubuntu 22.04+ | `Agentty-X.Y.Z-linux-amd64.deb` — `sudo apt install ./Agentty-*.deb` |
| RHEL 9+ / Fedora | `Agentty-X.Y.Z-linux-x86_64.rpm` — `sudo dnf install ./Agentty-*.rpm` |

On macOS you can also install with [Homebrew](https://brew.sh):

```sh
brew install --cask empty-user77/agentty/agentty
```

Agentty updates itself: on macOS and Windows it installs the new version and restarts, on Linux it points at the new
packages.

### Build from source

```sh
git clone https://github.com/empty-user77/Agentty.git
cd Agentty
cargo run --release -p agentty-app
```

Rust 1.98 (pinned in `rust-toolchain.toml`) and the platform's build tools are needed — see
[docs/platforms.md](docs/platforms.md) for Windows and Linux. `claude` and/or `codex` on your `PATH` gives you agent
tabs; Settings → System installs what is missing.

## Documentation

| | |
|---|---|
| [Feature guide](https://www.agentty.run/docs) | What every part of the app does |
| [docs/configuration.md](docs/configuration.md) | `settings.json`, agent sign-in, harnesses, databases, themes, and the files Agentty keeps |
| [docs/platforms.md](docs/platforms.md) | What differs on Windows and Linux, and how to build there |
| [docs/plugins](docs/plugins/README.md) | Writing a plugin (Node.js SDK, or [the protocol](docs/plugins/protocol.md)) and [using one](docs/plugins/usage.md) |
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

The full list is in **Settings → Keyboard Shortcuts**. On Windows and Linux ⌘ becomes Ctrl+Shift, ⇧⌘ becomes
Ctrl+Alt+Shift and ⌥⌘ becomes Ctrl+Alt, so Ctrl+letter stays with the shell.

## Plugins

A plugin fills a panel next to your terminals, adds buttons above agent panes and command palette entries, and can
send prompts — always after you pick where, in a "Send to…" dialog. Plugins run as separate processes and may only do
what their manifest declares; the Plugins page shows that list before you install. The built-in **Cosmica** plugin
turns [Cosmica](https://www.cosmica.ink/) notes into prompts and saves session summaries back into it.

See [docs/plugins](docs/plugins/README.md) — the Plugins page can also create one and let Claude Code build it.

## Privacy

Agent transcripts are read **locally** to list sessions, compute usage and build handoffs. Prompts, output, paths and
repository data are never uploaded, and the agent status hooks talk to a socket private to your account.

Official release builds send anonymous usage statistics (which features were used, the app and system version, a
random install ID). Settings → General turns it off, `DO_NOT_TRACK=1` does too, and
builds from source send nothing. Every event is listed in [docs/metrics.md](docs/metrics.md).

## Contributing

Contributions are welcome — please read [CONTRIBUTING.md](CONTRIBUTING.md) and the
[Code of Conduct](CODE_OF_CONDUCT.md). Security issues: [SECURITY.md](SECURITY.md).

## License

Agentty is free software: you can redistribute it and/or modify it under the terms of the
[GNU General Public License](LICENSE), version 3 or (at your option) any later version.

Bundled fonts and third-party components keep their own licenses; see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
