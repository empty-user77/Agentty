# Platforms

Agentty is developed on macOS, which remains the reference platform. Windows and Linux builds share every feature
that doesn't depend on AppKit or WebKit.

## What differs

| | macOS | Windows | Linux |
|---|---|---|---|
| Pane shell | `$SHELL` (zsh) | PowerShell (`pwsh`, else Windows PowerShell); `AGENTTY_SHELL` overrides | `$SHELL` (bash fallback) |
| Agent status hooks | `nc -U` to the Unix socket | `agentty signal` to a token-guarded loopback port | `agentty signal` to the Unix socket |
| Credential store | Keychain | Credential Manager | Secret Service (`secret-tool`), else `~/.agentty/secrets/` (`0600`) |
| Notifications | UserNotifications (click opens the pane) | Toast | `notify-send` |
| Title bar | Agentty's, with traffic lights | System | System; Agentty's with window buttons when the compositor has no server-side decorations |
| In-app browser | WKWebView | Default browser | Default browser |
| Menu bar item, mini mode | ✓ | — | — |
| Reserved words (`claude zzzz`) | zsh / bash | PowerShell | zsh / bash |
| Foreground program, cwd, ports | ✓ | Launch kind only | `/proc` |
| Auto-update | Signed DMG | Release page | Release page |

## Keyboard shortcuts

Bindings are written once for macOS and translated on Windows and Linux (`crates/agentty-app/src/keymap.rs`), so
Ctrl+letter always reaches the shell:

| macOS | Windows / Linux |
|---|---|
| ⌘X | Ctrl+Shift+X |
| ⇧⌘X | Ctrl+Alt+Shift+X |
| ⌥⌘X | Ctrl+Alt+X |
| ⌘1 … ⌘9 | Alt+1 … Alt+9 |
| ⌘= / ⌘- / ⌘0 | Ctrl+= / Ctrl+- / Ctrl+0 |
| ⌘-click (links) | Ctrl-click |

Text fields use plain Ctrl+C / V / X / A. Shortcut labels in the UI are shown in the platform's form.

## How it is organized

- `crates/agentty-app/src/platform/` holds what never depended on AppKit (tray state, drop queue, URL input, window
  frames) and helpers that differ per OS (opening folders, OS version, UI language).
- The macOS modules `native`, `status_item`, `webview`, `file_drop` and `notifications` are compiled only on macOS;
  Windows and Linux compile the same module names from `platform/fallback/` with the same API, so call sites have no
  `cfg`.
- `ipc.rs` is the socket transport; `launch.rs` builds the pane command line per platform; `agentty_bridge::process`
  starts background programs without console windows and finds programs on `PATH` (with `PATHEXT` on Windows).

## Windows

### Install

The release zip contains `agentty.exe`, `install.cmd` and `install.ps1`. Double-clicking `install.cmd` installs for the
current user (no administrator rights) into `%LOCALAPPDATA%\Programs\Agentty` with a Start menu entry, `agentty://`
links, "Open in Agentty" on folders, `agentty` on the user `PATH` and an entry in Settings → Apps that uninstalls it.
`install.ps1 -DesktopShortcut` adds a desktop shortcut; `-NoLaunch` skips starting it. Settings in
`%USERPROFILE%\.agentty` are kept on uninstall.

### Tools (Settings → System check)

Agentty checks what it uses on every start. On first launch, when something important (required or recommended) is
missing, it opens **Settings → System check**, which installs each tool in a new terminal tab with one click (or
copies the command). Until everything important is there, the start page shows a bar that leads to that page, and the
first-run tour waits: it opens once a check finds the environment ready.

| Tool | Need | Why | Install |
|---|---|---|---|
| Windows 10 1809+ | required | ConPTY terminals | Windows Update |
| Git for Windows | required | Claude Code runs hooks with Git Bash (pane status); the Git page | `winget install Git.Git` |
| Claude Code | recommended | Claude Code tabs | `irm https://claude.ai/install.ps1 \| iex` |
| Node.js LTS | recommended | npm-based agent CLIs, Node.js plugins | `winget install OpenJS.NodeJS.LTS` |
| PowerShell 7 | recommended | faster panes, correct argument passing | `winget install Microsoft.PowerShell` |
| Codex CLI | optional | Codex tabs | `npm install -g @openai/codex` |
| WinGet | recommended | the Install buttons | App Installer (Microsoft Store) |

Tools installed while Agentty runs are found without a restart: panes and lookups use this process's `PATH` plus the
user and machine `PATH` from the registry (read as stored and expanded, `%USERPROFILE%` and all), then the folders
installers use whether or not they touched `PATH` (`%USERPROFILE%\.local\bin` for Claude Code, `%APPDATA%\npm`,
`Program Files\nodejs`, `PowerShell\7`, `Git\cmd`). Agentty sets `CLAUDE_CODE_GIT_BASH_PATH` to Git for Windows'
`bash.exe` in panes (never WSL's) unless you set it yourself.

Linux gets the same page with `apt` / `dnf` / `pacman` / `zypper` commands (Claude Code, Node.js, Codex, Git,
`secret-tool`, `xdg-utils`, `notify-send`, `lsof`), and macOS one for Claude Code, Node.js (Homebrew), Codex and Git
(Command Line Tools). There a check asks a fresh login shell for its `PATH` (what `.zshrc` / `.profile` added since
Agentty started) and also looks in `~/.local/bin`, `~/.claude/local`, `~/.npm-global/bin`, `~/.volta/bin`,
`~/.bun/bin`, `/opt/homebrew/bin` and `/usr/local/bin`.

### Single instance

A second launch, an `agentty://` link or "Open in Agentty" hands its links and folders to the running Agentty (over
its authenticated socket, recorded in `~/.agentty/instance.json`) and exits, so two processes never share the settings
and workspace files. macOS does this through the system already.

### Notes

- Agent panes run a PowerShell script passed as `-EncodedCommand`: credentials, the agent (looked up as an application,
  so npm's `.ps1` shims and execution policies don't matter), cleanup, then `-NoExit` keeps the shell. Claude Code's
  `--settings` / `--mcp-config` JSON is passed as files under `~/.agentty/launch/`.
- Reserved words are defined inline in each pane's command, so Windows PowerShell's default `Restricted` execution
  policy doesn't block them (a dot-sourced script would be).
- `agentty.exe` carries its icon and version information (`crates/agentty-app/build.rs`, `packaging/windows/agentty.ico`).
- Building on Windows needs Visual Studio Build Tools with the Windows SDK (C++ workload) and Rust from `rustup`.
- Release builds are GUI-subsystem programs; `agentty notify` / `agentty browser` attach to the terminal they were
  started from.
