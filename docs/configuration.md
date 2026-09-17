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
  "recentDirs": []
}
```

| Key | Values |
|---|---|
| `language` | `en`, `ko`, `ja`, `zh` |
| `theme` | A built-in theme name or the file name (without extension) of an imported `.itermcolors` |
| `cursorShape` | `block`, `beam`, `underline` |
| `askDirectory` | Show the folder picker for new tabs and workspaces |

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
