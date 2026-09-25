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

## Accounts

How new Claude Code and Codex tabs sign in. The default changes nothing — the agents use their own login. See [MCP and connectors](/docs/mcp-and-connectors) for the alternatives and where the credentials are kept.

## Appearance

Theme, font, font size, line height, letter spacing, bold text, colours of your own for background, text, cursor and selection, cursor shape and blink, padding, scrollback. Also which items the agent status bar shows, and in what order. See [Themes and fonts](/docs/themes).

## Keyboard shortcuts

The full list, in your platform's notation. See [Keyboard shortcuts](/docs/keyboard-shortcuts).

## Notifications

System notifications, whether to notify while Agentty is in front, and chat notifications to Slack, Discord or Telegram. See [Agent status](/docs/agent-status).

## Browser

Whether links open in the in-app browser or your default one, whether dev servers open by themselves, and the search engine.

## System check

What Agentty uses and whether it is installed — Git, Node.js, the agent CLIs. Each missing tool can be installed from here in a new terminal tab.

## About

Version, update check, and links — including the [Privacy Policy](https://www.agentty.run/privacy-policy), [Terms of Service](https://www.agentty.run/terms-of-service) and [License Agreement](https://www.agentty.run/eula).
