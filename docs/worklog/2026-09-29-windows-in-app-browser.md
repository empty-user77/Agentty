# 2026-09-29 — in-app browser on Windows (WebView2)

> **Historical work log.** Written while the work was done (September 2026); it describes the code as it was then.
> For how Agentty works today, see the [documentation](../../README.md#documentation).
>
> **작업 당시의 기록입니다.** 현재 앱과 다를 수 있습니다.

Branch `feat/windows-in-app-browser` from `main` (`a1bdc87`).

## Request (owner)

The in-app browser on Windows, the same as on macOS: the panel, an AI driving it (`agentty browser`, the browser MCP
tools), plugins driving it (`browser/*`), one browser per terminal. If Chromium is needed, clicking the browser button
says a component must be installed and lets the user download it.

## Decisions

| # | Decision |
|---|---|
| D1 | Microsoft Edge WebView2 (Chromium). It ships with Windows 10 and 11; where it is missing, opening the browser shows a dialog that downloads Microsoft's Evergreen bootstrapper (`go.microsoft.com/fwlink/p/?LinkId=2124703`), runs it only if its Authenticode signature is valid and signed by `O=Microsoft Corporation`, and then opens the browser as asked. "Open in default browser" is the other way out. |
| D2 | Same API as `webview.rs` (macOS) so no call site changes: `platform/windows/webview.rs` is compiled as `crate::webview` on Windows. |
| D3 | Each page is a WebView2 controller in a window ("host") that Agentty's window owns — a pop-up, not a child — moved over the placeholder every frame like the WebKit view, and with the window when it moves (a WinEvent hook). See "Update: owned windows" below for why not a child window. |
| D4 | Scripts (`call_async` / `call_in_world`) and screenshots go through the DevTools protocol: `Runtime.evaluate` awaits promises and ignores the page's CSP (what `callAsyncJavaScript` does); a plugin's world is `Page.createIsolatedWorld`, kept per document and made again when the document changes. |
| D5 | Profiles (separate cookies per terminal, plugins' profiles) are WebView2 profiles (`ProfileName`, runtime ≥ 101); private mode is InPrivate. Data folder `~/.agentty/webview2`. |
| D6 | Pages out of sight keep running: browser arguments turn off Chromium's occlusion detection and background throttling; parked pages stay visible at a desktop size outside the window, as on macOS. |

## What changed

- `platform/windows/webview.rs` (new): environment and page creation queue, navigation filter (same schemes as macOS),
  load errors, pop-ups → new tabs, browser shortcuts (F5, Ctrl+R, Ctrl+Shift+R, Ctrl+T, Ctrl+W, Ctrl+L, Alt+←/→),
  console/network capture scripts, zoom, mobile user agent, lock (disabled host window), park/hide, cookies
  (get/set/clear, pending until a page of the store exists), profile removal, focus hand-back, edit commands, renderer
  process lookup for the memory limit, runtime detection and installer.
- `platform/page_scripts.rs` (new): console/network scripts and `navigation_allowed`, shared by macOS and Windows.
- `workbench/webview_setup.rs` (new): the install dialog; `browser.rs` shows it from `open_browser`.
- `browser_budget.rs`: process memory (private commit) and system memory on Windows.
- Gates on `webview::available()`: terminal links, servers opening by themselves, plugins' `browser/*` (error names the
  missing runtime), agents' `agentty browser open` (same message).
- Settings: "Allow developer tools (F12, right-click → Inspect)" on Windows instead of Safari's Web Inspector.
- Docs (en/ko/ja/zh `in-app-browser.md`, `platforms.md`, `docs/platforms.md`).

## Review by reading (no Windows machine at hand)

Found and fixed while going through the logic against the macOS module and every caller:

| Problem | Fix |
|---|---|
| `wait-load` right after `navigate` returned at once: WebView2 says a load started only later (`NavigationStarting`), WebKit's `isLoading` is true at once | a load is marked started when asked for; `back`/`forward` only when there is history; taken back when it cannot start; a same-document move ends it (`HistoryChanged`) |
| Ctrl+F3 / Ctrl+F8 / numpad keys taken for Ctrl+R / Ctrl+W / … (virtual-key codes 0x60–0x7A share values with `a`–`z`) | shortcuts from letters A–Z only |
| Frame, zoom or visibility changed while the page was being set up were lost | applied again when the page becomes ready |
| A plugin asking who is signed in could hear "nobody" while WebView2 was still starting (the warm-up view waits 600 ms) | cookies of a page still being made are read once it is ready |
| Work queued for a page whose view was dropped never got an answer | answered with an error on the next turn of the message loop (never from inside the drop) |
| View dropped while WebView2 was still making its page: the host window was destroyed first | the host stays hidden until the page is made, then both go |
| A script in a plugin's world could run twice after "Execution context was destroyed" | only "Cannot find context" (the script never ran) is tried again |
| A script returning a DOM node failed the call (`returnByValue`) | only strings come back, anything else is `null`, as on macOS |
| A download link showed "The connection was aborted" over the page; WebView2 saved files where WebKit's view downloads nothing | downloads are cancelled (as on macOS), and not shown as a failed load |
| `alert`/`confirm`/`beforeunload` stopped the page behind a native dialog | default script dialogs off: dismissed at once, as on macOS |
| GPU / utility process restarts were shown as "The page stopped working"; a browser process crash left every new page failing | only the page's own process or the browser's; a browser crash lets the next page start a new one; a failed start is tried again |

## Verified

- Type-checks and passes clippy (`-D warnings`) for `x86_64-pc-windows-msvc` from macOS (C code of dependencies not
  compiled: a stub compiler, check only). macOS: fmt, clippy, all tests, release build.
- The plain logic (`platform/webview2_logic.rs`: script building, reading `Runtime.evaluate` answers, shortcuts,
  signature subject, profile names, screenshots) is compiled and unit-tested on every platform.
- The JavaScript sent to `Runtime.evaluate` was run in Node (V8): strings / `null` as on macOS, thrown errors'
  messages, `agentty browser eval`'s expression-then-statements retry (`SyntaxError`), the console / network capture
  scripts (network only in the top document), the network panel's readers, and every `agentty browser` command's
  script parse.

## Not verified (needs a Windows machine)

Nothing here ran on Windows. To try on a Windows PC: the panel shows pages over the terminals and they follow the window,
typing and clicking in a page, Ctrl+C/V inside it, focus returning to terminals on click, two terminals' pages running
at once, `agentty browser open/click/eval/screenshot/console`, a plugin's `browser/*` calls and isolated world state,
separate cookies option, responsive mode, the install dialog on a PC without the runtime.

## Update: owned windows instead of child windows (2026-10-01)

The first version put each page in a child window and told GPUI to draw without DirectComposition
(`GPUI_DISABLE_DIRECT_COMPOSITION=1`), whose layer covers child windows. Tried on Windows by the owner: the pages
showed, but the NVIDIA overlay ("press Alt+Z") came up — drawn straight into the window, Agentty looks like a game
to overlays (NVIDIA, Steam, Discord), which then hook into it — the app closed a little later, without a
`crash.log` (a native crash in the hooked code, not a Rust panic), and moving the window stuttered.

Now GPUI keeps DirectComposition, and each page is a pop-up window owned by Agentty's window (`WS_POPUP`, a tool
window: never in the taskbar or Alt+Tab). An owned window always stays above its owner and is hidden with it when
it is minimized; a WinEvent hook (`EVENT_OBJECT_LOCATIONCHANGE`, this thread) moves the pages with the window, whose
frames are not drawn while it is dragged. Parked pages sit at screen position -30000. Clicking into a page makes its
window the active one, so the keyboard is handed back by activating Agentty's window again (`SetActiveWindow`
before `SetFocus`); while a page has the keyboard, Agentty's window counts as inactive (its title bar dims, plugin
context is not sent). Alt+F4 in a page closes Agentty, not the page's window.

## Not included

`media/htmlToVideo` (a plugin recording an HTML page into an MP4) stays macOS-only: its MP4 encoder is AVFoundation,
and Windows has no encoder in Agentty yet (`platform/video.rs`, which also keeps `media/svgsToVideo` macOS-only).
