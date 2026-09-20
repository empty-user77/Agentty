# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Notifications: when an agent asks for permission or an answer, Agentty notifies you even while it is in front (unless
  you are looking at that pane). Settings → Notifications can also send these to Slack or Discord (webhook) and
  Telegram (bot), optionally also when an agent finishes; the command or question is included only if you turn that
  on. Webhook URLs and bot tokens are kept in the Keychain.
- Windows and Linux support. Terminals, agents, sessions, usage, Git, plugins and connectors work on both; panes use
  PowerShell on Windows. Shortcuts use Ctrl+Shift (⌘), Ctrl+Alt+Shift (⇧⌘), Ctrl+Alt (⌥⌘) and Alt+1…9 (⌘1…9) there,
  so Ctrl+letter stays with the shell. The in-app browser, menu bar item and mini mode remain macOS-only. See
  `docs/platforms.md`.
- Windows: per-user installer (Start menu, `agentty://` links, "Open in Agentty" on folders, PATH, uninstall from
  Settings → Apps), app icon and version information, single instance, and Settings → System check, which finds
  missing tools (Git for Windows, Node.js, Claude Code, Codex, PowerShell 7) and installs them with one click via
  winget. Newly installed tools are found without restarting Agentty. Linux gets the same System check.
- System check on macOS (Claude Code, Node.js, Codex, Git). While something important is missing, the start page
  shows a bar that leads to it.
- `claude` or `codex` typed into a terminal in a project where another agent is at work starts in a working tree of
  its own, like sessions opened from the + menu.
- Pages (Settings, Monitoring, …) have a home tab first that goes back to the start page.
- File editor: click a file in the files panel to view and edit it in a tab next to the terminals. Syntax colors for
  Java, Kotlin, JavaScript, TypeScript / TSX, JSON, YAML, TOML, Markdown, Rust, Python, Go, shell, HTML, CSS, SQL and
  more; undo / redo, IME input, atomic saves that keep file permissions, and a Format button that uses the installed
  formatter (Prettier, google-java-format, ktlint, rustfmt, gofmt, Black) and says what to install when it is missing.
  Files changed on disk (by an agent, git or another editor) reload, or ask when there are unsaved changes. "Open in
  editor" hands the file to VS Code, Cursor or the system's text editor (Settings → Project → File editor). Large,
  binary and non-UTF-8 files, and links to files outside the project, open read-only or with a notice. Project files
  are never run: formatter settings that are code are refused.
- The Docker and plugin panels can be made wider or narrower by dragging their left edge; the width is kept.
- Closing a tab or pane that worked in a working tree of its own offers "Also remove the working tree and its branch"
  (off by default). Uncommitted changes keep the tree, and a branch with unmerged commits is kept.
- "Send to…" (prompts from links and plugins) can add the session as a split pane in the current tab, and says what
  an open workspace does with the prompt (typed into its waiting agent, or a new tab).
- Agent sessions opened from links and plugins in a project where another agent works get a working tree of their own,
  like sessions from the + menu.
- Right-click a working tree in the files panel: show its files, open a terminal there, show it in the file manager,
  copy its path, remove it (with its branch when that is merged) or clean up a tree whose folder is gone.
- Releases include a Windows installer (`Agentty-X.Y.Z-windows-x64-setup.exe`, also as a `.zip`) for Windows 10 1809
  and later, and Linux packages for x86_64: `.deb` (Debian 12+, Ubuntu 22.04+) and `.rpm` (RHEL 9+, Fedora). The
  installer is per user and needs no administrator rights.
- Windows: updates install in place — Agentty downloads the new installer, checks it against the release checksums,
  runs it silently and restarts. On Linux, Agentty announces new versions and links to the packages.
- Docker panel. In a project with a compose file, a Dockerfile or containers of its own, the status bar shows how many
  of its containers are running; clicking it opens a panel with each compose service's image, state and ports, start,
  stop, restart, `compose up -d`, `compose down` (confirmed first; volumes are kept) and logs in a new terminal tab.
  Without compose it lists the project's containers. Environment values are never shown.
- Databases. In a project whose configuration names a database (`.env`, Spring `application.yml` / `.properties`,
  Prisma, compose services), the status bar shows DB n and the database page lists its connections, tables and rows
  and runs read queries. MySQL, MariaDB, PostgreSQL, Oracle, MongoDB and Amazon RDS. Hidden passwords are entered once
  and kept in the Keychain; connections can also be added by hand. Agents read the same databases with `agentty db`;
  a statement that writes or changes the schema runs only after the user approves the exact statement in a dialog.
- Parallel tasks: an agent can split work into several sessions (`agentty tasks`). Agentty asks once ("Start N tasks
  at the same time?"); each task then starts right away in a split pane of the current tab, in its own worktree from
  the project's default branch, with its prompt already sent.
- Claude Code and Codex started in Agentty get a short guide to what Agentty offers them, and Claude Code an Agentty
  skill for parallel tasks, passed on the command line (Settings → General can turn it off).
- Session worktrees of a project you trust in Claude Code are trusted too, so their sessions start without the folder
  question.
- Prompts from links and plugins that arrive while one is being shown wait for their turn instead of replacing it.

### Changed
- The menu bar item tells finished agents apart from ones that need you: a spinner while agents work, `● N` for agents
  waiting for a permission or an answer, and `✓ N` for agents that finished and haven't been looked at.
- API connector keys use the platform's credential store on Windows (Credential Manager) and Linux (Secret Service);
  macOS keeps using the Keychain.
- New session working trees start from the project's default branch instead of the branch the project folder has
  checked out.
- The first-run tour waits until the environment is ready (nothing important missing in the System check).

### Fixed
- Recheck in the System check finds tools installed while Agentty runs, including ones whose installer didn't add
  them to `PATH` (Claude Code in `~/.local/bin`): Windows reads the registry's `PATH` correctly, and macOS and Linux
  ask a fresh login shell.
- The System check's description wraps instead of pushing the Recheck button off the page.

## [0.1.11] - 2026-09-19

### Fixed
- Status bar messages ("Linked with …" and others) go away after a few seconds instead of staying until the next
  message, so a closed session link no longer looks linked.

## [0.1.10] - 2026-09-19

### Added
- Build my idea (start page banner, + menu): describe an app in a chat-style box, paste a plan or drop files, and
  Agentty creates a project with your idea, a build guide and agent settings, then starts Claude Code or Codex to
  build it while previewing the app live in the in-app browser. New idea workspaces go into an "Agentty Idea" group;
  Settings → General → "Use idea mode" hides the entry points.
- Launch plugin (built in): put a project on the web from Agentty — installs the GitHub and Vercel CLIs when missing,
  signs in through their browser logins, creates the repository, pushes, sets environment variables and deploys to
  production with the public URL. When a step fails, "Ask the agent to fix it" hands the error to the agent. Finished
  steps show a green check.
- Launch can connect Supabase: pick or create a project, keep its URL and public key in `.env.local`, and apply
  pending migrations before each "Update site".
- Launch recognizes a repository Vercel already deploys from GitHub and shows the live site, the latest deploy's
  state and recent deploys instead of the first-launch wizard; "Publish my changes" then only pushes to GitHub.
- New start page: the "Build my idea" banner, launch cards for the terminal, Claude Code, Codex and your other
  installed agents, recent sessions one click from running again, and an Explore list.
- Settings → Accounts: start Claude Code with an API key, a gateway token and base URL, a `claude setup-token` OAuth
  token, Amazon Bedrock or Google Vertex AI, and Codex with an API key or an imported `auth.json`, for machines where
  the CLI login isn't possible. Keys are kept in the Keychain and can be tested before use.
- Files panel docked at the right (⌥⌘B, next to the split buttons): the folder structure and changes of the project
  the active tab works in, with git status colors. A path can be typed into the terminal or shown in Finder.
- A working tree per AI session: a new AI session in a project where another one is already at work starts in its own
  git worktree on a new `agentty/…` branch, so sessions never edit the same files. The files panel lists every working
  tree with the sessions in it and what changed there, the status bar shows a working-tree chip, and trees Agentty
  created can be removed from the panel once no session works in them and they are clean. Settings → General turns
  it off.
- The + menu opens what you pick in a new tab, a split to the right or a split below. An agent opened in a split goes
  through the same path as a tab, so it also gets its own working tree when the project is taken. An agent typed into
  a shell of a taken project is pointed out once, with how to get a tree of its own.
- Monitoring → Proxy: start capture, open a tab or AI session, and every connection it makes is listed — endpoint,
  method, status, bytes and duration — with filters by endpoint, tab or text. HTTPS stays encrypted (no certificate is
  installed); records are kept in memory only.
- Status bar customization (Settings → Appearance): drag the items of the AI CLI status bar into any order and hide the
  ones you don't need; model, context, status and branch can be moved but not hidden. A local server ports item can be
  turned on.
- Status bar position (Settings → General, and asked on first run): the AI CLI status bar and the header of a split
  pane can sit under the terminal, like a status line, instead of above it (still the default); menus opened from the
  bar open away from it.
- A local server started in a tab opens in the in-app browser as soon as it answers with a page (when links open
  in-app), and is stopped when the tab, pane or workspace that started it closes. Both can be turned off.
- First-run onboarding: a welcome, the settings worth choosing on day one, and a follow-along tour of split panes,
  Session Flow, mini mode, the in-app browser, the files panel and Monitoring — numbered steps that light the real
  button, notice when you did them and say what just happened; Session Flow brings two example cards to link, and
  the files panel shows example working trees where the project has none yet — both go away with the tour.
  Settings → About shows it again.
- Branch chip in pane headers: ⌘-click opens the branch on GitHub, GitLab, Bitbucket or Gitea in your browser, and
  the chip turns yellow with uncommitted changes and light blue with commits to push.
- Finished-agent notices show the first line of the agent's last reply instead of just "Done".
- Agent harnesses: a project skill named `harness` or `harness-<something>` (`<project>/.claude/skills/…/SKILL.md`)
  now marks the project as a harness, like `HARNESS.md` does.
- In-app browser: a progress bar under the toolbar while a page loads or reloads, and a page that can't be opened
  (a dev server that isn't up yet) says so with a "Try again" button instead of leaving the previous page.

### Changed
- "AI Usage" is now "Monitoring", with AI Usage, AI Processes and Proxy as its tabs.
- The enlarge and close buttons of split panes are always visible, not only on hover.
- With the in-app browser and the files panel both open, the terminals keep at least 440 pt: the browser gives way
  first, then the files panel.
- The folder in the AI CLI status bar and in split pane headers is cut in the middle when it is long
  (`~/code/…/src/app`); hovering shows the whole path and ⌘-click shows the folder in Finder.
- The session viewer stays fast with long conversations.
- The first-launch website tour in the in-app browser is replaced by the new start page and onboarding.

### Fixed
- Reloading the in-app browser now fetches the page from the server, and retries the address shown when the last load
  failed; before, it could reload the previous page or do nothing.
- The context meter dropped only after the next prompt following `/compact`; it now shows the compacted size right away.
- The context meter could show the usage of another Claude Code session working in the same folder.
- A Claude Code pane started with a prompt while the browser tools were on failed with "Invalid MCP configuration".
- Plugins could lose the login shell's PATH when one of its folders contained a space, so "Install tools" failed.
- Text next to a button in a plugin panel row could collapse to one character per line in Korean, Japanese and
  Chinese.
- The resume bar now hides while any agent CLI runs in the pane, not only Claude Code or Codex.
- The Cursor CLI (`agent` / `cursor-agent`) and xAI's Grok Build (`grok`) are recognized while they run in a pane (tab icon,
  status bar name, AI processes), not only in the + menu: their processes are not called like their commands.
- Icons next to text in the AI CLI status bar and pane headers (folder, branch, working tree) sat slightly above
  their labels.

### Security
- Only processes running inside a pane Agentty started can report that pane's status, post notifications or drive
  the in-app browser.
- MCP server details mask credentials by value as well as by name, and a development build can no longer overwrite
  the installed app's connector keys.
- Launch keeps idea notes, agent settings, key files and migrations out of deploys and public repositories, and asks
  before the first push to a remote it did not create.

## [0.1.9] - 2026-09-18

### Fixed
- Split pane dividers could not be moved in a tab where a pane was in focus view (the maximize button in a split
  pane's header). Dragging a divider now leaves focus view, keeping the current layout, and resizes as expected.

## [0.1.8] - 2026-09-18

### Changed
- Harness detection is narrower: a project counts as a harness only when it has a `.harness`, `harness.json`,
  `harness.yaml`, `harness.yml` or `HARNESS.md` file, a `harness` list in `agentty.json`, or matches one of your own
  patterns. Claude Code or Codex commands, skills and hooks alone no longer show the harness bar; in a harness they
  are still offered as entry points.

## [0.1.7] - 2026-09-18

### Added
- Agent harnesses: when a terminal enters a project with its own Claude Code or Codex commands, skills, hooks or
  workflow files, the bar above it offers **Start with harness**. Pick an entry point (such as `/implement`), paste a
  ticket key, link or prompt, choose the agent and start. Projects can declare entry points in `agentty.json`.
- Settings → Project: session suggestions, folder prompts, harness options and your own harness detection patterns.
- The branch menu in pane headers shows the full branch name with a copy button, and can pull (fast-forward only) or
  push without leaving the terminal. Long branch names show in full on hover.

### Fixed
- Update checks work on networks where the GitHub API is rate-limited, such as shared company connections. If the
  release channel still can't be reached, the update window links to the release page to download it directly.

## [0.1.6] - 2026-09-18

### Changed
- The subagent list shows what each subagent was asked to do, what it is working on and its answer, and no longer
  lists Claude Code's internal background tasks.

### Fixed
- Updates download on networks that inspect HTTPS with their own root certificate (for example some company VPNs),
  when that certificate is trusted by macOS.

## [0.1.5] - 2026-09-18

### Added
- Antigravity CLI has its own logo instead of a letter avatar.
- Plugins: a Plugins page to install, enable, restart and remove plugins, install from a folder or a Git repository,
  and create a new plugin that Claude Code builds for you. Plugins can fill a panel next to the terminals, add buttons
  above agent panes and command palette entries, and send prompts to a new workspace, a new tab or an open workspace
  after you pick where in a "Send to…" dialog.
- Cosmica plugin (built in): search Cosmica notes and use them as prompts, continue a note from Cosmica's
  "Continue in Agentty" menu, and save an AI summary or the conversation log of a session into Cosmica.
- `agentty://` links let other apps hand work to Agentty (plugin links, prompts, the Plugins page).
  A prompt that arrives this way is typed in for you to read and send, and a plugin a link reached
  keeps going through that dialog until it is restarted.
- Plugin SDK for Node.js, developer guide, protocol reference and a prompt for building plugins with AI agents. The
  guide opens inside Agentty.
- Session links from the terminal: the link button above an agent pane connects it to another session without going to
  the Session Flow page, including picking the pane to work with on screen. Two Claude Code sessions are introduced to
  each other and message each other directly; everything else keeps getting the conversation as a document. Links can be
  closed from the same place, one at a time or all at once.
- Tabs and split panes can be rearranged by dragging: a tab dropped into another tab joins its layout where you choose,
  and a split pane moves to another position (or another tab) when dragged by the icon in its header.
- First launch opens what Agentty can do next to the start page.
- The "Ungrouped" section of the sidebar folds away like a group, group names are easier to read, and
  a group's buttons no longer crowd its row or push the workspace count off the edge.
- The start page scrolls in a short window instead of running under its own footer.
- The start page is reachable again once workspaces exist (the house button in the sidebar header), Claude Code and
  Codex are always offered with a link to their install guide when they are missing, and the page has a footer with the
  website, release notes and GitHub.

### Changed
- An agent between tool calls is shown as "Thinking…" instead of "Ready", and the running-turn hint of newer Claude Code
  versions is recognised again, so a working agent is no longer shown as idle. A pane that stays silent for a minute and
  a half goes back to "Ready", so a missing Stop hook cannot leave "Thinking…" up for good.

### Fixed
- Closing a session link from the Session Flow page now also tells both sessions to stop messaging
  each other, as closing it from the terminal already did.
- Reconnecting two sessions that already message each other no longer hands one a copy of the
  other's conversation.
- A pane follows the session its agent is actually running, so two agents in the same folder no
  longer show each other's model, context and subagents.
- An agent that asks a question after a long pause is reported again instead of staying quiet.
- Korean and other composed text is drawn on the terminal's own grid while the input method is still
  composing, so a syllable no longer lands on top of the text that was just committed.
- A workspace where one pane finished while another keeps working now says "Working" in the sidebar
  instead of "Done"; the finished pane still shows its own state and keeps its unread mark.
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
