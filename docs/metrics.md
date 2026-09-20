# Usage analytics

Release builds send anonymous usage events to **Google Analytics 4** through the
[Measurement Protocol](https://developers.google.com/analytics/devguides/collection/protocol/ga4).

## Setup

1. In GA4, create a property with a **Web** data stream (any URL, e.g. `https://www.agentty.run`).
2. Stream details → copy the **Measurement ID** (`G-…`).
3. Stream details → **Measurement Protocol API secrets** → create a secret.
4. Put both in `.env.agentty-prod` (never committed):

   ```sh
   AGENTTY_GA_MEASUREMENT_ID="G-…"
   AGENTTY_GA_API_SECRET="…"
   ```

`scripts/build-dmg.sh prod` loads that file before `cargo build`, so the values are compiled into
the release binary. Builds without them (dev builds, CI) send nothing. To check events arrive,
use GA's **Realtime** report or **DebugView**.

## What is sent

| Event | Parameters |
|---|---|
| `app_launched` | `windows` |
| `window_opened` | — |
| `pane_opened` | `tool`: `shell`, `claude`, `codex`, `gemini`, `agy`, … or `command` |
| `agent_turn_finished` | `tool` |
| `feature_used` | `feature`: `agentgit`, `flow`, `usage`, `settings`, `extensions`, `mini`, `browser_api` |

Every event also carries `app_version`, `os_version` and a random install ID (`client_id`, stored
in `~/.agentty/install_id`). Parameters are checked against an allow-list and must be short
identifiers — paths, commands, prompts, output, repository and branch names are never sent.
Events are queued and sent once a minute.

## Turning it off

**Settings → General → Share anonymous usage statistics** (`analytics` in `settings.json`, on by default) is the
consent switch: turning it off stops queueing right away and drops whatever was queued before the next upload.
`DO_NOT_TRACK=1` in the environment turns it off regardless of the setting, and builds without the credentials above
send nothing either way.

## Implementation

`crates/agentty-bridge/src/metrics.rs` (allow-list, GA request) and
`crates/agentty-app/src/metrics.rs` (queue, consent check and call sites).
