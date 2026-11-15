---
title: Multiple accounts
description: Sign in to several Claude Code and Codex accounts and run each in a window of its own, at the same time.
---

You can stay signed in to more than one Claude Code or Codex account. Each account runs in a window of its own, with its own sessions, usage and plan limits, and accounts can run at the same time. Use it to keep a work account and a personal one apart, or to carry on with another account when one reaches its limit.

## Adding an account

Click the account icon above Settings to open the account menu. **Add account** signs in to another Claude Code or Codex account. Each account keeps its own login.

The login you already had in `~/.claude` and `~/.codex` is listed as the **base account**, and stays as it was.

## One window per account

Clicking an account in the account menu brings its window forward, or opens one.

| Each account has its own | Shared by every account |
|---|---|
| Window and the workspaces in it | Settings |
| Session history | Skills, rules and hooks |
| Usage and plan limits | MCP servers |
| Sync repository | |

## Choosing the account to start on

Starting an agent that has more than one account asks which account to use, every time. The window's own account is picked to begin with, and each account shows its plan and usage.

- Opening Claude Code or Codex in a new tab asks.
- Typing `claude` or `codex` into a terminal (a split pane included) asks the same and runs it on the account picked. **Esc** closes the question without starting it.
- With one account, nothing is asked.

## Panes on another account

A pane running on another account than its window's shows that account's initial in a blue circle at the start of its status bar, with a blue line under the bar. Click it for the account's plan, its 5-hour and weekly limits and when they reset, and what it used in the last 7 days.

With the status bar off, a pane shown in [chat view](/docs/chat-view) has the same circle under its composer.

## Plans and sign-in state

Plans show as Pro, Max or Ultra in the account menu, Settings, the Usage page and the menu bar. An account that is signed out, or whose login ends within two days, says so, with **Sign in again** next to it.

## Moving a workspace to another window

With more than one window open (another account's window included), a workspace or a whole group can move to another window:

- **Move to Window** in a workspace's menu
- **Move group to window** in a group's menu

Terminals keep running, and agents stay in their sessions on the account they started with, so a pane moved into another account's window gets the blue circle above.

Workspace options, the sidebar's width and the side panels' widths can differ per window. The main window sets them for every window; a window that changes one keeps its own value until **Use the main window's** puts it back.

## Settings → Accounts

Lists every login, grouped by agent. A login opens to show:

- its folder (**Open**, **Copy**)
- the environment variable that points the agent there
- where its sign-in is kept
- **Sign in again** and **Remove**

Removing an account signs it out and deletes its sessions from this computer; it is refused while a pane runs on that account. The base account can't be removed; **Sign in again** replaces it with another login.

## Sync

[Sync](/docs/settings#sync) is per account too. A window syncs for its Claude Code account, else its Codex account, into a repository of its own, and two accounts can't share one. Sessions of any other account in that window are not synced there.

## From your phone

The [remote access](/docs/remote-access) page shows each workspace's account, and a new workspace can choose among the open windows' accounts.
