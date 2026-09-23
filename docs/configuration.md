# Configuration

Agentty stores its files in `~/.agentty/`. Most settings are available in **Settings** (⌘,).

## settings.json

```json
{
  "language": "ko",
  "theme": "Agentty Dark",
  "fontFamily": "JetBrains Mono",
  "fontSize": 13.0,
  "lineHeight": 1.25,
  "cursorShape": "beam",
  "cursorBlink": true,
  "letterSpacing": 0.0,
  "boldText": false,
  "padding": 8.0,
  "optionAsMeta": false,
  "scrollback": 10000,
  "sidebarWidth": 280.0,
  "askDirectory": true,
  "recentDirs": [],
  "resumeBar": true,
  "autoWorktree": true,
  "agentTasks": true,
  "agentGuide": true,
  "stopServersOnClose": true,
  "browser": { "autoOpenServers": true },
  "agentBarPosition": "top",
  "hud": [{ "item": "model", "visible": true }, { "item": "context", "visible": true }],
  "harnessDetect": true,
  "harnessPatterns": [".team-harness", "tools/agent/*.yaml"],
  "analytics": true,
  "harnessSubmit": true,
  "harnessAgent": "auto",
  "externalEditor": "auto"
}
```

| Key | Values |
|---|---|
| `language` | `en`, `ko`, `ja`, `zh` |
| `theme` | A built-in theme name or the file name (without extension) of an imported `.itermcolors` |
| `cursorShape` | `block`, `beam`, `underline` |
| `letterSpacing` | Extra width added to every terminal cell, in points (0–8). A terminal is a grid, so tracking widens the cell rather than the glyph |
| `boldText` | Draw ordinary terminal text bold; what the agent marks bold goes heavier still |
| `colorBackground`, `colorForeground`, `colorCursor`, `colorSelection` | Colours of the chosen theme replaced, as `0xRRGGBB` numbers. Unset (absent) keeps the theme's own. Settings → Appearance → Colours, with "theme's own" to undo |
| `workspaceSearchBar` | The search box above the workspace list (default on). Off hides it, and stops it filtering whatever was typed before |
| `sortFinishedToTop` | A workspace whose agent just finished jumps to the top of its group (default off, so the order you arranged stays) |
| `askDirectory` | Show the folder picker for new tabs and workspaces |
| `resumeBar` | Offer earlier Claude Code / Codex sessions when a terminal enters their folder |
| `notifyAnswerRequests` | An agent asking for permission or an answer notifies even while Agentty is in front, unless you are looking at that pane (default on) |
| `chatNotify` | `slack`, `discord`, `telegram` (on / off), `onFinish` (also when an agent finishes), `details` (include what the agent asks). Slack and Discord take either a webhook URL or a bot token: `slackBot` / `discordBot` pick which, and `slackChannel` (`#general`, a name, or a channel id) / `discordChannel` (a channel id — the API posts to an id, not a name) / `telegramChat` say where the bot writes. Webhook URLs and bot tokens are set in Settings → Notifications and kept in the Keychain, never in this file; the two ways have separate entries, so switching does not send with the other's credential |
| `agentTasks` | Agents may ask (`agentty tasks`) to start parallel tasks in split panes, each in its own worktree; you confirm each request |
| `agentGuide` | Claude Code and Codex started by Agentty get a short guide to Agentty's commands (and Claude Code Agentty's skills), passed on the command line |
| `autoWorktree` | A new AI session in a working tree where another one is at work (opened from the + menu, or `claude` / `codex` typed into a terminal) asks: **Create a new worktree** (`~/.agentty/worktrees/<project>-<hash>/<name>`, branch `agentty/<name>` from the project's default branch) or **Open the existing worktree** (the new session works on the same files). A prompt from a link or a plugin gets a new worktree without asking. `false`: never a new worktree |
| `askRemoveTrees` | Closing the last pane working in a linked working tree asks whether to remove that tree and its branch (default on). Off means the tree simply stays; "Don't ask again" in that dialog turns it off |
| `stopServersOnClose` | Closing a tab, pane or workspace stops the local servers started in it (`SIGTERM`, then `SIGKILL`) |
| `browser.autoOpenServers` | A local server started in a tab opens in the in-app browser once it answers with a page (only while links open in-app) |
| `agentBarPosition` | `top` (default) or `bottom`: the AI CLI status bar, and the header of a split pane, above the terminal or under it |
| `hud` | Items of the AI CLI status bar in order: `model`, `context`, `usage`, `status`, `elapsed`, `links`, `spacer`, `ports`, `worktree`, `branch`, `folder`. `model`, `context`, `status`, `branch` and `spacer` cannot be hidden. Easier in Settings → Appearance → Status bar |
| `harnessDetect` | Offer to start work through a project's agent harness (see below) |
| `harnessPatterns` | Extra harness patterns, relative to the project (`*` within a name, `**` any folders) |
| `harnessSubmit` | Send the harness prompt right away (`false`: only type it in) |
| `analytics` | Consent to anonymous usage statistics (Settings → General). `false`, or `DO_NOT_TRACK=1`, sends nothing; see [metrics.md](metrics.md) |
| `harnessAgent` | `auto`, `claude`, `codex` |
| `externalEditor` | What the file editor's "Open in editor" button uses: `auto` (VS Code when installed, else the system's text editor), `vsCode`, `cursor`, `system` (macOS: default text editor, Windows: Notepad, Linux: an installed desktop text editor). Settings → Project → File editor |

## Accounts (agent sign-in)

**Settings → Accounts** chooses how new Claude Code and Codex tabs sign in. The default, **CLI login**, changes
nothing: the agents use their own login. The other methods are for machines where that login isn't possible:

| Agent | Method | What a new tab gets |
|---|---|---|
| Claude Code | API key | `ANTHROPIC_API_KEY` (+ optional `ANTHROPIC_BASE_URL`) |
| Claude Code | Gateway token | `ANTHROPIC_AUTH_TOKEN` (+ optional `ANTHROPIC_BASE_URL`) |
| Claude Code | OAuth token | `CLAUDE_CODE_OAUTH_TOKEN` — from `claude setup-token`; a pasted `.credentials.json` also works |
| Claude Code | Amazon Bedrock | `CLAUDE_CODE_USE_BEDROCK=1`, `AWS_REGION`, optional `AWS_PROFILE` / `AWS_BEARER_TOKEN_BEDROCK` |
| Claude Code | Google Vertex AI | `CLAUDE_CODE_USE_VERTEX=1`, `ANTHROPIC_VERTEX_PROJECT_ID`, `CLOUD_ML_REGION` |
| Codex | API key | `OPENAI_API_KEY` (+ optional `OPENAI_BASE_URL`) and `CODEX_HOME=~/.agentty/codex-home` |
| Codex | auth.json | `CODEX_HOME=~/.agentty/codex-home` with the imported `auth.json` (Codex refreshes it there) |

- The method and non-secret values are saved in `~/.agentty/agent-auth.json`; keys and tokens in the OS credential
  store (service `run.agentty.agent-auth`). **Test** checks a key against the provider's `/models` endpoint.
- Values reach only the agent process: the pane's environment carries `AGENTTY_AUTH_<NAME>`, the command line
  refers to it (`NAME="$AGENTTY_AUTH_NAME" claude …`), and the shell that remains after the agent exits doesn't
  keep it. Variables of other methods are removed for the agent (`env -u …`), so an `ANTHROPIC_API_KEY` exported
  in a shell profile can't override the chosen method.
- The private Codex home links `config.toml`, `AGENTS.md`, `sessions/`, `history.jsonl`, prompts and skills to
  your `~/.codex` (junctions / copies on Windows without symlink rights), so sessions and `codex resume` work across
  sign-in methods. Its `auth.json` is never linked.
- If the saved method can't be used (e.g. the key was removed from the credential store), the pane says so and the
  agent falls back to its own login.

## Agent harnesses

When a terminal enters a project that declares a harness, the bar above it offers
**Start with harness**. The dialog lists the project's entry points, takes a ticket key, link or
prompt, and starts Claude Code or Codex in that project with it — `/implement PROJ-123`, for example.
The project root is the folder itself or a parent up to the repository root.

A project counts as a harness when it has any of these:

- `.harness`, `harness.json`, `harness.yaml`, `harness.yml` or `HARNESS.md`
- a project skill named `harness` or `harness-<something>`: `<project>/.claude/skills/harness/SKILL.md` or
  `<project>/.claude/skills/harness-*/SKILL.md` (exactly there: not nested deeper, not the user's `~/.claude`)
- a `harness` list in `agentty.json`
- a pattern you add in **Settings → Project**

Claude Code or Codex commands, skills, hooks and instruction files (`.claude/`, `.codex/`,
`CLAUDE.md`, `AGENTS.md`) alone do not count, but once a project is a harness its project-level
commands and skills are offered as entry points.

Commands that take an argument (`argument-hint`) are offered first, then other commands and skills.
A project can name its own entry points in `agentty.json`:

```json
{
  "harness": [
    { "label": "Start a ticket", "command": "/implement", "input": "JIRA key or URL" },
    { "label": "Fix a bug", "prompt": "Reproduce and fix {input} following docs/bugfix.md", "agent": "codex" }
  ]
}
```

`command` gets the input appended; in a `prompt`, `{input}` is replaced by it. `agent` (`claude` or
`codex`) is optional.

## Databases

Connections come from the project's own configuration (see `docs/architecture.md` → Databases); nothing needs to be
set up when it holds host, user, password and database. Otherwise:

- A hidden password (secret manager, environment variable set elsewhere): **Enter password** on the database page. It
  is kept in the Keychain (Windows: Credential Manager, Linux: Secret Service) under `run.agentty.database`.
- A database the configuration doesn't name: **Add** on the database page (engine, host, port, database, user,
  password). Stored in `~/.agentty/db-connections.json` (`0600`) without the password. For Amazon RDS use the
  instance endpoint as the host; TLS is used automatically. "No TLS" is meant for a database on this machine or a
  private network.
- Oracle needs Oracle Instant Client installed (`libclntsh` on the library path).

## Themes

Drop iTerm2 color presets (`*.itermcolors`) into `~/.agentty/themes/` or use **Settings → Import .itermcolors…**.
Thousands of presets are available at [iterm2colorschemes.com](https://iterm2colorschemes.com/).

## pricing.json

Claude model prices are built in. For other models (e.g. those used by Codex), add USD prices per million tokens:

```json
{
  "gpt-6-astra": { "input": 1.25, "output": 10.0, "cacheRead": 0.125 }
}
```

Fields: `input`, `output`, and optional `cacheRead` (default 10% of input), `cacheWrite` (125%), `cacheWrite1h` (200%).
The values above are placeholders — use your provider's published pricing.

## Files Agentty keeps

Everything lives under `~/.agentty` (or `$AGENTTY_DATA_DIR`). Secrets are never in these files: they are in the OS
credential store (macOS Keychain, Windows Credential Manager, Linux Secret Service).

| Path | Purpose |
|---|---|
| `settings.json` | Preferences (language, theme, font, …) |
| `workspaces.json` | Saved workspaces, tabs, splits and groups |
| `themes/*.itermcolors` | Imported color themes |
| `pricing.json` | Optional model prices for non-Claude models |
| `handoffs/` | Context documents created by migrations and Session Flow |
| `connectors.json` | API connector definitions |
| `db-connections.json` | Database connections added by hand (`0600`) |
| `agent-auth.json` | Sign-in method for new Claude Code / Codex tabs (Settings → Accounts) |
| `codex-home/` | Private Codex home for API-key / `auth.json` sign-in (`auth.json` `0600`) |
| `plugins/` | Installed plugins (`state.json` records which are enabled) |
| `plugin-data/<id>/` | Private data of each plugin |
| `prompts/` | Long prompts from plugins and links, handed to agents as files |
| `worktrees/<project>-<hash>/` | Working trees created for agent sessions |

`agentty.json` in a project (or `commands.json` beside `settings.json`) adds command palette entries.
