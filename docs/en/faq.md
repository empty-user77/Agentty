---
title: FAQ
description: Short answers to the questions that come up while using Agentty.
---

## Is Agentty free?

Yes. No account, no subscription, no paid tier. You pay your AI providers directly for what your agents use; Agentty is not involved in that.

## How is this different from tmux?

tmux multiplexes terminals. Agentty is built around the agents running in them: it knows which one is working, which one is waiting for you, what each changed, and what it cost. You still get tabs, splits and a real terminal — plus everything that only makes sense when a program in the pane is an AI agent.

## Does Agentty upload my conversations?

No. Session lists, search, usage numbers and handoffs are all built from files already on your computer. The agent CLIs you run talk to their own providers directly — Claude Code to Anthropic, Codex to OpenAI — exactly as they would in any other terminal, under those providers' terms.

What Agentty itself sends is listed in [Usage statistics](/docs/telemetry). The full picture is in the [Privacy Policy](https://www.agentty.run/privacy-policy).

## An agent isn't in the + menu

Agentty offers only the CLIs it can find on the `PATH` of your **login shell**. If it works in your terminal but not here, the `PATH` is probably set in a file only interactive shells read. **Settings → System check** lists what is missing and can install it.

## How do I reopen an old conversation?

**⇧⌘S** lists every Claude Code and Codex session on your machine, and **⇧⌘O** searches them by title, content or path. Sessions are the agents' own files — Agentty just reads them. See [Sessions](/docs/sessions).

## Branches and worktrees are piling up

Each parallel session gets its own worktree on an `agentty/<name>` branch. The [files panel](/docs/agent-git) (**⌥⌘B**) lists every working tree; right-click one to remove it, and its branch once merged.

If you'd rather work in one folder, turn off **Settings → General → own worktree per session**.

## The usage numbers don't match my bill

They are estimates computed from local transcript files, and only Claude prices are built in. Add other models' prices to `~/.agentty/pricing.json` — see the [configuration reference](/docs/configuration-reference). Your provider's invoice is the real number.

## The agent keeps asking for permission

That is the agent's own permission system, not Agentty's. Configure it in the agent — for Claude Code, its settings or `/permissions`. Agentty passes your choice through and does not loosen it for you.

## A plugin won't load

Bundle everything it needs, including the SDK and any `node_modules` — Agentty does not run `npm install`. The folder name must match the `id` in the manifest. **Logs** on the plugin card shows the actual error. See [Publishing a plugin](/docs/plugin-publishing).

## Is there a Windows or Linux build?

Yes. Every release has a Windows installer and Linux `.deb` / `.rpm` packages next to the macOS download. See [Installation](/docs/installation) and [Platforms](/docs/platforms).

## Is the source available?

Yes. Agentty is open source under the Apache License 2.0, at [github.com/empty-user77/Agentty](https://github.com/empty-user77/Agentty).

## Something else is broken

[Troubleshooting](/docs/troubleshooting) covers the common cases. Anything else can go to the [issue tracker](https://github.com/empty-user77/Agentty/issues); please report security problems privately rather than in a public issue.
