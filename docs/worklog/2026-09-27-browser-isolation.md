# 2026-09-27 — one in-app browser tab per terminal

> **Historical work log.** Written while the work was done (September 2026); it describes the code as it was then.
> For how Agentty works today, see the [documentation](../../README.md#documentation).
>
> **작업 당시의 기록입니다.** 현재 앱과 다를 수 있습니다.

Branch `fix/plugin-browser-isolation` from `main` (`0adc5ea`).

## Problem (reported by the owner)

An agent in one terminal ran `agentty browser open http://localhost:<port>/…` and the page of a plugin's
automation — signed in, in the plugin's workspace — turned into that site. The window had one browser; every
`agentty browser` command (and browser MCP tool) went to the tab in front, whoever's it was: another terminal's page, the
user's, or a plugin's. Tabs out of sight were hidden, and WebKit suspends hidden pages, so two terminals could not test
at once either.

## Decisions (agreed with the owner)

| # | Decision |
|---|---|
| D1 | Each terminal (pane) has a tab of its own; its commands reach that tab only. |
| D2 | Selecting a terminal brings its tab forward; what is on screen does not matter for the work — tabs out of sight keep running. |
| D3 | Cookies and sign-ins stay shared (sign in once). Option: Settings → Browser → "Separate cookies and sign-ins for each terminal" (a `WKWebsiteDataStore` per tab, removed with it; macOS 14+). |
| D4 | Closing a terminal closes its tab. |
| D5 | A narrow panel does not change the page: a terminal's page is laid out at 1280 px (or its viewport) on screen and off, scaled to fit the panel by zoom. |

## What changed

- `workbench/terminal_browser.rs` (new): where a terminal's tab is (the panel, the user's browser set aside while a
  plugin workspace is in front, or backstage when no panel is open), `open_terminal_tab`, `close_terminal_tab`,
  `set_terminal_viewport`, and a per-frame sync: backstage tabs join a panel that opens, the selected terminal's tab
  comes forward (only when the terminal in front changes), tabs of closed panes close, each tab keeps its viewport.
- `browser_control.rs`: every command resolves the caller's pane (`BrowserRequest.pane`, set from the connection by
  `agent_signal::serve`) to its tab; no tab → an error, never another tab. `close` closes that tab only.
- `browser.rs`: `BrowserTab.pane / driven_at / viewport / profile`; the "AI drives it" lock is per tab (was one
  timestamp for the window); the user's own link opening never writes into a plugin's or terminal's tab; a page over the
  window (Settings…) and closing the panel keep terminal tabs running instead of dropping them.
- `plugin_workspace.rs`: entering / leaving a plugin workspace merges tabs into the browser set aside instead of dropping
  them when one was already there.
- `webview.rs`: `park_as` — a terminal's view parks at its layout size with zoom 1.
- Texts for agents (CLI help, MCP tool descriptions, agent guide), docs in four languages, the setting in `i18n.rs`.
- Debug driver: `browser-tabs` (every tab, place, owner, real URL), `workspace-at <n>`.

## Tested in a dev build (scratch data folder, local pages, a test plugin logging its page's URL every second)

| Case | Result |
|---|---|
| Two terminals testing at once, A navigating halfway | each kept its own URL throughout; the one behind kept running (JS counter rising); both 1280 wide; background screenshot is the full page |
| The reported case: plugin workspace in front with its automation's page, a terminal behind navigating | plugin page never left its URL (32 checks while it happened, 0 over the whole session of 250+); the terminal's tab navigated where it waited and kept running |
| Selecting a terminal | its tab comes to the front, and back again |
| A terminal with no tab sends click / eval / url | refused with an error, nothing changes |
| User opens a link while a terminal's tab is in front | goes to a user tab; the terminal's page stays |
| Viewport on A (iPhone 15 Pro), then A out of sight | A: 393 wide, phone user agent, still running; B: 1280, desktop |
| Cookies | shared by default (B reads A's cookie); with the option on, the other terminal sees none |
| Settings page over the window while B works | B's tab kept, B's job ran on without a gap |
| Another workspace in front; a terminal behind opens its first tab / navigates | tab added behind, the front tab unchanged |
| Terminal closed | its tab closed, others untouched |
| Panel closed by the user while A works | A's tab kept backstage and running; back in front when the panel reopens |

Not verified: that a separate cookie store is removed from disk when its terminal closes (`removeDataStoreForIdentifier`
is called); Windows / Linux (no in-app browser there).

Known limitation: with separate cookies on, a terminal tab still open when Agentty quits leaves its store on disk (it
is removed only when the tab closes).

## Update: one browser per terminal (same day)

The first version only routed the **agent's** commands to a per-terminal tab. The owner tried the debug build and found
that what people do themselves still shared one browser: the browser button and address bar, links clicked in a
terminal, port chips and servers opening by themselves. That path had not been designed or tested — the testing had
used `agentty browser` only. The owner's rule, verbatim in spirit: open Google from workspace A's terminal and Naver
from another terminal; switching must show each one as it was.

So every terminal now has a **browser of its own** (`Workbench::pane_browsers`, `browser_owner`): the panel shows the
browser of the terminal in front, the others are kept (their pages parked and running), a terminal that never opened
one shows none. Links, port chips and servers open in their terminal's browser (a port chip of another workspace
selects that terminal). The agent keeps its own tab inside its terminal's browser. Closing the panel hides the
terminal's browser (restored when opened again); closing the terminal closes it. Every page is a background web view,
so a terminal's pages keep working behind another's.

Tested on the final build from a fresh data folder (`debug browser-tabs` after each step):

| Case | Result |
|---|---|
| A opens its browser → page-a; B opens its own → page-b; switch A↔B several times | each shows its own page every time; B had none before opening |
| The page of the terminal out of sight | its counter kept rising (121 → 158 in ~7 s) |
| The owner's example with real sites: Google in A, Naver in B | kept apart across switches (screenshots) |
| A split in workspace A | no browser at first; its agent's open/navigate changed only its own |
| Browser button closes, then opens again | hidden (kept, "closed"), then back as it was |
| Settings over the window and back | browser kept |
| Terminal closed | its browser gone, the others untouched |
| Server started in B opens by itself | in B's browser, next to B's own page; A untouched |
| B's port chip clicked while A is in front | B selected, B's server page in front |
| ⌘-click on a link in B's output | B's tab, A untouched |
| Plugin workspace in front, A's agent working behind | plugin page never moved (14 checks); A's agent worked in A's set-aside browser |

## Update: limits on what runs out of sight (same day)

Asked by the owner: cap how many terminals' browsers run in the background, and how much memory the in-app browser
may use (e.g. 30% of the computer's), unloading the oldest first. `workbench/browser_budget.rs`, checked every 15 s:

- **Count** (`browser.backgroundLimit`, default 6): beyond it, the browser out of sight seen longest ago is unloaded.
- **Memory** (`browser.memoryLimit`, default 30%): the footprint of every page's web process (`_webProcessIdentifier`
  → `proc_pid_rusage` `ri_phys_footprint`, a shared process counted once), plugins' pages included, against
  `hw.memsize`; above it, browsers out of sight are unloaded oldest first until the estimate is under.
- Order: idle browsers before those whose agent sent a command in the last 2 minutes; the browser on screen never.
- Unloading drops the views and keeps each page's address; the browser loads again when shown, an agent tab when its
  agent sends a command — page commands then wait for the load (up to 15 s) instead of running on a blank page.
- Settings → Browser has both limits (steppers, 4 languages). Debug: `browser-budget`, `browser-budget unload <pane>`.

Tested (fresh data folders, `debug browser-budget` / `browser-tabs`):

| Case | Result |
|---|---|
| Limit 2, four workspaces each with a browser | the periodic check unloaded the oldest idle one; the older one whose agent was at work was left |
| Unloaded browser's terminal selected | its page loaded again (counter from 0) |
| Memory limit 1% (~368 MB here), four pages of ~190 MB | idle browsers out of sight unloaded oldest first, stopping once under (361 MB); the at-work agent's and the one on screen kept |
| Agent tab unloaded, then its agent sends url / eval / navigate | url answered from the kept address; eval waited for the reload (0, not null); navigate worked; 189 MB again |
| The whole per-terminal browser regression, on the final build | as before |

The first two attempts at the revive case did not test it (the test design picked a browser the rules rightly kept;
then a refused restart left the old app running) — reported as such and redone.

## Update: real Claude Code, cleanup, and Monitoring → In-app browsers (same day)

- **Two real Claude Code sessions** (v2.1.283, launched by Agentty so they get the browser MCP tools; the two scratch
  project folders allow `mcp__agentty-browser` in `.claude/settings.local.json`) were told at the same time to open their
  own project page, click its button, read the result, check the URL four times and take a screenshot. Each reported
  its own page and click result and a URL that never changed; the debug dump agreed (A on :38551, B on :38552 for the
  whole run). They noticed the MCP screenshot tool has no path argument (it saves to the temp folder) — unrelated.
- **Cleanup verified with process ids**: closing a tab (after its confirm dialog) and deleting a workspace closed their
  terminals' browsers, and those pages' web processes exited (`ps`); the other terminal's stayed.
- **Monitoring → In-app browsers** (`workbench/browsers_page.rs`, first tab of Monitoring): one row per terminal browser
  and per plugin — workspace · terminal #n, what runs there, a 240-pt preview (a five-minute cache: taken while the page is open
  for a browser without one or with an older one — every 10 s made the page flicker, the owner asked for this —
  in `data_dir()/cache/browser-previews` (0700), replaced each time, removed with the browser, cleared at start), state
  (on screen / out of sight / closed (kept) / unloaded), memory, last seen; totals against the limits. **Go to
  terminal** (selects its workspace and terminal, shows its browser, an unloaded one reloads) and **Close now** (drops
  the browser; its web process exited) were clicked in the dev build.
- Branch brought up to date with `main` by a merge (no force-push). Debug: `close-workspace <n>`.
- Monitoring (activity bar, menu, ⌥⌘U, palette) opens on In-app browsers, its first tab; the welcome screen's usage row and the tray's usage card still open AI usage.

## Audit before merge (same day)

Bug, security and docs audits over the whole branch; fixed:

- **Two browsers for one terminal**: the panel could become a terminal's (start screen, or a plugin page opening a
  panel) while a browser its agent had made was kept for it; the agent then got `NO_TAB`, and putting the panel away
  overwrote the kept one. Now the kept browser's tabs join the panel (`merge_kept_browser`, each frame and when shown),
  and `put_browser_away` merges instead of replacing.
- **The start screen's browser was dropped** when a port chip or link gave the panel to a terminal: with no owner,
  `put_browser_away` now leaves it for that terminal (as `sync_terminal_tabs` already did).
- **Separate-cookie stores outlived their tab**: checked on disk (`~/Library/WebKit/<app>/WebsiteDataStore/<uuid>`),
  removing a store right after its page was released left the folder there (WebKit keeps a store it still uses), and a
  store was never removed when the app quit with its tab open. Now the removal runs 5 s after the tab closed (each frame
  and each budget check), every store is recorded in `data_dir()/browser-terminal-profiles` (0600), and the ones a
  run left are removed when the next run makes its first terminal page. Closing an agent tab by its X removes its store
  too. Verified on disk: a workspace deleted → its store gone within 10 s; a quit with the tab open → gone after the
  next run's first page.
- **Crash found while testing this (never shipped)**: removing the left stores in `Workbench::new`, before WebKit had
  made any view, crashed on `WebsiteDataStoreIO` (`RunLoop::dispatch` on a null main run loop) — about one start in
  two. The sweep now runs right after the first terminal web view is created; five restarts in a row with left stores
  started cleanly.
- **The budget no longer unloads a browser whose agent sent a command in the last two minutes** (it used to go last;
  with more agents at work than the limit, or plugin pages alone over the memory cap, a working agent lost its page
  every 15 s). The count limit counts every browser running out of sight but unloads only idle ones. Docs changed.
- **A server starting no longer navigates the agent's tab while the agent is at work.**
- Previews: files 0600; a snapshot WebKit never finishes is retried after 30 s; a second window no longer clears the
  first one's pictures (cleared once per process). The ja docs' bold next to `）` fixed.

After the fixes, on the final build: the budget (limit 1) kept the at-work agent's browser and unloaded the
idle ones; a tab close (confirmed) and a workspace delete closed their browsers and their web processes exited;
Monitoring showed all four states with previews.

Security audit found no new exposure: the agent's pane still comes from the connection, every lookup is by that pane,
debug commands stay behind `AGENTTY_DEBUG=1`, preview names cannot leave their folder.
