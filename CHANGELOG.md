# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- macOS: install with Homebrew — `brew install --cask empty-user77/agentty/agentty`.

### Changed
- Agentty opens as you left it: the window's size and place (on a screen that is still connected),
  the sidebar hidden or shown, the files panel, the in-app browser with its tabs, a plugin panel,
  the Docker and database panels, and a pane in focus view.
- The recent sessions list shows each agent's logo in its own colours.
- Quitting while terminals are running asks first ("Every session running in Agentty ends when it
  quits"): ⌘Q, the menu, the menu bar popover's Quit and, on Windows and Linux, closing the main
  window. A restart to install an update does not ask.

### Fixed
- A tab opened in a workspace right after its last tab closed was not saved, and opening that
  workspace again from the list closed it.
- A layout file that could not be read in full (written by a newer version, or damaged) no longer
  empties every workspace: what can be read is kept, and the file is copied aside first. The layout
  is flushed to disk before it replaces the old file.
- Panes that end together (a logout or shutdown ending their shells) no longer vanish from the
  saved layout before Agentty quits.
- A pane whose folder is missing at startup (an unplugged drive) keeps that folder in the layout
  instead of switching to the home folder for good.
- Dragging a divider between two very narrow panes no longer crashes Agentty.

## [0.1.21] - 2026-09-23

### Changed
- A new version no longer opens the update popup while a terminal is running, since installing
  restarts Agentty and ends its sessions. The status bar says an update is available; click it to
  install when you are ready. The click checks again first, so the popup offers the newest version.
- macOS: Option is used as Meta by default (Settings → Terminal), so Option shortcuts work in
  shells and agents — word moves and deletes, Claude Code's Option+P / Option+T. Turn it off to
  type characters such as é with Option.

### Fixed
- Windows: panes now remember where you are. A `cd` in PowerShell is followed, so after a restart
  every tab and split reopens in its own folder, and new tabs and splits open where the current
  pane is, instead of all falling back to the workspace folder.
- Windows: Claude Code and Codex running in a pane are now recognised (status, session, model),
  including when installed through npm.
- Claude Code or Codex started by hand in a terminal comes back after a restart, resuming its
  conversation, like an agent tab Agentty started. An agent that moved to another conversation
  (`/clear`, `/resume`) resumes the one it was in last, not the one it started with.
- macOS: ⌘⌫ / ⌘⌦ delete to the start / end of the line and ⌘← / ⌘→ jump to its start and end in
  terminals, Claude Code and Codex, as in iTerm2 and Ghostty. ⌥⌦ deletes the next word.
- Windows and Linux: Alt shortcuts reach the terminal (Alt+B / F / D in shells, Claude Code's Alt+P,
  Alt+T, Alt+V); they did nothing before.
- Windows: Ctrl+V pastes and Ctrl+C copies selected text (and still interrupts without a
  selection), as in Windows Terminal.
- Ctrl+Backspace deletes a word in Claude Code and Codex, and Ctrl+/ is undo.

## [0.1.20] - 2026-09-23

### Added
- The in-app browser has a responsive mode, like Chrome's device toolbar: the phone button next to
  the address bar lays the page out at a device's size (iPhone, Pixel, Galaxy, iPad, laptop,
  desktop) or any width × height, centred and scaled to fit the panel. The size can be typed,
  rotated, or dragged from the right and bottom edges. Agents set it with
  `agentty browser viewport <WxH|device|off>` (MCP: `browser_viewport`) and are told to check
  their pages at phone and tablet sizes.
- Settings → General → Prevent sleep can stay on for a set number of hours (1–72) instead of
  always; it then turns itself off, also when the time ran out while Agentty was closed. The time
  left is shown under the switch. "Always" stays the default.

### Changed
- The AI CLI status bar shows the ports of local servers started in a pane by default (it was
  off until turned on in Settings → Appearance → Status bar).
- "Later" in the update popup keeps it away for 24 hours, across restarts; a newer version is
  announced right away. Meanwhile the update shows in the status bar in place of the version
  ("Update available v0.1.20"), and a click there opens the popup again. The floating update
  button over the workspaces is gone.
- Moving a tab to another workspace shows each workspace's colour as a dot before its name.

### Fixed
- On Windows, the Codex session folders are linked again when Agentty or what starts it runs
  inside a terminal pane: the link is now made directly instead of through PowerShell.
- A Codex session link that an earlier attempt left broken or empty is made again, instead of
  staying broken for good.
- On Windows, terminals use the regular PowerShell 7 install over the Microsoft Store copy when both
  are installed: under the Store copy, what programs write to AppData is redirected into its
  package, where other programs can't find it.

## [0.1.19] - 2026-09-23

### Changed
- On Windows and Linux, terminals keep the bundled JetBrains Mono when no font was picked, and
  D2Coding is no longer recommended there: PowerShell's glyphs break in it. macOS is unchanged.

### Fixed
- Database query results scroll sideways: columns past the right edge of the page can be reached.
- On Windows, linking the Codex session folders and showing notifications no longer fail when
  Agentty (or what starts it) runs below PowerShell 7.

## [0.1.18] - 2026-09-23

### Added
- For someone using Agentty in Korean, terminals use D2Coding — whose Hangul is exactly two columns
  wide — when it is installed and no other font was picked. When it is not installed, the first-run
  setup and Settings → Terminal Style recommend it, with a link to get it.
- Settings → Terminal Style → Font has a "Default" choice, which follows the language.
- Launch opens on a dashboard of the Vercel account when the focused folder is not a web project:
  which command-line tools are installed, which of GitHub and Vercel are signed in, and every
  project on the account with the repository it deploys from drawn as a code → build → live
  diagram. Both logins can be done from there, before there is anything to publish. A folder that
  can be published still opens on the publishing steps, and the two are a tab apart.
- Monitoring → Worktrees lists every git worktree on this computer, grouped by project: when it was
  last worked on, uncommitted changes, commits not merged yet, its pull request and its size.
  Several can be removed at once, with their branches when nothing on them would be lost; the
  project folder itself and worktrees open in a pane are never removed.
- Monitoring → Disk shows how full the disk is, which folders take the room, and what can be
  cleared without anything noticing: each project's build output ("Remove build junk") and caches
  ("Clear cache"), the development tools' caches, the trash and logs. Dependencies and anything a
  project needs to run are never offered.

### Changed
- The first-run setup is shorter, and now includes the terminal font.
- A new AI session opened where another is already at work asks where it goes — "Create a new
  worktree" or "Open the existing worktree" — instead of always getting a worktree of its own: two
  sessions on one branch, to look at it or work on it together, is often what is wanted. The same
  question comes up for `claude` or `codex` typed into a terminal, which waits for the answer.
- Launch no longer asks for a GitHub login it does not need: a signed-in GitHub CLI is used as it
  stands, and so is an SSH key this computer already has — enough to push to a repository that
  exists. A login is asked for only when a repository has to be created.

### Fixed
- The menu behind a plugin panel's layout button drew under the panel's contents; it is on top now.
- Scrollbars can be dragged, and a click on the track jumps there.
- A split pane's bar no longer cuts the model's name: "Opus 5.5 (1M context)" came out as "Opus 5"
  once the pane was narrow. The full name and version always show; short of room the window reads
  "(1M)" and the context meter keeps only its figure, while the folder and branch give way — and the
  pane's own zoom and close buttons stay put.
- A new Claude Code session is named by its own welcome banner until it answers, instead of a guess
  from other sessions: one started with `--model sonnet` shows Sonnet straight away.
- A model picked with `/model` shows on the pane's bar right away, not a few prompts later, and a
  new Claude pane names the model it really runs on.
- Underlined text in the terminal (links, for one) is underlined to its last letter.
- Launch follows the terminal you are looking at: it shows that terminal's folder, opens on the
  publishing steps for a web project and on the dashboard otherwise, and shows a spinner at once
  while a new folder loads. Connections that are all set, and a checklist that is all done, take a
  single line.

## [0.1.17] - 2026-09-22

### Added
- The context panel carries a compact button beside the figure it is about: it runs the agent's
  own compaction command (Claude Code and Codex), whatever the number says. The bar's own compact
  button now appears from 70% — where the meter already turns orange — instead of 80%.
- Double-clicking a split pane's bar — its status, or the icon it is dragged by — enlarges that
  pane, and puts it back on the next double click.
- Clicking a working tree in the files panel goes to the terminal already working in it, on top of
  showing its files. The right-click menu offers the same as its own item, beside "Open a new
  terminal here".
- Removing a working tree whose branch was pushed can delete that branch on the remote as well. The
  tick is off by default, appears only for a branch the repository already knows a remote one for,
  and the remote branch only goes once the local one really went — a branch git kept holds commits
  the remote is the last copy of.

### Changed
- Removing a working tree asks in a dialog that names the branch and says what goes with it. Both
  ways of removing one — the bin on the row and the two menu items — used to want a second click on
  the same red item, which says nothing about what is about to happen and is easy to hit twice.
- The dialog stays on screen with a spinner while git works, instead of closing and leaving the
  panel looking frozen.

### Fixed
- The files panel no longer walks every terminal's folder up to its working tree on every frame and
  every row of the list, which is what made it stutter while a working tree was being deleted.

## [0.1.16] - 2026-09-22

### Changed
- The terminal cursor is a bar. A settings file carries every field, so a block in an old one is
  the previous default rather than a choice: anyone who picked a shape picked one of the other two
  and keeps it.
- A workspace card answers a right click with the menu its `…` opens.

### Fixed
- What is being composed with an input method is back on the line, where the text will land.
  v0.1.15 moved it into a chip below the cursor so it could never cover the program's own text; it
  cannot, but it also flashes a box under every syllable, which is most of what typing Korean,
  Japanese or Chinese looks like. Where it goes is decided by whether the terminal cursor means
  anything: a shell leaves it where the text will land, and a program that draws its own input box
  hides it, so there the composition follows the last thing written on that line instead.
- The context memory panel belongs to the pane whose meter opened it. In a split it opened in every
  pane at once, and then — once that was fixed — in the right pane with another pane's reading.

## [0.1.15] - 2026-09-22

### Added
- The plugin protocol is at version 2. A plugin says which one it was built against, and an
  Agentty that speaks less than that lists it and says to update rather than installing a module
  it cannot run.
- A plugin can wait (`host/timer`) and can hear how the agents it started are getting on
  (`pane/status`, with `workspace.read`, whether or not the window is being drawn) — what a plugin needs to walk a piece of work through
  several agent sessions. `docs/plugins/agentos.md` describes that shape, and Blogger AgentOS in
  the marketplace is a skeleton of it: outline, draft and edit, each one asked of an agent you can
  watch and checked before the next one is offered.
- Plugins can be written as compiled programs: a Rust crate built for WebAssembly, shipped as one
  `.wasm` file that works on macOS, Windows and Linux. Agentty runs the module itself and hands it
  three functions — send a message, write a log line, read the clock — so such a plugin has no
  files, no processes and no network of its own, whatever its code says, and everything it asks for
  goes through the permissions in its manifest. The Rust SDK and two worked examples live in the
  [marketplace repository](https://github.com/empty-user77/Agentty-Marketplace), which is where
  plugins are published.
- A plugin panel opens the way it suits: docked beside the terminals (as before), floating over the
  window, in a window of its own that can be moved and resized, or filling the whole area. The
  plugin says which it prefers; the layout button in the panel's header changes it and remembers.
  A docked panel can no longer be dragged so wide that the rest of the window is squeezed — past
  what fits, it becomes an overlay.
- Plugins choose where their panel is reached from: the activity bar on the left (`sidebar`), the
  tab strip above the terminals (`pane`, where they have always been) or the status bar at the
  bottom (`status`). The activity bar scrolls once there are more items than fit.
- `net.request`: a plugin may make HTTP requests to addresses it chooses. Agentty bounds the
  method, the headers, the body and response sizes, the redirects and the time, sends nothing of
  yours with the request, and writes each call to the plugin's log with the URL redacted. A request
  can go through an HTTP proxy the plugin names. Every address a redirect names is checked like the
  first one, and what was given for one host is not carried to another.
- Agentty bundles no plugin modules: the ones it used to carry are published like everyone else's,
  with their source and their checksum where anyone can read them.
- The marketplace list is kept between runs, so the Plugins page has something to show before the
  network answers — and still does when it cannot reach it, saying how old what it shows is.
  Plugins with a newer version in the marketplace say so in the list, and **Update all** takes
  them one after another.
- A plugin can put text on the clipboard (`host/copy`) — what a panel's "copy this" button needs.
  Agent Rest Client uses it for **Copy as cURL**: the request as it would actually be sent, with
  the environment's values filled in and every value quoted so a shell reads it back as one.
- A marketplace: **Plugins → Marketplace** lists what
  [Agentty-Marketplace](https://github.com/empty-user77/Agentty-Marketplace) offers, and installing
  one downloads its module and checks it against the checksum in the listing — a module that does
  not match is refused before anything is written. Only WebAssembly plugins are offered there, so
  what you install from a list reaches nothing but the permissions you saw before installing it.
  Plugins you keep to yourself are still installed from a folder or a Git repository.
- Agent Rest Client, installed from the marketplace in one click. An HTTP client as a plugin: requests with headers, a body and authorization;
  environments whose values fill in `{{placeholders}}` everywhere in a request; a collection of
  saved requests; a proxy and a timeout; and the response with its status, time, size, headers and
  pretty-printed JSON.
- Plugins can keep things between runs (`storage/get`, `storage/set`, `storage/keys`): one
  document in the plugin's own folder, up to 64 keys and a megabyte, in a file only you can read.
  A plugin needs no permission for its own folder, and what it kept goes when it is uninstalled.
- A panel's text field can be a text area (`"rows": 10`): Enter adds a line, a paste keeps its line
  breaks, and Up and Down walk through the lines. For request bodies and anything else that is
  longer than a line.
- The Plugins page is a list beside details: search, then the plugin's description, what it adds
  and where, its permissions in full sentences, where it came from, how it runs and its log.
- A plugin can ship its own logo: a file in its folder (`logo` in the manifest), or — for a plugin
  that is one module and has no folder — the picture carried inside the module itself, in a
  WebAssembly custom section called `agentty.logo`. It is preferred over the icon name everywhere
  the plugin is shown: the tab strip, the activity bar, the status bar, the panel header, and both
  the list and the card on the Plugins page. A logo is never fetched from an address, so installing
  a plugin tells its author nothing, and the picture drawn is the one the checksum covered.
- Terminal style in detail: letter spacing, bold text, and the background, text, cursor and
  selection colours of the chosen theme, each replaceable and each undoable back to the theme's
  own. A real terminal sits under the controls and shows every change at once — the actual
  terminal, with real colours, bold, underline, CJK and Powerline glyphs, not a mock-up.
- The workspace list has a search box: it matches a workspace's name and the conversations held in
  it, so a workspace can be found by something an agent said in it. It can be turned off
  (Settings → General → Workspaces).
- Slack and Discord notifications take a bot token and a channel, as well as a webhook URL. The two
  ways have separate credentials, so switching one does not send with the other's, and each is only
  accepted in its own shape. Slack takes `#general`, a name or an id; Discord takes a channel id.
- Reserved words have a settings page of their own, above Shortcuts.
- Settings → General is grouped — Basics, AI sessions, Workspaces, System — and every explanation
  is one line.

### Changed

- A workspace card's title, path and marks are drawn in whichever of black or white can be read on
  the colour the card carries, measured against the fill rather than assumed. A pale yellow card no
  longer has white letters on it.
- The list view of the workspace list shows the branch beside the name.
- Context memory opens from the Context meter in the pane's own bar, where the figure it explains
  is, instead of from an icon in the status bar along the bottom.
- Switching into and out of mini mode is a fade rather than a resize. Animating the real window's
  frame down to a sliver made everything in it — every terminal, every pane — lay out again at each
  step of the animation, which is what made the switch feel slow.
- A workspace whose agent finishes stays where it is. It jumping to the top of its group is now a
  setting, off by default.
- Plugin commands are reached from the command palette. `paneBar` and `when` are gone from the
  manifest: the bar above a pane had no room to spare. A manifest that still has them loads and
  installs as before, and simply gets no button there.
- Cosmica and Launch are both reached from the tab strip.

### Fixed

- A terminal moved with `cd` comes back in that folder. Where a pane works is saved when it changes,
  so it survives a window closed, a force quit or a crash — not only a clean quit.
- Session Flow no longer draws two sessions on top of each other. A pane that drops out of the chart
  for a moment (a session reconnecting) and comes straight back took the cell a neighbour was
  already in.
- The search box in the sidebar takes the caret when clicked. The click reached the field and then
  carried on to the sidebar, which handed focus to the list, so nothing typed went anywhere. The
  sessions search box had the same fault.
- Typing and scrolling no longer wait on anything. Drawing a workspace card asked git which
  repository its folder belonged to, and asking that means starting a process and waiting for it, on
  the thread that draws. Every keystroke repaints, so every keystroke started one per card and
  waited for all of them — worse in Korean, Japanese and Chinese, where an input method sends
  several events per character. That answer is worked out once in the background now, and three
  smaller things the repaint did for nothing went with it.
- What is being composed with an input method is drawn beside the grid rather than on it. In an
  agent's prompt a syllable being composed landed on top of the text already on screen until it was
  finished: it was placed in the terminal's cursor cell, and a program that draws its own input box
  leaves that cursor somewhere else entirely.
- A tab that has just opened says which model it is on and that it has read nothing, instead of the
  agent's name and a dash until the first answer arrives.

## [0.1.14] - 2026-09-22

### Added
- Workspace cards redesigned: name, branch and path, pull request and open ports, with a bar marking a pane that is
  waiting for you. A workspace that is closed goes dormant instead of disappearing, and its card reads the same
  before and after it is opened again. Workspaces and groups can be given colours, renamed in place, and dragged
  with the list scrolling under the pointer.
- Closing the last tab of a workspace no longer removes the workspace. Each workspace keeps its recently closed tabs,
  and reopening one restores its splits.
- Session links: pick any pane in the window to link to, and the workspaces a link connects are gathered under the
  one it started from. Unlinking puts the list straight back.
- A database panel beside the terminals, with a SQL editor and a result grid — row numbers, NULLs, row detail, and
  columns dragged to a width that is kept per column name.
- A capture proxy that records the requests and responses of a terminal, with credentials in them redacted. It applies
  to terminals that are already open, and it can point this machine's proxy settings at it while it runs and put them
  back afterwards.
- The start page is a tab of its own, wherever you are. The workspace list's header now switches between the thin
  list and the roomy cards, and what was last picked is what comes back.
- Skills, agents and MCP servers are tabs of the monitoring page, beside usage, processes and the capture proxy.
- Picking a file in the files panel's changes tab opens it as a diff — hunk headers, both line numbers, added and
  removed lines — with the file itself one click away.
- Resume offers to compact a session whose context is nearly full, and jumps to the pane a session is already running
  in. Sessions can be deleted.
- The status bar carries the path of the active terminal, and the AI bar carries the context meter, with a compact
  action past 80%.
- Agentty keeps the machine awake while an agent is working.

### Changed
- Korean calls a workspace 작업공간.
- Pull reports what arrived instead of printing raw git output, and branches and pull requests are read for
  workspaces that are folded away.
- Usage keeps an account that has tokens but no price, refreshes faster, and can be refreshed by hand; the plan meter
  appears from 50% up.
- Floating scrollbars stay out of the way until the pointer is inside the area they belong to.
- Links: text that follows a URL is no longer swallowed by it, file paths open in the editor, and pull requests open
  in the external browser.

### Fixed
- MySQL connects with utf8mb4, and a missing password no longer stops the table list from loading.
- Files and images pasted from the clipboard reach the terminal.

## [0.1.13] - 2026-09-20

### Added
- The in-app browser keeps several pages at once. Each tab has its own view, so background tabs keep loading, and a
  page that opens a new window (`window.open`, `target="_blank"`) becomes a tab instead of being dropped. Browser
  shortcuts — reload, hard reload, new and close tab, address bar, back and forward — now work while the page has the
  keyboard, and a hard reload drops the caches before fetching the page again.
- A network panel under the page: how much the page pulled over the network, the requests it made with status, size
  and time, and, for one of them, its headers and bodies. Its height is dragged from the top edge and kept.
- Plugins are listed as small tiles side by side; opening one takes the whole row and shows its links, permissions,
  folder and logs.
- The status bar shows the branch the active terminal is on, and opens the Git page when clicked.

### Fixed
- Copy, paste and select all reach the page in the in-app browser instead of the terminal behind it.

## [0.1.12] - 2026-09-20

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
  (off by default). Uncommitted changes keep the tree, and a branch with unmerged commits is kept. "Don't ask again"
  in that dialog stops the question without changing "Ask before closing".
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
- Help menu: the guide at agentty.run/docs, the quick start, pages for agent status, sessions, Git, databases, Docker
  and plugins, keyboard shortcuts, settings, troubleshooting and the FAQ, and a link for support. Each opens in the
  language Agentty is showing (English, Korean, Japanese or Chinese; other languages open the English page).
- Settings → About links to the guide, the privacy policy, the terms and the license.

### Changed
- The menu bar item tells finished agents apart from ones that need you: a spinner while agents work, `● N` for agents
  waiting for a permission or an answer, and `✓ N` for agents that finished and haven't been looked at.
- API connector keys use the platform's credential store on Windows (Credential Manager) and Linux (Secret Service);
  macOS keeps using the Keychain.
- New session working trees start from the project's default branch instead of the branch the project folder has
  checked out.
- The first-run tour waits until the environment is ready (nothing important missing in the System check).
- The first-run tour is five steps instead of six.

### Fixed
- Recheck in the System check finds tools installed while Agentty runs, including ones whose installer didn't add
  them to `PATH` (Claude Code in `~/.local/bin`): Windows reads the registry's `PATH` correctly, and macOS and Linux
  ask a fresh login shell.
- The System check's description wraps instead of pushing the Recheck button off the page.
- Installing a tool from the System check keeps the page open, so the other missing tools can be started right away.
  While an install runs, its button leads to the tab it runs in instead of starting it again.

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
