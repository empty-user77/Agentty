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
