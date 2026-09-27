---
title: In-app browser
description: A browser panel beside your terminals, dev servers that open themselves, and letting an agent drive the page.
---

**⇧⌘B** opens a browser panel next to your terminals. On macOS it is a WebKit view inside Agentty; on Windows and Linux links open in your default browser instead.

## Opening things in it

- ⌘-click a link in any terminal (Ctrl-click on Windows and Linux).
- **Settings → General** decides whether ⌘-click opens links here or in your default browser.
- Type a URL, or a search term — the search engine is configurable.

## Dev servers

A local server started in a tab appears as a `:port` chip in the workspace list and opens in the panel as soon as it answers with a page.

Closing the tab that started a server stops the server. Turn either behavior off in **Settings → General** and **Settings → Browser**.

## Responsive mode

The phone button beside the address bar lays the page out at a device's size instead of the panel's: iPhone SE, iPhone 15 Pro, iPhone 15 Pro Max, Pixel 8, Galaxy S24, iPad mini, iPad Air, iPad Pro 12.9", Laptop, Desktop — or any width and height you type. The page is centred and scaled to fit whatever room the panel has, so a 1920-wide desktop layout is legible in a narrow panel. Rotate swaps the two numbers, and dragging the right or bottom edge resizes by hand.

Turning it off returns the page to the panel's own width.

## Letting an agent drive it

Claude Code and Codex can control this browser through MCP: click, type, read the console, take a screenshot. It is how an agent checks its own work on a running page.

The tools an agent gets are:

| Tool | What it does |
|---|---|
| `browser_open` | Open this terminal's tab (made the first time), optionally loading a URL |
| `browser_navigate` | Load a URL, `host:port`, or search words |
| `browser_status` | Current URL, title, and whether the page is still loading |
| `browser_wait_load` | Wait until loading finishes |
| `browser_click` | Click an element |
| `browser_console` | Read the page's console output |
| `browser_screenshot` | Capture the page |
| `browser_viewport` | Lay the page out at a device size, `WxH`, or `off` |
| `browser_close` | Close this terminal's tab |

They are registered with the agent when Agentty starts it, so there is nothing to install or configure. A typical loop is: the agent starts the dev server, opens the page, clicks through the change it just made, reads the console for errors, and fixes what it finds.

### One browser per terminal

Every terminal has a browser of its own. Open the browser in one terminal and go to a site, open it in another and go to another: selecting a terminal shows its browser, as it left it, and a terminal that never opened one shows none. Pages of the terminals out of sight keep running — loading, scripts, screenshots — so work in one never waits for you to look at it.

- **Links and servers go to their terminal.** A link clicked in a terminal, the port chip of a server it started, and a server that opens by itself all open in that terminal's browser. A port chip on another workspace's card selects that terminal.
- **An agent works in a tab of its own** in its terminal's browser, made on its first `browser_open` and labelled with the terminal's folder and number (`app #3`). Everything that agent does goes to that tab only: it never changes another terminal's page, a page you opened, or a plugin's page. Its page is laid out at 1280 px (or the size set with `browser_viewport`) whether it is on screen or not, and scaled to fit the panel when it is, so what it checks does not depend on how wide your panel is.
- **Limits keep it light.** At most 6 terminals' browsers run out of sight, and all in-app browser pages together use at most 30% of the computer's memory (both in **Settings → Browser**). Beyond either, the browsers out of sight are unloaded, the one seen longest ago first — never the one on screen, nor one whose agent sent a command in the last two minutes. An unloaded page keeps its address and loads again when its terminal is selected or its agent sends a command (which then waits for the page to load).
- **Closing the panel hides the terminal's browser**; it comes back as it was when you open it again. **Closing a terminal closes its browser.**
- **Cookies and sign-ins are shared**, so you sign in to a site once. To give each agent's tab cookies of its own, turn on **Settings → Browser → Separate cookies and sign-ins for each terminal** (macOS 14 or later); they are removed when the terminal closes.

> [!IMPORTANT]
> The browser keeps the sessions you are signed into. An agent driving it acts inside those sessions. Enable it when you want that, and be aware of which tabs are open.
