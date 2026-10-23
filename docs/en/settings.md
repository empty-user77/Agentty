---
title: Settings
description: What each page of Settings (⌘,) does.
---

**⌘,** opens Settings. Changes apply immediately and are saved to `~/.agentty/settings.json`.

The sidebar groups the pages: **General**, **AI**, **Work**, **Connections & data** and **System**. The search box above it finds any setting by its name or its description, on every page — choose a result to go to that page with the setting marked.

## General

Language (English, 한국어, 日本語, 中文), **Appearance** (System, Dark or Light — System follows macOS as it switches; terminals keep the theme from Terminal Style either way), and the behavior of the things you meet daily:

| Setting | What it does |
|---|---|
| Build my idea | Offers the idea page on the start screen and in the + menu |
| Confirm close | Ask before closing a pane, tab or workspace that was used |
| Own worktree per session | A second session in a busy project gets its own git worktree |
| Search box above the workspace list | Searches workspace names and the conversations held in them |
| Move a finished workspace to the top | Off keeps the order you arranged yourself |
| Stop servers on close | Closing a pane stops the local servers started in it |
| Share anonymous usage statistics | See [Telemetry](/docs/telemetry) |
| Menu bar icon | Keeps Agentty running after you close the window (macOS) |
| Prevent sleep | **On** keeps the machine awake (always, or for a set 1 to 72 hours), **Agents** only while an agent is working, **Off** lets it sleep as usual. Also in the status bar: click the cup |

## AI settings

What Agentty adds to agent sessions:

| Setting | What it does |
|---|---|
| Claude advisor | The [advisor](/docs/advisor) new Claude tabs start with |
| Show the advisor in the status bar | Off by default. On, the status bar of a Claude tab shows its advisor and lets you change it for that tab |
| AI CLI status bar | The bar above AI panes, and whether it sits above or below the terminal |
| Tell agents what Agentty offers | Agents started here get a short guide to Agentty's commands, passed on the command line |
| Parallel tasks | Agents may ask to start tasks in split panes; you confirm each request |

## Agents

The agent CLIs on this computer. **Agent permissions** sets whether Claude Code and Codex tabs start in YOLO (Bypass) mode. **Installed** lists every agent found, with its version and command; turn one **Off** and it leaves the launcher, the start page and the command palette (it stays installed). **Refresh** looks again after you install one; **Available to install** lists the agents Agentty can start once their CLI is there: **Install** runs the vendor's own install command in a new terminal tab, and the link opens its official page (for an agent without a documented command, the page only).

Open **Claude Code** for its own features, saved in Claude Code's user settings (`~/.claude/settings.json`, or `$CLAUDE_CONFIG_DIR`): **Agent teams** (a lead session that starts and coordinates teammate sessions; experimental), **Always think** and **Automatic updates**. Only the key a switch stands for is written; the rest of the file stays as it is, and a file that is not valid JSON is left alone. New Claude Code sessions pick the change up. **Codex** links to its `~/.codex/config.toml`.

## Integrations

The outside services Agentty works with, one card each with whether it is connected; open a card to set it up.

- **GitHub** — pull requests, cloning and sync go through the GitHub CLI (`gh`) and its own sign-in; nothing is stored in Agentty. **Sign in in a terminal** runs `gh auth login` in a new tab.
- **Slack, Discord, Telegram** — where notifications are sent. A webhook URL or a bot token goes into the system credential store; **Send notifications here** turns one service on or off. What is sent is set in Notifications.
- **Jira** — the site, email and API token the development board imports tickets with.

## Project

Where a new tab starts, the harness patterns Agentty looks for, whether the harness prompt is sent right away, and which agent starts harness work. Also which editor **Open in editor** uses. The language servers of the file editor, with an install button for a missing one, are there too.

## Windows

Every Agentty window (**⇧⌘N** opens another) keeps a workspace list of its own. This page lists the windows, open and recently closed, with the workspaces in each and the terminals still running. Click a workspace to go to it. **Bring forward** shows an open window, **Reopen** opens a closed one again, and **Delete** closes a window and removes its workspaces after asking; the conversations stay in the session list. The main window is never deleted.

## Accounts

How new Claude Code and Codex tabs sign in. The default changes nothing — the agents use their own login. See [MCP and connectors](/docs/mcp-and-connectors) for the alternatives and where the credentials are kept.

## Appearance

Theme, font, font size, line height, letter spacing, bold text, colours of your own for background, text, cursor and selection, cursor shape and blink, padding, scrollback. Also which items the agent status bar shows, and in what order. See [Themes and fonts](/docs/themes).

## Keyboard shortcuts

The full list, in your platform's notation. See [Keyboard shortcuts](/docs/keyboard-shortcuts).

## Notifications

System notifications, whether to notify while Agentty is in front, and what goes to chat services: finished turns too, the request's details, sessions on screen. The services themselves — Slack, Discord, Telegram — are connected in **Integrations**. See [Agent status](/docs/agent-status).

## Sync

Workspaces and agent sessions follow you between computers through a private Git repository you own (GitHub through `gh`, or any SSH URL); only empty private repositories and ones already used for sync are offered. A workspace syncs when its agents finish a turn, when a tab closes, every five minutes while nothing happens, and on **Sync now**; a sync that fails is tried again by itself.

**This computer (SQLite)** needs no repository, account or install: everything goes into one file, `~/.agentty/sessions.sqlite` (through the SQLite that comes with macOS and Linux; on Windows it is built into Agentty). It is a backup of your sessions on this computer: when a session disappears from the agent's own folder (`~/.claude`, `~/.codex`), **Continue** in the sync popover brings it back from the file. Settings → Sync shows the file's size and **Show in folder**. To move the backup, copy the file into `~/.agentty` on the other computer and choose **This computer (SQLite)** there: its sessions come back. Disconnecting keeps the file. It is not shared with other computers by itself; use a Git repository for that. Both together: with a repository connected, turn on **Also keep a SQLite copy**, and after every sync what the repository holds (every computer's sessions) also goes into the same file, which keeps working as a backup without the repository.

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

**Build** lists what this copy of Agentty is: version, commit, build date, profile (release or debug), target platform, Rust version and whether on-device voice recognition is included, followed by the API versions of the interfaces Agentty offers (plugin API, marketplace index, MCP, shell integration, settings backup file, session sync format). **Copy build info** copies all of it as plain English text to paste into a bug report; it holds no paths, user names or keys.
