---
title: Settings
description: What each page of Settings (⌘,) does.
---

**⌘,** opens Settings. Changes apply immediately and are saved to `~/.agentty/settings.json`.

## General

Language (English, 한국어, 日本語, 中文), and the behavior of the things you meet daily:

| Setting | What it does |
|---|---|
| Build my idea | Offers the idea page on the start screen and in the + menu |
| Agent status bar | The bar above AI panes, and whether it sits above or below the terminal |
| Confirm close | Ask before closing a pane, tab or workspace that was used |
| Own worktree per session | A second session in a busy project gets its own git worktree |
| Search box above the workspace list | Searches workspace names and the conversations held in them |
| Move a finished workspace to the top | Off keeps the order you arranged yourself |
| Parallel tasks | Agents may ask to start tasks in split panes; you confirm each request |
| Tell agents what Agentty offers | Agents started here get a short guide to Agentty's commands, passed on the command line |
| Stop servers on close | Closing a pane stops the local servers started in it |
| Share anonymous usage statistics | See [Telemetry](/docs/telemetry) |
| Menu bar icon | Keeps Agentty running after you close the window (macOS) |
| Prevent sleep | Keeps the machine awake so a long unattended run is not cut short — always, or for a set 1 to 72 hours. Uses more power |

## Project

Where a new tab starts, the harness patterns Agentty looks for, whether the harness prompt is sent right away, and which agent starts harness work. Also which editor **Open in editor** uses.

## Windows

Every Agentty window (**⇧⌘N** opens another) keeps a workspace list of its own. This page lists the windows, open and recently closed, with the workspaces in each and the terminals still running. Click a workspace to go to it. **Bring forward** shows an open window, **Reopen** opens a closed one again, and **Delete** closes a window and removes its workspaces after asking; the conversations stay in the session list. The main window is never deleted.

## Accounts

How new Claude Code and Codex tabs sign in. The default changes nothing — the agents use their own login. See [MCP and connectors](/docs/mcp-and-connectors) for the alternatives and where the credentials are kept.

## Appearance

Theme, font, font size, line height, letter spacing, bold text, colours of your own for background, text, cursor and selection, cursor shape and blink, padding, scrollback. Also which items the agent status bar shows, and in what order. See [Themes and fonts](/docs/themes).

## Keyboard shortcuts

The full list, in your platform's notation. See [Keyboard shortcuts](/docs/keyboard-shortcuts).

## Notifications

System notifications, whether to notify while Agentty is in front, and chat notifications to Slack, Discord or Telegram. See [Agent status](/docs/agent-status).

## Sync

Workspaces and agent sessions follow you between computers through a private Git repository you own (GitHub through `gh`, or any SSH URL); only empty private repositories and ones already used for sync are offered. A workspace syncs when its agents finish a turn, when a tab closes, every five minutes while nothing happens, and on **Sync now**; a sync that fails is tried again by itself.

Turn on **Sync settings** (and press **Save**) to keep your settings, custom commands, connectors, imported themes and plugins' settings in step too: the newest change wins, and a computer that joins takes the settings already there. Secrets (API keys, tokens, passwords) are never synced — enter them once on each computer. A part of the settings that looks like it holds one (a token typed into a custom command, say) stays on its computer, is not replaced by another computer's, and the sync popover says which.

**Remove old sessions** keeps the repository from growing without end (GitHub's private repositories are not unlimited): sessions unchanged for 30, 90, 180 or 365 days (or a number you choose) are no longer synced, and the sync popover asks before deleting them from the repository. The period is the repository's, so every computer uses the same one.

Each computer is told apart by an id Agentty keeps in `~/.agentty/computer-id` (nothing about the hardware is read); deleting `~/.agentty` makes it show up as a new computer. Deleting a workspace with **Also delete sync data** removes it for every computer. The repository keeps every version, so it also works as a backup. The repository must stay private: sync stops while it is public.

## Backup

**Export configuration** saves your whole setup to one `.agenttyconfig` file: settings, custom commands, connectors, database connections, how agents sign in, imported themes, the list of installed plugins and their settings. Panel sizes, recent folders and other things that belong to one computer stay out. Turn on **Include secrets** to add the API keys, tokens and passwords from the credential store, and plugins' settings that hold one, encrypted with a password you choose (PBKDF2 + AES-256-GCM); without it those plugin settings stay out; Agentty does not keep that password.

**Import configuration** puts a file's configuration in place of this computer's: settings from the file win, while this computer's panel sizes and recent folders stay; commands, connectors and the other files are replaced as a whole. Leave the password empty to import everything except the secrets. Missing plugins can be installed from the marketplace or their Git repository. The configuration from before each import is kept in `~/.agentty/backups/`.

## Browser

Whether links open in the in-app browser or your default one, whether dev servers open by themselves, and the search engine.

## System check

What Agentty uses and whether it is installed — Git, Node.js, the agent CLIs. Each missing tool can be installed from here in a new terminal tab.

## About

Version, update check, and links — including the [Privacy Policy](https://www.agentty.run/privacy-policy), [Terms of Service](https://www.agentty.run/terms-of-service) and [License Agreement](https://www.agentty.run/eula).
