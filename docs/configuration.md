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
  "cursorShape": "block",
  "cursorBlink": true,
  "padding": 8.0,
  "optionAsMeta": false,
  "scrollback": 10000,
  "sidebarWidth": 280.0,
  "askDirectory": true,
  "recentDirs": [],
  "resumeBar": true,
  "harnessDetect": true,
  "harnessPatterns": [".team-harness", "tools/agent/*.yaml"],
  "harnessSubmit": true,
  "harnessAgent": "auto"
}
```

| Key | Values |
|---|---|
| `language` | `en`, `ko`, `ja`, `zh` |
| `theme` | A built-in theme name or the file name (without extension) of an imported `.itermcolors` |
| `cursorShape` | `block`, `beam`, `underline` |
| `askDirectory` | Show the folder picker for new tabs and workspaces |
| `resumeBar` | Offer earlier Claude Code / Codex sessions when a terminal enters their folder |
| `harnessDetect` | Offer to start work through a project's agent harness (see below) |
| `harnessPatterns` | Extra harness patterns, relative to the project (`*` within a name, `**` any folders) |
| `harnessSubmit` | Send the harness prompt right away (`false`: only type it in) |
| `harnessAgent` | `auto`, `claude`, `codex` |

## Agent harnesses

When a terminal enters a project that declares a harness, the bar above it offers
**Start with harness**. The dialog lists the project's entry points, takes a ticket key, link or
prompt, and starts Claude Code or Codex in that project with it — `/implement PROJ-123`, for example.
The project root is the folder itself or a parent up to the repository root.

A project counts as a harness when it has any of these:

- `.harness`, `harness.json`, `harness.yaml`, `harness.yml` or `HARNESS.md`
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
