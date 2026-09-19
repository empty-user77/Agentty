# 2026-09-18 — workspace tools (files panel, worktrees, onboarding, monitoring, HUD)

Branch `feat/workspace-tools` (from `main` at `a8c513a`). Eight requests from the owner, in the order they
are being built. Tick an item only when it is built **and** checked in the running app.

| # | Request | State |
|---|---|---|
| 8 | Context meter resets right after `/compact` | done — unit-tested against the real transcript shape (`claude_stats`); not watched live in a pane |
| 4a | "AI Usage" page is renamed "Monitoring" | done, seen (`page.monitoring`; tabs AI Usage / AI Processes / Proxy) |
| 7 | Closing a tab stops the local servers it started (setting, default on) | done, seen: a `nohup` server was stopped when its tab closed (`workbench/servers.rs`) |
| 6 | A local server that starts opens in the in-app browser when links open in-app | done, seen: `python3 -m http.server` opened by itself |
| 1 | Files panel docked right, toggle next to the split icons | done, seen (`workbench/files_panel.rs`, ⌥⌘B) |
| 2 | Sessions in the same project run in their own git worktree; the files panel shows which | done, seen: Codex got `agentty/codex-…` next to a Claude pane (`agentty-bridge/src/worktree.rs`, `workbench/worktrees.rs`) |
| 5 | Status bar (HUD) items can be reordered / hidden; required ones only reordered | done, seen: move + hide saved to `settings.hud` (`hud.rs`, `workbench/hud_settings.rs`); the bar itself not looked at with a custom order |
| 4b | Monitoring → "Proxy" tab: capture traffic of tabs, filter by endpoint | done, seen with real HTTPS and HTTP requests from a captured tab (`capture.rs`, `workbench/proxy_page.rs`) |
| 3 | First-run onboarding: basic settings, then follow-along feature tour | done, seen. Tour rebuilt twice on the owner's feedback ("Try it" was vague): numbered steps per feature, the real button gets a pulsing ring, the app notices each step, says what happened, "Next" is explicit. Command palette taken out of the tour at the owner's request (`workbench/onboarding.rs`) |

## Decisions

- **Context after `/compact` (8):** Claude Code writes `compact_boundary` with
  `compactMetadata.postTokens`; nothing with `usage` follows until the next prompt. `session_stats`
  now reads the boundary when it is newer than the last assistant usage.
- **Servers on close (7):** the process tree of the pane's shell is read (`ps`) *before* the pane is
  dropped (children are re-parented to pid 1 afterwards), listeners are looked up in the background
  and get `SIGTERM`, then `SIGKILL` after 3 s. Unix only (no `lsof` on Windows).
- **Auto-open (6):** the 5 s port poll notices a new port of a pane in the active workspace, probes
  `GET /` on loopback and opens it only if HTML comes back. Only when links open in-app.
- **Worktrees (2):** created under `data_dir()/worktrees/<repo>-<hash>/<name>` on branch
  `agentty/<name>`, only for a *new agent* session whose folder is a working tree another live agent
  pane already uses. Resumed sessions keep their folder. Setting `autoWorktree` (default on).
- **Proxy capture (4b):** a loopback forward proxy (`CONNECT` tunnels + plain HTTP). Tabs opened while
  capture is on get `HTTPS_PROXY` / `HTTP_PROXY` with a per-pane user name and a random token, so
  records name their pane and other local processes cannot use the proxy. HTTPS content stays
  encrypted: host, port, bytes and timing only. Records live in memory only. The listener stays up
  while any pane may still point at it; stopping capture only stops recording.
- **Onboarding (3):** the browser tour was already removed (2026-09-18, idea-launch). Onboarding is a
  modal for the settings steps and a floating coach card for the follow-along steps.

## Second pass (2026-09-19): whole-app check, all in the scratch debug app

| Checked | Result |
|---|---|
| First run on a fresh data folder | onboarding shows by itself; Skip is remembered; second launch shows nothing |
| Tour, every task, step by step (`tour do` / real clicks) | each step advances exactly one; ring on the right control; six tasks finish |
| Following the tour by hand | found two dialogs the texts did not mention (folder picker after the Terminal card, close confirmation after ✕): both are in the step texts now |
| Second agent in: a plain folder / a repository without commits | starts in place; the failed `git worktree add` falls back with a toast and leaves no folder behind |
| Project path with a space, launch from a subfolder | tree created; the session starts in the same subfolder of its tree |
| Shell in a taken project; `autoWorktree` off | stays in the project; no tree |
| Remove a tree from the panel | refused with a clear message while it has changes; removed with its branch once clean; a tree of another data folder is not offered |
| Claude Code and Codex through the capture proxy (no prompt sent) | both work: api.anthropic.com, mcp-proxy.anthropic.com, downloads.claude.ai, raw.githubusercontent.com; no errors |
| Eight parallel ~1 MB HTTPS downloads through the proxy | all complete, byte counts match |
| Capture stopped | the captured tab keeps its network, nothing new is recorded; a tab opened afterwards has no proxy variables |
| Proxy without / with a wrong token (live) | 407, CONNECT refused, nothing recorded |
| JSON API port vs HTML port | only the HTML one opens in the in-app browser; `autoOpenServers` off opens nothing |
| Closing a bystander tab / the owner tab | the server stays / is stopped with the toast; `stopServersOnClose` off leaves it running |
| Status bar layout | move + hide saved and shown in the preview (the live bar with a custom order still not looked at) |

Hardening from this pass: a file name starting with `-` is typed as `./-name`; a working tree counts
as Agentty's only if it exists and resolves into `data_dir()/worktrees` (no `..`, no symlink); the
proxy keeps the user's own `NO_PROXY` and leaves protocol upgrades alone; an unknown status bar item
never fails the settings file; toasts stay clear of the native browser view.

## Small requests that came in during the second pass

- Enlarge / close buttons of split panes are always visible (were hover-only).
- Icons next to small text in the status bar and pane headers sat ~2 pt above their labels
  (measured in a 2x snapshot: icon centre 41.5 px, text 45.5 px). `beside_text` lowers them 1.5 pt.
- Harness detection: `<project>/.claude/skills/harness/SKILL.md` and `…/harness-*/SKILL.md` are
  default patterns now (a real skill, exactly there; tests cover the near misses).
- The folder of the status bar / pane header is cut in the middle (`ui::middle_ellipsis`), shows the
  whole path on hover, and ⌘-click shows it in the file manager (checked by counting Finder windows
  with a small Swift script: no automation prompt needed).

## Live test (2026-09-19): six sessions building landing pages in one repository

`~/worktreeTestWorkspace`, release build with the debug driver, own data folder
(`~/.agentty-dev-worktree-test`). One workspace with a tab of four splits (3 × Claude Code, Codex)
and a second tab, plus a second workspace — all on the same repository.

- The first session stayed in the project; the other five got `agentty/<agent>-<time>` trees,
  including the splits (`launch-split`), which is what the new split target of the + menu does.
- The files panel showed it live: a row per tree with its session, `변경 N`, `↑1` after a commit,
  status dots, the project folder's branch switching to `feat/coffee-landing-page` (the agent
  followed the owner's git rules), "현재 탭" following the active pane.
- Agents opened their own `file://…/worktrees/…/index.html` in the in-app browser to check it.
- Found and fixed: a split could only open a shell (so agents in splits shared the folder); with
  the browser and the files panel both open four splits were left ~220 pt (now the terminals keep
  440 pt). Found, not fixed: every new tree asks Claude Code to trust the folder again and asks for
  each shell command (friction of parallel sessions; "auto mode" in Claude's own prompt helps).
- Codex hit its usage limit and did nothing; Claude warned at 85 % of the session limit with six
  default-model sessions, so the test was not pushed further.

## After the pull request (2026-09-19, not committed yet)

Requests that came in after PR #5 was opened; all tried in the scratch debug app, none committed
(the owner has not asked for a commit).

- Working-tree list of the files panel: height can be dragged (`files_panel_trees_height`, 0 = auto).
- Tour, files task: four steps now — open the panel, pick a session's tree, open a folder, close.
  In a project without linked trees the panel shows example trees for that step
  (`files_panel::demo_trees`, tag "example", nothing on disk or in git, no remove button); with real
  trees those are ringed instead. `TourEvent::TreePicked`; "do this step for me" pins the first real
  tree or picks the first example. `tour <task>` (e.g. `tour files`) jumps to a task.
- Status bar position: `agent_bar_position` (`hud::HudPosition`, default **top** — bottom was the
  default for an hour; the owner tried it and found it awkward — read leniently).
  `layout::bars_below` decides for the single-pane bar *and* the split pane header (one rule, like a
  vim status line); `layout::bar_popover` opens the branch menu and the collaboration panel away from
  the bar. Settings → General "Status bar position", and a question in the basics step of onboarding.
- `row_with_hint` (Settings): a long hint wraps in its column; before, it pushed the control of that
  row out of line with the others (Claude Advisor, working trees).
- Seen, left alone: the debug command `close-tab <index out of range>` opens a close dialog for zero
  terminals. The UI cannot send such an index.

## Notes for whoever continues

- More debug commands: `probe` (one JSON line: panes, listeners, browser URL, files panel, capture
  records, toast — no secrets), `capture start|stop`, `tour [start|do|next]`, `paint` (shows the
  hidden scratch window for half a second and sends it to the back: a covered window does not
  paint, and a pane only starts on its first paint), `onboarding`.
- Synthetic `key` events do not reach a window that is not key: close panes with `close-tab` +
  `close-confirm`, not `key cmd-w`.

- Debug driver additions: `files [path]` toggles / pins the files panel, `launch-agent <kind> <dir>`
  launches through the normal path (so the session-tree logic runs), `page proxy`.
- Scratch check: `scratchpad/run-dev.sh` starts `target/debug/agentty` on a scratch data folder
  (`AGENTTY_BACKGROUND=1`), `scratchpad/dbg.sh <cmd> <arg>` drives it; stop it by pid only.
- The commit guard refuses `git commit` in commands whose folder it cannot read (`cd $VAR`): use
  literal paths and a separate command for scratch repositories.
- New icons must be added to `ui::ICONS` *and* to the icon list in `docs/plugins/README.md`.
- Not done for worktrees: copying untracked files (`.env.local`, `node_modules`) into a new tree;
  merging a session branch back (the Git page / the agent does that); trees are only removed by
  hand from the files panel, and only Agentty's own (`data_dir()/worktrees`).
- Snapshots of the debug app are black while the display sleeps (nothing paints, panes don't even
  spawn): wait for the user rather than waking the screen.
- Toasts used to sit under the native browser view; `render_toast` now moves left of docked panels.
- Seen while testing: a background process that still holds the pane's tty can stay in the "exiting"
  state until Agentty quits (the PTY master seems to outlive the closed pane). Its port was freed at
  once, so it did not matter for stopping servers; not looked into further.
- Not built: decrypting HTTPS in the capture proxy (would need a local CA trusted by each CLI) and
  pointing agent CLIs at a reverse proxy (`ANTHROPIC_BASE_URL`) to see API paths. Both put
  credentials in Agentty's hands, so they wait for the owner's decision.
- Servers are not stopped when a whole window closes or the app quits (panes are dropped without
  `remove_pane`); only tab / pane / workspace close.
