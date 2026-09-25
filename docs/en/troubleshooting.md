---
title: Troubleshooting
description: The things that usually go wrong, and what to do about them.
---

## An agent isn't offered, or won't start

Agentty only offers agent CLIs it can find on the `PATH` of your **login shell**. If a tool works in your terminal but not here, the `PATH` is probably set in a file only interactive shells read (`.zshrc`, `.bashrc`). Move the export to where a login shell reads it, or check **Settings → System check**, which lists what is missing and installs it for you.

## Pane status stays blank

Status comes from a hook Agentty passes the agent when the pane starts.

- On Windows, Claude Code runs hooks through Git Bash — install **Git for Windows**.
- If you launched the agent yourself inside an existing shell, it has no hook. Open it from the **+** menu instead.

## An agent edited the wrong files

Two sessions in one folder will collide. Turn on **Settings → General → own worktree per session**, which gives a second session its own git worktree and branch. The [files panel](/docs/agent-git) shows every working tree and what changed in it.

## A dev server keeps running after I closed the tab

**Settings → General → stop servers on close** ends the servers a pane started when you close it. Monitoring → **Processes** lists what is still running.

## Usage numbers look wrong

They are computed from the transcript files on your disk, and only Claude model prices are built in. For other models, add prices to `~/.agentty/pricing.json` — see the [configuration reference](/docs/configuration-reference). The numbers are estimates; your provider's invoice is authoritative.

## A plugin doesn't appear or stops

- JavaScript plugins need **Node.js 18+** on your login shell's `PATH`.
- The folder name must match the `id` in `agentty-plugin.json`. Press **Refresh** after copying one in.
- **Logs** on the plugin card shows what it printed, including crashes. After fixing code, press **Restart**.

More in [Using plugins](/docs/plugins-overview).

## An `agentty://` link does nothing

Links work with the installed application, not with a build started from a development shell. Check that the plugin the link names is installed and enabled.

## The app won't update

Updates are downloaded from GitHub and verified before use. If a download fails — a proxy, a blocked host — get the installer by hand from the [releases page](https://github.com/empty-user77/Agentty/releases).

## Still stuck

Report it on the [issue tracker](https://github.com/empty-user77/Agentty/issues). Include your Agentty version (**Settings → About**), your operating system, and what you expected. Redact any credentials before pasting logs or screenshots.

Security problems should be reported privately rather than in a public issue.
