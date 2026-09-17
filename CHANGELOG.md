# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

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
