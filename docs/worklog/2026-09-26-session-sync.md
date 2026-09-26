# 2026-09-26 — session sync (design)

> **Design written before the work.** Decisions agreed with the owner on 2026-09-26; it describes the plan, and the
> built feature may differ. For how Agentty works today, see the [documentation](../../README.md#documentation).
>
> **작업 전에 쓴 설계 문서입니다.** 2026-09-26 합의 내용이며 실제 구현과 다를 수 있습니다.

Branch `feat/session-sync` (from `main` at `f95afbf`).

## Goal

Workspaces and their agent sessions follow the user between computers through a private git repository they own
(GitHub via `gh`, or any remote over SSH). Work on one computer, sync, pick the session up on another, sync again.
Reference: cosmica-desktop's GitHub sync (`lib/web/sync/`), whose connect / pull-rebase / push / backoff loop this
reuses.

## Decisions (agreed with the owner)

| # | Decision |
|---|---|
| D1 | One sync repository per user. Layout `workspace/<workspace-id>/…` and `plugin/<plugin-id>/<workspace-id>/…`. |
| D2 | **Every device writes only its own files.** Sessions are stored per device; another device's files are read, never rewritten, so a push never conflicts on content. |
| D3 | Continuing another device's session creates a **branch** of it in this device's folder (a new session id), recording where it came from (device, session, message uuid). Sessions form a tree, like git branches. |
| D4 | No file or session merge. **Context merge**: compress another session's conversation and put it into the chosen terminal's input as a prompt; the user reviews and sends it. |
| D5 | A workspace is synced only while none of its agents is working. |
| D6 | Sync runs when a session finishes a turn (debounced), every 5 minutes when idle, and on demand. Never continuously. |
| D7 | Closing a tab or deleting a workspace records its final state in the metadata. Deleting a workspace that has sync data asks **"Also delete sync data"** (off by default); git history still has it. |
| D8 | Sessions missing locally can be downloaded and resumed from **"Sync sessions of this workspace"**; sync continues from there. |
| D9 | Sync icon left of the notifications icon in the status bar, with cosmica-style states; clicking it shows the sync sessions of the workspace in view, marking which terminal continues which session. |
| D10 | **No encryption.** The repository must be **private**: checked when connecting, before every push and on every periodic check; a repository found public stops sync and warns. Credentials in transcripts are masked before they are written. (Encryption with a password and a recovery key was designed first and dropped by the owner on 2026-09-26: simpler, and the files stay readable on GitHub.) |

## Repository layout

```
.agentty-sync.json                         marker: format version, created_at, created_by (device id)
devices/<device-id>.json                   name, OS, app version, last_sync_at — written by that device only
workspace/<ws-id>/sync_metadata.json       per-device sections (see below)
workspace/<ws-id>/devices/<device-id>/<agent>/<session-id>/
    session.json                           title, agent, cwd hint, git remote/branch/commit, parent link
    0001.jsonl, 0002.jsonl, …              append-only chunks of the transcript (plain text)
    sidecar/…                              subagent transcripts, tool results (chunked the same way)
plugin/<plugin-id>/<ws-id>/…               same shape as workspace/
.gitattributes                             "* -text" (no line-ending conversion)
```

- **Chunks, not whole files.** A transcript only grows; each sync uploads the bytes added since the last one as a
  new chunk, cut at the last complete line. Chunks never change, so history grows linearly and never conflicts.
  A branch stores only what was added after its fork point and references the parent's chunks.
- **`sync_metadata.json`** holds one section per device (`devices.<device-id>`: last sync, workspace name / group /
  color / layout snapshot, open sessions, tombstones). A device rewrites only its own section; on a rebase conflict
  in this file the device re-reads the remote version, replaces its own section and retries. The merged view
  (latest layout wins by timestamp, sessions are the union) is computed on read, so any computer rebuilds the same
  state from the repository alone.
- Device id: random UUID in `data_dir()/sync/device.json`; the name defaults to the computer name and can be edited.

## Leak prevention

Files are plain text, readable on GitHub. What protects them:

- The repository must be **private**: checked when connecting, before every push and on every periodic check
  (`gh repo view --json visibility`, or an anonymous read that must fail for other remotes). A repository found
  public stops sync: the status icon turns red and says to make it private again or disconnect. What was pushed
  before is exposed from the moment it turned public; the check cannot undo that, which is why masking matters.
- Transcript text is passed through the `check-secrets` patterns and matches are replaced by `***` before it is
  written to the clone.
- Local clone at `data_dir()/sync/repo`, folders `0700`, files `0600`.
- Plugin storage (`storage.json`, uploads) is **not** synced; only a plugin workspace's transcripts are.

## When a workspace syncs

- **Idle rule (D5):** every agent pane of the workspace is in a state other than `Working` (`Stop`, `Notification`,
  `Permission`, `SessionEnd`, or no pane). Panes without hook signals count as idle when their transcript has not
  changed for 30 s.
- **Triggers (D6):**
  - `Stop` of any pane → debounce 20 s → sync that workspace;
  - every 5 min with no user input and no agent activity → sync all idle workspaces;
  - the manual "Sync now" button → sync all idle workspaces, full check.
- A busy workspace is skipped and marked pending; it syncs on its next trigger.
- One sync at a time (mutex in the app plus a lock file in the clone for a second app instance). Failures retry with
  backoff (10 s doubling, capped at 30 min).
- Order: `pull --rebase` → write chunks and this device's metadata section → commit → push; on rejection repeat
  pull → push; on a rebase that fails, reset to the remote and rewrite this device's files (they are only ours).

## Continue, branch, context merge

- **Sync sessions of this workspace** (the sync popover): sessions grouped by device, newest first, as a tree when
  branched. The terminal in view that continues one of them is marked.
- **Continue** a session:
  - of this device → resume as today;
  - of another device, or deleted locally → download its chunks (and parents), rebuild the transcript under the
    local project folder, resume with a new session id in this device's folder, linked to the source (D3).
- **Latest by default:** the newest branch is suggested, so switching between two computers feels linear.
- **Before resuming** a session that has a newer continuation elsewhere, the popover says so and offers the newer
  one.
- **Path mapping:** each session records its git remote URL, branch, commit and original cwd. On another computer
  the folder is matched by remote URL; if none matches, the user picks a folder or clones. `cwd` inside the rebuilt
  transcript is rewritten to the local path. If the local checkout lacks the recorded commit, a warning says so.
- **Context merge:** pick a session → a summary is produced by the user's agent CLI (`claude -p`, or a plain
  extract of requests, replies and changed files for short sessions) → masked → placed into the chosen terminal's
  input, not sent.
- **Agents:** Claude Code first (transcript tree with `uuid` / `parentUuid`). Codex and Gemini are checked before
  they are supported; until then their sessions sync as files and resume only on the same session id.

## Close and delete (D7)

- **Close tab:** this device's metadata section records the final state (sessions, last message uuid, closed_at).
- **Delete workspace:** a confirm dialog with **"Also delete sync data"**, unchecked by default:
  - off → tombstone in this device's section; the data stays.
  - on → the workspace folder is removed from the repository (git history keeps it).
  - Other devices keep their local workspace and show it as "Sync removed".

## UI

- **Title bar:** a sync icon left of the notifications icon (`workbench/chrome.rs`, beside `render_notices_button`).
  States: off / synced / n pending / syncing / stopped because the repository is public / error. Hover shows the
  last sync time.
- **Popover:** repository, Sync now, devices with their last sync, the workspace's sync sessions (above), recent
  activity, errors.
- **Settings → Sync:**
  - connect: `gh` account repositories or an SSH URL; create a private repository with `gh repo create --private`;
  - device name, pause, disconnect.
- All strings in `i18n.rs` (en / ko / ja / zh).

## Phases

1. Connect (gh / SSH), private check, masking, device and workspace metadata, chunked upload, idle rule, triggers,
   status icon.
2. Sync sessions popover, download and resume, path mapping, close / delete records.
3. Branch tree view, newer-elsewhere warning, context merge.
4. Codex / Gemini support after checking their formats.

## Progress

| Phase | State |
|---|---|
| 1 | Built. Bridge: `agentty-bridge/src/sync/` (`repo.rs` git and gh, `session.rs` chunks, `mask.rs`, `model.rs`, 21 tests including two devices on a bare repository). App: `workbench/sync.rs` (triggers, title bar icon and popover, Settings → Sync), delete dialog in `confirm.rs`. Seen in a dev build with two data folders on one bare repository: connect, first push, masking (`ghp_***`), the second computer joining the same workspace, the popover listing both computers' sessions, removal recorded as `deleted`, "Also delete sync data" removing the folder. Not seen: a real GitHub repository through `gh`, a repository turned public. |
| 1+ | Both programs: a GitHub repository goes through `gh` (HTTPS with its login) or `git` (SSH keys), switchable in Settings → Sync; picked from the gh list it follows gh's own `git_protocol`. Any other URL is plain `git`, used as typed (an SSH GitHub URL is no longer turned into gh). |
| 2 | Built. `sync/restore.rs`: a synced session is rebuilt under the agent's folder for this computer's project folder (`cwd` lines rewritten); a local copy is left alone when it has everything, brought up to date when the synced one continues it, and a Claude Code session that went on separately on both sides comes back as a new session id (a branch). The continuing computer records where the session came from (`origins` → `parent` in its section). App: "Continue" in the sync popover (new tab in the workspace in front), Session history → "Synced" filter listing sessions this computer lacks (looked up in the agents' folders); continuing one without a local workspace creates one that syncs into the same repository workspace. Seen in a dev build with two data folders and two homes: continue from the popover, the Synced list after a local delete, a new workspace joining the original sync workspace with the parent recorded. Not seen: resuming inside a signed-in Claude Code (the test homes had no login). |
| 3 | Built. **Tree:** the popover lists a workspace's sessions as branches under the session each came from (`model::tree_order`, newest subtree first). **Newer elsewhere:** a session running here that another computer took further shows a warning with "Get latest" in the popover, and resuming it from Session history asks which copy to continue; a session open in a pane is never rewritten under it — its newer copy comes back as a branch (`keep_local`). **Context merge:** "Context" writes the synced session as a handoff document (the flow feature's `handoff::create_share`) and types the prompt pointing at it into the agent in front, without sending it. Seen in a dev build: tree with a branch under its origin, the warning after the other computer synced a newer turn, Get latest giving a branch while the original stayed as it was, the context prompt typed in with the toast, the handoff document masked. Not built: an agent-written summary instead of the full document. |
| 4 | Built. **Which agents travel as files** (`sync::syncs_files`): Claude Code, Codex, Gemini CLI, Kimi CLI — one text file that only grows. Antigravity keeps SQLite databases (chunks would break them) and Amp keeps its threads on its own servers: listed, never uploaded; continuing an Amp thread resumes it directly, an Antigravity one says it is not synced. **Codex branches** like Claude Code: a new id in the rollout's name and its `session_meta` line, next to the local rollout. **Gemini / Kimi:** their panes run as commands, so the session is the newest one in the pane's folder since it started (`gemini::find_recent`, `kimi::find_recent`, resolved during the sync); restored only into the same folder and only one the agent already registered (Gemini's `projects.json` slug, Kimi's `kimi.json` work dir) — their registries are never written; two copies that went on separately are refused rather than branched. Context merge reads all four. Seen in a dev build: a Codex session uploaded from one computer and restored under the other's `~/.codex/sessions/…` with the tab opening `codex resume` in the mapped folder. Not verified: Codex 0.155 also indexes threads in `state_5.sqlite`; whether `codex resume <id>` finds a rollout that is only a file could not be checked without a signed-in Codex (the test home showed the login screen). Gemini and Kimi are not installed here: covered by unit tests only. |
| GitHub | Seen against a real private repository (the owner's test repository, 2026-09-26): computer A connected through `gh` (HTTPS) — the empty repository set up on `main`, commits authored `Agentty (<device>) <sync@agentty.invalid>`; computer B connected with plain `git` over an SSH URL and joined A's workspace; B continued A's Codex session and the metadata on GitHub records where it came from; A switched to `git` (SSH) and back to `gh`, pushing a new chunk each way (origin URL followed); connecting to a public repository (`empty-user77/Agentty`) was refused before anything was cloned. Not seen: a repository turned public while connected (not done to a real repository on purpose). Test note: `gh` finds its login through the keychain of `$HOME`, so the test apps (fake homes) got it as `GH_TOKEN`. |

**Decided while building:** a workspace synced for the first time joins the one another computer already synced
for the same project (git remote and folder in it; the name when there is no remote), unless this computer
already syncs that one under another of its workspaces. Ids are given out inside the sync pass, after the fetch,
so the match sees the newest repository.

## Open questions

- Codex and Gemini: transcript shape and whether a copied session resumes.
- Whether Claude Code's `--fork-session` covers branching from the latest message, so that rebuilt transcripts are
  only needed for earlier fork points.
- Summary cost for context merge: default to the plain extract, with the agent summary as an option?
