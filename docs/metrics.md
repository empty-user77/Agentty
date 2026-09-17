# Usage metrics

Agentty can record anonymous usage statistics to help decide what to improve. It is **off by
default** and nothing is collected until you turn it on in **Settings → General → Privacy**.

## Rules

- **Opt-in.** Disabled unless you enable it.
- **`DO_NOT_TRACK=1`** (also `true` / `yes`) in Agentty's environment disables it regardless of the setting.
- **No content.** Never paths, file names, repository or branch names, commands, prompts, agent
  output, environment variables or credentials. Properties are checked against an allow-list and
  must be short identifiers (`claude`, `cosgit`), so free text cannot slip through.
- **Local first.** Events are appended to `~/.agentty/metrics/events-YYYY-MM.jsonl`, which you can
  open from the settings page. They are uploaded only if an endpoint is configured.
- **No keys in the repository.** There is no built-in collection service. An endpoint is set with
  `metrics.endpoint` in `~/.agentty/settings.json` or `AGENTTY_METRICS_ENDPOINT` (HTTPS, or
  `localhost` for testing). Pending events are posted as a JSON array every 15 minutes; failures
  are silent and retried later.
- **Resettable ID.** A random install ID (not derived from hardware or account) is created when
  the first event is recorded. "Reset anonymous ID" deletes it.

## Events (schema v1)

| Event | Properties |
|---|---|
| `app_launched` | `windows` (number) |
| `window_opened` | — |
| `pane_opened` | `tool`: `shell`, `claude`, `codex`, `gemini`, `agy`, … or `command` |
| `agent_turn_finished` | `tool` |
| `feature_used` | `feature`: `cosgit`, `flow`, `usage`, `settings`, `extensions`, `mini`, `browser_api` |

Every record also has `v` (schema version), `ts` (epoch ms), `install_id`, `app_version` and
`os` (macOS version).

## Implementation

`crates/agentty-bridge/src/metrics.rs` (allow-list, local log, upload) and
`crates/agentty-app/src/metrics.rs` (call sites respect the setting and `DO_NOT_TRACK`).
