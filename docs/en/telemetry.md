---
title: Usage statistics
description: Exactly what Agentty sends about itself, and how to turn it off.
---

Release builds send a small set of anonymous usage events to Google Analytics, so we can see roughly how many people use Agentty and which features are worth continuing to build.

Nothing about your work is included. No paths, file names, commands, prompts, AI output, conversation content, repository or branch names, environment values or credentials — the application checks every event against an allow-list before sending, and discards whatever is not on it.

## The events

| Event | Property |
|---|---|
| `app_launched` | How many windows opened at launch |
| `window_opened` | — |
| `pane_opened` | Which kind of pane: `shell`, `claude`, `codex`, `gemini`, … |
| `agent_turn_finished` | Which kind of agent finished a turn |
| `feature_used` | Which area was used: `agentgit`, `flow`, `usage`, `settings`, `extensions`, `mini`, `browser_api` |

Every event also carries the application version, the operating system and its version, the language Agentty is shown in, the country of your system's region setting, and a random installation identifier stored in `~/.agentty/install_id`. The country is read from that setting alone — no IP address and no location of any other kind is used, and the setting is read fresh at each upload rather than stored. That identifier is generated from random bytes on first use, is not derived from anything about you, and is reset by deleting the file.

Events are queued locally and sent at most once a minute.

## Turning it off

**Settings → General → Share anonymous usage statistics.** It is on by default; switching it off stops collection immediately and discards anything still queued.

Setting the environment variable **`DO_NOT_TRACK=1`** turns it off regardless of the setting.

Builds made outside the official release pipeline — including any you compile yourself — have no analytics credentials and send nothing under any circumstances.

## What is never sent this way

Your conversations, code and files never leave your computer through Agentty at all. That is a property of the application, not of this setting. The agent CLIs you run talk to their own providers directly, under those providers' terms.

The full picture is in the [Privacy Policy](https://www.agentty.run/privacy-policy).
