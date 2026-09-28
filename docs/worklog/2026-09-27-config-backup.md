# 2026-09-27 — configuration export / import and settings sync

> **Historical work log.** Written while the work was done (September 2026); it describes the code as it was then.
> For how Agentty works today, see the [documentation](../../README.md#documentation).
>
> **작업 당시의 기록입니다.** 현재 앱과 다를 수 있습니다.

Branch `feat/config-backup`, from `feat/session-sync` (PR #86, not merged yet): settings sync reuses its repository.
Merge after #86; the pull request's base is `feat/session-sync`.

## Decisions (agreed with the owner)

| # | Decision |
|---|---|
| D1 | Settings sync goes through the session sync repository (`settings/<device id>.json`), not a separate folder. |
| D2 | Manual export can include secrets, **encrypted with a password** (PBKDF2-HMAC-SHA256 600k + AES-256-GCM, `ring`). Sync never carries secrets. |

## What was built

- **Bridge `backup/`** — `collect(scope, password)`, `write_file` / `read_file`, `import(bundle, options)`,
  `import_synced`, `merge_settings`, `fingerprint`. `crypto.rs` seals the secrets.
  - Travels: `settings.json` minus `DEVICE_SETTINGS` (panel sizes, recent folders, timers, first-run state, update
    postponement), `commands.json`, `connectors.json`, `db-connections.json` (project folders as `~/…`),
    `agent-auth.json`, `themes/*.itermcolors`, the plugin list (reinstalled from the marketplace / Git / built-in;
    folder plugins are skipped).
  - Sync scope: settings, commands, connectors, themes only (agent sign-in and database connections need secrets or
    folders of one computer).
  - Secrets: connector, database, agent sign-in and chat-notification slots, read from the credential store by the
    ids in the files. An import only writes those four services.
  - Import writes a copy of the configuration before it to `~/.agentty/backups/before-import-<time>.agenttyconfig`
    (with the secrets sealed by the same password when one was given). Settings are merged by the app on the main
    thread (`settings::replace_settings`), not written under it.
- **Bridge `sync/settings.rs`** — each computer writes its own file; the newest `changedAt` wins; a computer joining
  takes what is there (its first change time is 0); after applying, `applied()` records the fingerprint of what is on
  disk with the other computer's change time, so nothing is sent back. Sync is held while settings are applied.
- **App** — Settings → Backup (`workbench/backup.rs`), "Sync settings" switch in Settings → Sync, toast when another
  computer's settings are applied. `switch()` split out of `toggle()`. Debug: `backup export|pick|import|plugins`,
  `sync settings on|off`, `settings-section backup`.

## Seen in a dev build (two scratch data folders)

- Export with a password: file `0600`, device keys absent, secrets sealed.
- Import with a wrong password: refused, nothing written. With the right one: theme and font size applied live, this
  computer's sidebar width and update postponement kept, copy written to `backups/`.
- Sync: A connected to a local bare repository and pushed `settings/<A>.json` (no secrets, no device keys); B joined
  with fresh settings, took A's (theme file included); further syncs on both committed nothing new.

## Not seen

- Secrets in the credential store going through a real export → import (covered by unit tests with a fake reader; the
  scratch apps had none stored, and writing test items into the login Keychain would prompt).
- A change made on B reaching A in the app (covered by `sync::settings` tests).
- Plugin reinstall on import (network).
- Windows / Linux.
