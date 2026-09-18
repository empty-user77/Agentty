# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Plugins: a Plugins page to install, enable, restart and remove plugins, install from a folder or a Git repository,
  and create a new plugin that Claude Code builds for you. Plugins can fill a panel next to the terminals, add buttons
  above agent panes and command palette entries, and send prompts to a new workspace, a new tab or an open workspace
  after you pick where in a "Send to…" dialog.
- Cosmica plugin (built in): search Cosmica notes and use them as prompts, continue a note from Cosmica's
  "Continue in Agentty" menu, and save an AI summary or the conversation log of a session into Cosmica.
- `agentty://` links let other apps hand work to Agentty (plugin links, prompts, the Plugins page).
- Plugin SDK for Node.js, developer guide, protocol reference and a prompt for building plugins with AI agents. The
  guide opens inside Agentty.
- Session links from the terminal: the link button above an agent pane connects it to another session without going to
  the Session Flow page, including picking the pane to work with on screen. Two Claude Code sessions are introduced to
  each other and message each other directly; everything else keeps getting the conversation as a document. Links can be
  closed from the same place, one at a time or all at once.
- Tabs and split panes can be rearranged by dragging: a tab dropped into another tab joins its layout where you choose,
  and a split pane moves to another position (or another tab) when dragged by the icon in its header.
- First launch opens what Agentty can do next to the start page.
- The start page is reachable again once workspaces exist (the house button in the sidebar header), Claude Code and
  Codex are always offered with a link to their install guide when they are missing, and the page has a footer with the
  website, release notes and GitHub.

### Changed
- An agent between tool calls is shown as "Thinking…" instead of "Ready", and the running-turn hint of newer Claude Code
  versions is recognised again, so a working agent is no longer shown as idle. A pane that stays silent for a minute and
  a half goes back to "Ready", so a missing Stop hook cannot leave "Thinking…" up for good.

### Fixed
- The model, context meter and subagent count of a pane keep updating after `/clear` or a resume, which start a new
  transcript; they used to freeze at the numbers from the moment the session was forked.
- Text sent to a terminal by a plugin or a link can no longer end the bracketed paste early, which could run the rest of
  the text as commands.

## [0.1.4] - 2026-09-18

### Fixed
- Reserved words (Settings → Reserved Words) now also work in the shell that opens after Claude Code or Codex exits in
  an agent tab.

## [0.1.3] - 2026-09-18

### Changed
- Settings → General has been simplified.

## [0.1.2] - 2026-09-17

### Added
- Claude advisor (experimental Claude Code feature): choose a default for new Claude tabs in Settings → General, and
  switch it for a running tab from the status bar or the command palette. Switching restarts Claude Code and resumes
  the same conversation; global Claude Code settings are not changed.
- Status bar shows the Agentty version and the installed Claude Code and Codex CLI versions.
- Settings → Font: a "More" list with every font installed on the Mac, each previewed in its own typeface.

### Changed
- The update dialog shows download progress (size and percentage), signature verification and restart steps instead
  of closing without feedback.

### Fixed
- Shift+Enter inserts a new line in agent prompts instead of sending them.

## [0.1.1] - 2026-09-17

First public release.

### Added
- Session context panel (brain icon in the pane status bar): memory files, skills, files in context, compaction summary
  and usage breakdown for Claude Code and Codex sessions.
- AI processes page next to AI usage: CPU, memory and disk I/O of local agent process trees with active / idle /
  stopped status and the pane each one runs in.
- macOS menus (File, Edit, View, History, Window) and a Dock menu; recently closed windows can be reopened, and
  folders opened with Agentty appear in the Dock's recent items.
- Tab drag and drop: reorder tabs, or drop a tab on a sidebar workspace to move it there.
- Workspace groups can be created before any workspace, and new workspaces can be created directly in a group.
- Activity bar tooltips with keyboard shortcuts; a "+" button right after the last tab.
- Anonymous usage statistics in release builds, with a switch in Settings → General (see docs/metrics.md).
- Native GPU-rendered terminal (GPUI + alacritty_terminal) with IME, selection, scrollback and Nerd Font symbols.
- Workspaces → tabs → split panes, workspace groups with drag and drop, renaming and layout restore.
- Live agent status for Claude Code and Codex, attention borders and sidebar badges.
- Folder picker for new tabs and workspaces.
- Local session browser with resume and Claude ⇄ Codex migration.
- Session Flow: connect sessions to share context.
- AI usage dashboard (cost, calls, cache hit rate, tokens, models, projects, tools).
- Settings: language (en/ko/ja/zh), themes with iTerm2 import, font, cursor, padding.
- Git page modeled on GitHub Desktop: repository and branch switchers, fetch/pull/push/publish, changes with
  per-file selection and discard, commit, history with per-file diffs, new branch and merge.
- Extensions page (skills, subagents, commands, plugins, MCP servers) with an official MCP catalog.
- API connectors served as MCP tools with Keychain-backed secrets.
- Pane status bar with model name, context and usage meters.
- Notification center, system notifications, jump to unread and `agentty notify`.
- Command palette with custom commands, reserved words (command aliases), session favorites.
- Signed and notarized DMG packaging (`scripts/build-dmg.sh`).
- Mini mode: floating panel with agent status and speech-bubble alerts; menu bar item with running animation, agent
  list and close-to-menu-bar.
- Native desktop notifications (UserNotifications) that open the pane on click; option to notify while focused.
- Live and two-way Session Flow links that forward new turns, with reply loop prevention and busy-target queueing.
- Launcher detects installed agent CLIs and Ollama models; Claude Code model aliases and recent Codex models.
- Branch switcher dropdown in pane headers, with "New branch".
- Auto-update from the release channel (launch + hourly check, popup, sidebar badge, verified install and relaunch).
- Settings reorganized (General / Terminal Style / Keyboard Shortcuts / About) and "About Agentty" dialog.
- Resume bar for earlier sessions in the current folder; full-text local session search.
- Service status banner for Claude Code, Codex and Amp.
- Tab context menu: close tab / other tabs / all tabs, move to another workspace.
- Extension details with the definition file, Show in Finder.
- Session Flow nodes show workspace and tab.
- Claude Code usage meter via a statusline wrapper that keeps the user's own statusline.
- Launcher: Claude/Codex models from local settings and sessions; Gemini and Amp up front, others under "Other models".
- New app icon, menu bar mark and styled DMG installer window.
- In-app browser (WebKit) with link-opening preference; ⌘-click links and OSC 8 hyperlinks.
- Find in terminal, font zoom, file drop, listening ports per workspace.
- Mouse reporting (SGR/X10) so full-screen TUIs like Claude Code scroll with the wheel.
- Status bar skills / agents / MCP menus.

### Fixed
- Scroll wheel in Claude Code panes cycled prompt history instead of scrolling.
- Terminal count in the status bar counted every workspace.
- Scrolling or dragging inside popovers also moved the terminal behind them.
- Closing a pane now also ends agents (Claude Code, Codex) that ignored the terminal hang-up.
- Missing translations in several placeholders and menus.

### Security
- OSC 8 terminal hyperlinks only open http(s) and mailto targets.
- MCP server details show the redacted command instead of the shared agent configuration file; credential headers in
  MCP arguments are redacted.
