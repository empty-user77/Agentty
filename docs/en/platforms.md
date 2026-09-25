---
title: Platforms
description: What differs between macOS, Windows and Linux.
---

Agentty is developed on macOS, which remains the reference platform. All three have a published download, and Windows and Linux share every feature that does not depend on AppKit or WebKit.

## What differs

| | macOS | Windows | Linux |
|---|---|---|---|
| Pane shell | `$SHELL` | PowerShell (`pwsh` when installed) | `$SHELL` |
| Credential store | Keychain | Credential Manager | Secret Service, else a private file |
| Notifications | System notifications, click opens the pane | Toast | `notify-send` |
| In-app browser | Inside Agentty | Default browser | Default browser |
| Menu bar icon, mini mode | ✓ | — | — |
| Title bar | Agentty's own | System | System, or Agentty's where the compositor has no decorations |
| Install | DMG | Setup program (per user) | `.deb` / `.rpm` |
| Auto-update | Installs and relaunches | Installs and relaunches | Links to the new packages |

Shortcuts are translated: ⌘ → Ctrl+Shift, ⇧⌘ → Ctrl+Alt+Shift, ⌥⌘ → Ctrl+Alt, ⌘1…9 → Alt+1…9. See [Keyboard shortcuts](/docs/keyboard-shortcuts).

## Windows

Requires Windows 10 version 1809 or later, x64. The setup program installs for the current user without administrator rights, and adds a Start menu entry, `agentty://` links, "Open in Agentty" on folders, and `agentty` on your `PATH`. It is not code-signed yet, so SmartScreen may warn on first run — see [Installation](/docs/installation).

Claude Code runs its hooks through Git Bash, so **Git for Windows** is required for pane status. **Settings → System check** lists what is missing and installs each tool in a new tab.

## Linux

Both X11 and Wayland. The packages install `/usr/bin/agentty`, a desktop entry (application menu and `agentty://` links) and the icon, and depend on the libraries GPUI needs. A Vulkan driver is recommended.

Updates are announced in the app; the package manager installs them.

## One instance

A second launch, an `agentty://` link or "Open in Agentty" hands its work to the running Agentty and exits, so two processes never share the settings and workspace files.