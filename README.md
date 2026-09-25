<h1 align="center">
  <a href="https://www.agentty.run"><img src="docs/assets/logo.png" alt="Agentty" width="64" valign="middle" /></a> Agentty
</h1>

<p align="center">
  <a href="https://github.com/empty-user77/Agentty"><img src="https://img.shields.io/github/stars/empty-user77/Agentty?style=flat&amp;label=%E2%98%85&amp;color=08C" alt="GitHub stars" /></a>
  <a href="https://github.com/empty-user77/Agentty/releases"><img src="https://img.shields.io/github/downloads/empty-user77/Agentty/total?style=flat&amp;color=08C" alt="Downloads" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-08C?style=flat" alt="License: Apache-2.0" /></a>
  <a href="https://x.com/agentty_run"><img src="https://img.shields.io/badge/X-000000?logo=x&amp;logoColor=white" alt="Follow Agentty on X" /></a>
  <img src="https://img.shields.io/badge/macOS%20%7C%20Windows%20%7C%20Linux-4493F8?style=flat-square" alt="Supported platforms: macOS, Windows and Linux" />
</p>

<p align="center">
  <sub><b>English</b> · <a href="README.ko.md">한국어</a> · <a href="README.ja.md">日本語</a> · <a href="README.zh.md">中文</a></sub>
</p>

<p align="center">
  <strong>The orchestration terminal for AI-native workflows.</strong><br/>
  Run Claude Code, Codex and a dozen other agents side by side — see at a glance who is working, who is done and who needs you.
</p>

<h3 align="center"><a href="https://github.com/empty-user77/Agentty/releases/latest"><ins>Download Agentty</ins></a> · <a href="https://www.agentty.run/docs"><ins>Documentation</ins></a></h3>

<p align="center">
  <img src="docs/assets/banner.png" alt="Agentty" width="960" />
</p>

A native, GPU-rendered terminal written in Rust with [GPUI](https://gpui.rs) and `alacritty_terminal`. No Electron.
It gives every agent what the work needs: the project's files, its Git history, its containers and its databases.

## Features

<table>
<tr>
<td width="50%" valign="top">

### Every agent, one window

Workspaces, tabs and split panes for your terminals, Claude Code, Codex and any other CLI agent — all restored after a
restart.

[Docs →](https://www.agentty.run/docs/workspaces-tabs-panes)

</td>
<td width="50%" valign="top">

### Know who needs you

Each pane and workspace shows whether its agent is working, finished or waiting for you. Notifications reach you on
the desktop, in the menu bar, or in Slack, Discord and Telegram.

[Docs →](https://www.agentty.run/docs/agent-status)

</td>
</tr>
<tr>
<td width="50%" valign="top">

### Session Flow

Drag a line from one agent to another to hand over the context, keep it live in both directions, or move a
conversation from Claude Code to Codex and back.

[Docs →](https://www.agentty.run/docs/session-flow)

</td>
<td width="50%" valign="top">

### Parallel worktrees

A second agent in the same project starts in its own git worktree on a new branch, or joins the one in use — you
choose. An agent can also split work into several sessions once you confirm.

[Docs →](https://www.agentty.run/docs/agent-git)

</td>
</tr>
<tr>
<td width="50%" valign="top">

### AgentGit

Switch repositories and branches, review diffs, commit selected files, browse history, fetch, pull, push and merge —
without leaving the window your agents run in.

[Docs →](https://www.agentty.run/docs/agent-git)

</td>
<td width="50%" valign="top">

### In-app browser

Dev servers open beside your terminals, at phone and tablet sizes too, and an agent can drive the page to test what it
built.

[Docs →](https://www.agentty.run/docs/in-app-browser)

</td>
</tr>
<tr>
<td width="50%" valign="top">

### Databases &amp; Docker

Browse the databases your project already configures. Agents read inside a read-only transaction, and every write waits
for you to approve the exact statement. Containers, ports and logs sit next to them.

[Docs →](https://www.agentty.run/docs/databases)

</td>
<td width="50%" valign="top">

### Plugins &amp; automation

Plugins add panels, buttons and palette commands, bring other apps in, and run automation agents in a workspace of
their own. Find them in the [Marketplace](https://github.com/empty-user77/Agentty-Marketplace) or build your own.

[Docs →](https://www.agentty.run/docs/plugins-overview)

</td>
</tr>
</table>

**Also in the box:**

- **[Sessions](https://www.agentty.run/docs/sessions)** — Browse, search and resume the Claude Code and Codex sessions already on your computer.
- **[Monitoring](https://www.agentty.run/docs/monitoring)** — Cost, tokens and cache hit rate from local transcripts, running AI processes, every git worktree on the computer, and a disk page that clears build output.
- **[Extensions, MCP and connectors](https://www.agentty.run/docs/mcp-and-connectors)** — Skills, subagents, commands and MCP servers for Claude Code and Codex in one place, with secrets kept by the OS.
- **[Build my idea](https://www.agentty.run/docs/build-my-idea)** and **[Launch](https://www.agentty.run/docs/launch)** — Turn a few sentences into a project an agent can build, then take it live on GitHub, Vercel and Supabase.
- **[Claude advisor](https://www.agentty.run/docs/advisor)** — Let Claude Code consult a stronger model at the moments that matter, set per tab.
- **Files &amp; editor** — A project tree and an editor with syntax colors and formatters, plus "Open in editor" for VS Code or Cursor.
- **Command palette, unread jump and mini mode** — ⇧⌘P, ⇧⌘U and ⌃⌘M; see [Shortcuts](#shortcuts).

---

## Supported Agents

Works with **any CLI agent** — if it runs in a terminal, it runs in Agentty. Agentty detects these and offers them in its
launcher:

<p>
  <a href="https://docs.anthropic.com/en/docs/claude-code"><kbd><img src="https://www.google.com/s2/favicons?domain=claude.ai&amp;sz=64" alt="" width="16" valign="middle" /> Claude Code</kbd></a> &nbsp;
  <a href="https://github.com/openai/codex"><kbd><img src="https://www.google.com/s2/favicons?domain=openai.com&amp;sz=64" alt="" width="16" valign="middle" /> Codex</kbd></a> &nbsp;
  <a href="https://github.com/google-gemini/gemini-cli"><kbd><img src="https://www.google.com/s2/favicons?domain=gemini.google.com&amp;sz=64" alt="" width="16" valign="middle" /> Gemini CLI</kbd></a> &nbsp;
  <a href="https://antigravity.google"><kbd><img src="https://www.google.com/s2/favicons?domain=antigravity.google&amp;sz=64" alt="" width="16" valign="middle" /> Antigravity CLI</kbd></a> &nbsp;
  <a href="https://ampcode.com"><kbd><img src="https://www.google.com/s2/favicons?domain=ampcode.com&amp;sz=64" alt="" width="16" valign="middle" /> Amp</kbd></a> &nbsp;
  <a href="https://docs.github.com/en/copilot/how-tos/set-up/install-copilot-cli"><kbd><img src="https://www.google.com/s2/favicons?domain=github.com&amp;sz=64" alt="" width="16" valign="middle" /> GitHub Copilot CLI</kbd></a> &nbsp;
  <a href="https://cursor.com/cli"><kbd><img src="https://www.google.com/s2/favicons?domain=cursor.com&amp;sz=64" alt="" width="16" valign="middle" /> Cursor CLI</kbd></a> &nbsp;
  <a href="https://opencode.ai"><kbd><img src="https://www.google.com/s2/favicons?domain=opencode.ai&amp;sz=64" alt="" width="16" valign="middle" /> OpenCode</kbd></a> &nbsp;
  <a href="https://github.com/QwenLM/qwen-code"><kbd><img src="https://www.google.com/s2/favicons?domain=qwenlm.github.io&amp;sz=64" alt="" width="16" valign="middle" /> Qwen Code</kbd></a> &nbsp;
  <a href="https://docs.factory.ai/cli/getting-started/quickstart"><kbd><img src="https://www.google.com/s2/favicons?domain=factory.ai&amp;sz=64" alt="" width="16" valign="middle" /> Factory Droid</kbd></a> &nbsp;
  <a href="https://block.github.io/goose/"><kbd><img src="https://www.google.com/s2/favicons?domain=goose-docs.ai&amp;sz=64" alt="" width="16" valign="middle" /> Goose</kbd></a> &nbsp;
  <a href="https://github.com/charmbracelet/crush"><kbd><img src="https://www.google.com/s2/favicons?domain=charm.sh&amp;sz=64" alt="" width="16" valign="middle" /> Crush</kbd></a> &nbsp;
  <a href="https://aider.chat"><kbd><img src="https://aider.chat/assets/icons/favicon-32x32.png" alt="" width="16" valign="middle" /> Aider</kbd></a> &nbsp;
  <a href="https://github.com/MoonshotAI/kimi-cli"><kbd><img src="https://www.google.com/s2/favicons?domain=moonshot.cn&amp;sz=64" alt="" width="16" valign="middle" /> Kimi CLI</kbd></a> &nbsp;
  <a href="https://kiro.dev/docs/cli/"><kbd><img src="https://www.google.com/s2/favicons?domain=kiro.dev&amp;sz=64" alt="" width="16" valign="middle" /> Kiro CLI</kbd></a> &nbsp;
  <a href="https://docs.cline.bot/cline-cli/overview"><kbd><img src="https://www.google.com/s2/favicons?domain=cline.bot&amp;sz=64" alt="" width="16" valign="middle" /> Cline CLI</kbd></a> &nbsp;
  <a href="https://x.ai/cli"><kbd><img src="https://www.google.com/s2/favicons?domain=x.ai&amp;sz=64" alt="" width="16" valign="middle" /> Grok Build</kbd></a> &nbsp;
  <a href="https://ollama.com"><kbd><img src="https://www.google.com/s2/favicons?domain=ollama.com&amp;sz=64" alt="" width="16" valign="middle" /> Ollama (local models)</kbd></a> &nbsp;
  <kbd>+ any CLI agent</kbd>
</p>

Claude Code and Codex get first-class panes: live status from their hooks, their sessions, usage and cost, and
handoffs between the two.

---

## Install

### macOS — Apple silicon, macOS 13+

```sh
brew install --cask empty-user77/agentty/agentty
```

That's it — Agentty is in your Applications folder. No Homebrew yet? Install it with the one-line command on
[brew.sh](https://brew.sh) first. Agentty updates itself; `brew upgrade --cask agentty` works too, and
`brew uninstall --cask agentty` removes it.

Prefer a download? Get `Agentty-X.Y.Z-…-arm64.dmg` from the
[latest release](https://github.com/empty-user77/Agentty/releases/latest), open it and drag **Agentty** into
**Applications**. It is signed and notarized by Apple. There is no Intel Mac build.

### Windows — 10 version 1809+, x64

1. Download `Agentty-X.Y.Z-windows-x64-setup.exe` from the
   [latest release](https://github.com/empty-user77/Agentty/releases/latest). If your browser blocks the `.exe`, take
   `Agentty-X.Y.Z-windows-x64-setup.zip` and unzip it: it holds the same installer.
2. Run it. It installs for your user only and needs no administrator rights.
3. The installer is not code-signed yet. If SmartScreen appears, choose **More info → Run anyway**.

To uninstall: Settings → Apps → Agentty → Uninstall.

### Linux — x86_64

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

---

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

---

## Community &amp; Support

- **X:** Follow **[@agentty_run](https://x.com/agentty_run)** for updates and announcements.
- **Feedback &amp; ideas:** Missing something or found a bug? [Open an issue](https://github.com/empty-user77/Agentty/issues).
- **Security:** Report vulnerabilities privately as described in [SECURITY.md](SECURITY.md).
- **Contributing:** Read [CONTRIBUTING.md](CONTRIBUTING.md) and the [Code of Conduct](CODE_OF_CONDUCT.md).
- **Privacy:** Agent transcripts are read **locally** to list sessions, compute usage and build handoffs; prompts,
  output, paths and repository data are never uploaded. Official release builds send anonymous usage statistics (which
  features were used, the app and system version, and a random install ID). Turn it off in Settings → General or with
  `DO_NOT_TRACK=1`; builds from source send nothing. Every event is listed in [docs/metrics.md](docs/metrics.md).
- **Show support:** [Star](https://github.com/empty-user77/Agentty) this repo to follow along.

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
