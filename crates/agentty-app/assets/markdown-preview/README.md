# Markdown preview assets

The editor's markdown preview (`src/editor/preview.rs`) renders a file the way VS Code's markdown preview does, with
the same libraries and styles. They are compiled into the app (`include_str!`) and put inline into a page that has no
address; its Content-Security-Policy lets only these scripts run, by hash.

| File | From | Version | License | SHA-256 |
|---|---|---|---|---|
| `markdown-it.min.js` | npm `markdown-it`, `dist/` | 14.3.2 | MIT (`LICENSE-markdown-it.txt`) | `e32488403e2e565ac12a9669bfdf2b1b876eb0a5c84f8e0699884b562d18eb52` |
| `highlight.min.js` | npm `@highlightjs/cdn-assets` (the common-languages build) | 11.12.0 | BSD-3-Clause (`LICENSE-highlight.js.txt`) | `8ab71eb09c51f501e5e25157d9cff100e46cc29bcbfc744d0b746d451fca7f53` |
| `purify.min.js` | npm `dompurify`, `dist/` | 3.4.16 | Apache-2.0 OR MPL-2.0 (`LICENSE-dompurify.txt`) | `2c90a9b46d6463f26038a29b686e82bc91de01fdac9d5229e7cfe3b360134ea2` |
| `markdown.css` | microsoft/vscode `extensions/markdown-language-features/media/` at `0af7cd09bb5f5adce41ab05d4d20c9fa53fbeebf` | — | MIT (`LICENSE-vscode.txt`) | `31acba626be3846f2a9b78421ce55e94feb5446d2b39bd06ab0453c0a010092d` |
| `highlight.css` | the same folder and commit | — | MIT (`LICENSE-vscode.txt`) | `b73e0ecc7a5b91532f4359206f082b0d73a7d1f7bea68d8853a30385cf85c756` |
| `webview-defaults.css` | the default styles in microsoft/vscode `src/vs/workbench/contrib/webview/browser/pre/index.html` at the same commit | — | MIT (`LICENSE-vscode.txt`) | — |
| `theme.css`, `preview.js` | Agentty | — | Apache-2.0 | — |

`preview.js` sets markdown-it up as VS Code's `markdownEngine.ts` does (raw HTML on, linkify without fuzzy links,
highlight.js with VS Code's language aliases, GitHub-style heading ids, front matter hidden) and sanitizes the result
with DOMPurify before showing it. `theme.css` holds the theme colors VS Code gives its webviews (Dark Modern), with
Agentty's editor background and text.

To update a library: take the new version from npm (`npm pack <name>@<version>`), replace the file, and update the
version and checksum here and in `THIRD_PARTY_NOTICES.md`. The page's script hashes are computed at runtime, so
nothing else changes.
