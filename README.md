<p align="center">
  <img src="docs/assets/banner.png" alt="Agentty" width="100%">
</p>

<p align="center"><b>The orchestration terminal for AI-native workflows.</b><br>나를 위한 멀티 오케스트레이션 AI 터미널</p>

<p align="center"><a href="https://www.agentty.run">agentty.run</a></p>

<p align="center">
  <a href="LICENSE"><img alt="License: GPL-3.0-or-later" src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue.svg"></a>
  <img alt="Rust 1.98" src="https://img.shields.io/badge/rust-1.98-orange.svg">
  <img alt="Platform: macOS | Windows | Linux" src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey.svg">
</p>

Agentty is a native, GPU-rendered terminal for running shells, **Claude Code** and
**Codex** side by side. Workspaces, tabs and split panes keep many agent sessions organized; the sidebar shows what
each agent is doing and highlights the ones that need you.

## Features

- **Native & fast** — Rust, [GPUI](https://gpui.rs) (Metal) rendering and `alacritty_terminal` emulation. No Electron.
- **Workspaces → tabs → splits** — each sidebar row is a workspace; group workspaces, drag them between groups, rename
  them, and pick up where you left off after a restart.
- **Agent awareness** — live status per workspace (working / done / waiting for input). When an agent finishes or asks
  a question, its pane border lights up until you click it.
- **Instant launch** — new Terminal, Claude Code (Opus / Sonnet / Haiku) or Codex (your recent models) tabs, starting
  in a folder you choose. Other installed agents (Gemini CLI, Copilot CLI, Cursor CLI, Grok Build, OpenCode, Qwen
  Code, Amp, Droid, Goose, Crush, Aider) and local Ollama models are detected and offered; missing ones stay hidden.
- **Agent harnesses** — entering a project that declares a harness (`.harness`, `HARNESS.md`, `harness.yaml`, a project skill named
  `harness` or `harness-*`, …)
  offers to start work through its commands and skills: pick an entry point such as `/implement`, paste a ticket key or link, and the
  agent starts with it. Detection patterns are configurable in Settings → Project.
- **Local sessions** — browse `~/.claude` and `~/.codex` sessions, resume them, or migrate a conversation from Claude
  to Codex (and back).
- **Session Flow** — drag a line from one agent session to another to share its context, across Claude Code and Codex.
  Mark a link **Live** to forward every new turn automatically, or make it two-way for a context tunnel.
- **Mini mode & menu bar** — fold the window into a small always-on-top panel at the screen edge (⌃⌘M); finished
  agents pop up as speech bubbles, a click brings the full window back. The menu bar icon animates while agents work,
  lists them, and keeps Agentty running after you close the window.
- **Pane status bar** — model, context window and rate-limit usage meters, branch and folder for every agent pane; under the terminal or above it, items in your order.
- **Notifications** — native macOS notifications when an agent finishes or needs input (click to open the pane), a
  notification center, jump to the latest unread (⇧⌘U), and `agentty notify` for your own scripts. When an agent needs
  your answer you are told even while Agentty is in front (unless you are looking at that pane), and Settings →
  Notifications can send a message to **Slack**, **Discord** (webhooks) or **Telegram** (a bot) as well — handy when
  you are away from the computer. Webhook URLs and the bot token are kept in the Keychain; what the agent asks is only
  included if you turn that on.
- **Git** — a GitHub Desktop–style page: switch repositories and branches, fetch / pull / push, review diffs, commit
  selected files, browse history and merge branches. Click the branch in any pane header to switch branches, pull, push or copy its name.
- **Extensions** — skills, subagents, commands, plugins and MCP servers for Claude Code and Codex in one place; add
  official MCP servers in one click and insert or run any of them in the active agent.
- **API connectors** — expose any HTTP API to your agents as an MCP server; secrets live in the OS credential store
  (macOS Keychain, Windows Credential Manager, Linux Secret Service).
- **Accounts without the CLI login** — Settings → Accounts starts Claude Code with an API key, a gateway token
  (`ANTHROPIC_AUTH_TOKEN` + base URL), a `claude setup-token` OAuth token, Amazon Bedrock or Google Vertex AI, and
  Codex with an API key or an imported `auth.json` — for machines where `claude` / `codex login` can't sign in.
- **Plugins** — connect other apps and add tools: a plugin fills a panel next to your terminals, adds buttons above
  agent panes and palette commands, sends prompts to new or open workspaces, and handles `agentty://` links from other
  apps. The built-in **Cosmica** plugin turns Cosmica notes into prompts ("Continue in Agentty" from Cosmica) and saves
  AI session summaries back into Cosmica. Build your own with the Node.js SDK, or let Claude Code build it from the
  Plugins page — see [docs/plugins](docs/plugins/README.md).
- **Monitoring** — AI usage (cost, calls, cache hit rate, tokens, models, projects and tools from local transcripts),
  the AI processes that are running, and a capture proxy that lists what your tabs talk to: start capture, open a tab,
  filter by endpoint. HTTPS stays encrypted (host, bytes and timing only); nothing is written to disk.
- **Files panel & working trees** — the folder structure and changes of the project you are in, docked right (⌥⌘B).
  A second AI session in the same project, opened from the + menu or typed into a terminal, starts in its own git
  worktree on a new branch from the project's default branch, so sessions never edit the same files; the panel shows
  every working tree, who works in it and what changed there. Right-click a tree to open a terminal there, copy its
  path, or remove it (and its branch once merged).
  every working tree, who works in it and what changed there.
- **File editor** — click a file in the files panel to view and edit it in a tab next to your terminals: syntax
  colors for Java, Kotlin, JavaScript, TypeScript / TSX, JSON, YAML, TOML, Markdown, Rust, Python, Go, shell, HTML,
  CSS, SQL and more, undo / redo, IME input, save (⌘S), and **Format** with the formatter you have installed
  (Prettier, google-java-format, ktlint, rustfmt, gofmt, Black). Files changed by an agent reload by themselves.
  **Open in editor** hands the file to VS Code, Cursor or the system's text editor. Files are only ever read and
  written — never run.
- **Docker** — in a project with a compose file, a Dockerfile or containers of its own, the status bar shows how many
  of its containers run (🐳 3 running). Click it for a panel with each compose service's image, state (running,
  stopped, unhealthy) and ports; start, stop, restart, `compose up -d` and `compose down` (asked twice, volumes kept),
  and logs in a new tab. Environment values from compose files are never shown.
- **Parallel tasks** — ask an agent to split work up ("do A, B and C in parallel"): it asks Agentty
  (`agentty tasks`), you confirm once, and each task starts right away in a split pane of the current tab, in its own
  worktree from the project's default branch. Agents started in Agentty are told what Agentty offers them (a short
  guide and skills passed on the command line; nothing is written into your projects or settings).
- **Resume where you left off** — `cd` into a folder with earlier Claude Code / Codex sessions and a bar offers to
  continue them in place. Search local sessions by title, conversation or path (⇧⌘O).
- **Service status** — when Claude Code, Codex or Amp is degraded while you use it, a banner links to its status page.
- **In-app browser** — a WebKit panel next to your terminals (⇧⌘B). ⌘-click links in terminals to open them there or in
  your default browser (Settings → General). Dev servers show up as `:port` chips in the workspace list, open in the
  in-app browser as soon as they answer, and are stopped when the tab that started them closes.
- **Terminal essentials** — find in scrollback (⌘F), font zoom (⌘= / ⌘- / ⌘0), drop files to paste their paths, mouse
  reporting for full-screen apps (Claude Code, vim, htop) with Shift to select.
- **Status bar inventory** — skills, subagents and MCP servers (with live connection state) of the agent in the current tab.
- **Auto-update** — checks the release channel at launch and hourly; updates install and relaunch on macOS (signed,
  notarized) and Windows (checksum-verified installer); Linux links to the new packages.
- **Command palette & reserved words** — fuzzy command palette (⇧⌘P) with custom commands from `agentty.json`, and
  shortcuts such as `claude yolo` → `claude --dangerously-skip-permissions`.
- **Favorites** — star local sessions to pin them to the top.
- **Make it yours** — themes (plus iTerm2 `.itermcolors` import), fonts, cursor, padding; English, 한국어, 日本語, 中文.
- **Nerd Font ready** — JetBrains Mono and Nerd Font symbols are bundled, so Powerlevel10k / Starship prompts render.

## Install

Download from [agentty-releases](https://github.com/empty-user77/agentty-releases/releases):

| Platform | File |
|---|---|
| macOS 13+ (Apple silicon) | `Agentty-X.Y.Z-release…-arm64.dmg` — notarized |
| Windows 10 1809+ (x64) | `Agentty-X.Y.Z-windows-x64-setup.exe`, or the same installer in `…-setup.zip` — per user, no admin rights; not code-signed yet (SmartScreen: More info → Run anyway) |
| Debian 12+ / Ubuntu 22.04+ (x86_64) | `Agentty-X.Y.Z-linux-amd64.deb` — `sudo apt install ./Agentty-*.deb` |
| RHEL 9+ / Fedora (x86_64) | `Agentty-X.Y.Z-linux-x86_64.rpm` — `sudo dnf install ./Agentty-*.rpm` |

Or build from source:

```sh
git clone https://github.com/empty-user77/Agentty.git
cd Agentty
cargo run --release -p agentty-app
```

Requirements: macOS 13+, Rust 1.98 (pinned in `rust-toolchain.toml`), Xcode command line tools. `claude` and/or `codex` on your `PATH` for agent tabs.
Packaging, signing and notarization: see [docs/release.md](docs/release.md).

### Windows and Linux

Agentty also builds and runs on Windows 10/11 and Linux (X11 and Wayland) — see [docs/platforms.md](docs/platforms.md)
for what differs.

- **Windows**: `cargo build --release -p agentty-app`, or `pwsh scripts/package-windows.ps1` for a zip (`-Installer`
  also builds the setup program; needs Inno Setup 6). Panes run
  PowerShell (`pwsh` when installed); Claude Code needs Git for Windows for its hooks.
- **Linux**: install the GPUI libraries (Debian/Ubuntu: `sudo apt install pkg-config libxkbcommon-dev
  libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev libvulkan-dev libfontconfig-dev libzstd-dev`), then
  `cargo build --release -p agentty-app`, or `scripts/package-linux.sh` for a tarball with `install.sh`.
  `scripts/build-linux-packages.sh` builds the x86_64 `.deb` and `.rpm` in Docker from any machine.

Shortcuts are the same with ⌘ → Ctrl+Shift, ⇧⌘ → Ctrl+Alt+Shift, ⌥⌘ → Ctrl+Alt and ⌘1…9 → Alt+1…9, so Ctrl+letter
stays with the shell.

## Keyboard shortcuts

| Action | Shortcut |
|---|---|
| New terminal / Claude Code / Codex tab | ⌘T / ⌥⌘C / ⌥⌘X |
| New workspace | ⌘N |
| Split right / down | ⌘D / ⇧⌘D |
| Next / previous pane | ⌘] / ⌘[ |
| Next / previous tab | ⇧⌘] / ⇧⌘[, ⌃Tab |
| Next / previous workspace | ⌥⌘↓ / ⌥⌘↑ |
| Close pane / tab | ⌘W / ⇧⌘W |
| Toggle sidebar | ⌘B |
| Workspaces / Local sessions | ⇧⌘E / ⇧⌘S |
| Git / Extensions | ⇧⌘G / ⇧⌘X |
| Session Flow / Monitoring / Settings | ⇧⌘F / ⌥⌘U / ⌘, |
| In-app browser / Files panel | ⇧⌘B / ⌥⌘B |
| Command palette / Jump to unread | ⇧⌘P / ⇧⌘U |
| Mini mode | ⌃⌘M |
| Go to workspace / tab 1–9 | ⌘1…⌘9 / ⌃1…⌃9 |
| Search local sessions | ⇧⌘O |
| Git page: commit / push / fetch / refresh | ⌘↩ / ⌘P / ⇧⌘T / ⌘R |
| File editor: save / format / close file | ⌘S / ⇧⌥F / ⌘W |
| Copy / paste / select all / clear | ⌘C / ⌘V / ⌘A / ⌘K |

The full list is in **Settings → Keyboard Shortcuts**.

Double-click the title bar to zoom the window; double-click empty space in the tab strip to open a new tab.

## Plugins

Plugins connect Agentty with other apps and add tools to your terminals: a panel next to the
terminals, buttons above agent panes, command palette entries, and prompts sent to a new workspace, a
new tab or an open workspace — always after you pick where in a "Send to…" dialog. Open **Plugins**
(puzzle icon) to install, enable, restart or remove them.

The built-in **Cosmica** plugin turns [Cosmica](https://www.cosmica.ink/) notes into prompts
(including "Continue in Agentty" from Cosmica itself) and saves AI session summaries back into it. Other apps can hand work to Agentty
with `agentty://` links.

Plugins run as separate processes and may only do what their manifest declares: send prompts, type
into terminals, read AI conversations, see workspaces. The Plugins page shows that list before you
install.

- [How to use plugins](docs/plugins/usage.md)
- [Writing a plugin](docs/plugins/README.md) — Node.js SDK, or [the protocol](docs/plugins/protocol.md) for any language.
  The Plugins page can also create one and have Claude Code build it.

## Files

| Path | Purpose |
|---|---|
| `~/.agentty/settings.json` | Preferences (language, theme, font, …) |
| `~/.agentty/workspaces.json` | Saved workspaces, tabs, splits and groups |
| `~/.agentty/themes/*.itermcolors` | Imported color themes |
| `~/.agentty/pricing.json` | Optional model prices for non-Claude models |
| `~/.agentty/handoffs/` | Context documents created by migrations and Session Flow |
| `~/.agentty/connectors.json` | API connector definitions (secrets are in the Keychain, never in this file) |
| `~/.agentty/agent-auth.json` | Sign-in method for new Claude Code / Codex tabs (Settings → Accounts; no secrets) |
| `~/.agentty/codex-home/` | Private Codex home for API-key / `auth.json` sign-in (`auth.json` `0600`; config and sessions link to `~/.codex`) |
| `agentty.json` (project) or `~/.agentty/commands.json` | Custom command palette entries |
| `~/.agentty/plugins/` | Installed plugins (`state.json` records which are enabled) |
| `~/.agentty/plugin-data/<id>/` | Private data of each plugin |
| `~/.agentty/prompts/` | Long prompts from plugins and links, handed to agents as files |

See [docs/configuration.md](docs/configuration.md) for details and [docs/architecture.md](docs/architecture.md) for how
the pieces fit together.

## Privacy

Agentty reads agent transcripts **locally** to list sessions, compute usage and build handoffs. Transcripts, prompts,
output, paths and repository data are never uploaded. Agent status hooks talk to a per-process Unix socket in your
private temporary directory (mode 0600). The Git page runs your own `git` with your existing configuration and
credentials.

Official release builds send **anonymous usage statistics** (event names such as `pane_opened` with the agent name, the
app and macOS version, and a random install ID) — see [docs/metrics.md](docs/metrics.md) for the full list. Turn it off
with `DO_NOT_TRACK=1`. Builds from source send nothing.
The update check downloads the release manifest from GitHub.

## Contributing

Contributions are welcome — please read [CONTRIBUTING.md](CONTRIBUTING.md) and our
[Code of Conduct](CODE_OF_CONDUCT.md). Security issues: see [SECURITY.md](SECURITY.md).

## License

Agentty is free software: you can redistribute it and/or modify it under the terms of the
[GNU General Public License](LICENSE), version 3 or (at your option) any later version.

Bundled fonts and third-party components keep their own licenses; see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
