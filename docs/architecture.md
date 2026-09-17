# Architecture

```
┌──────────────────────────── agentty-app (GPUI) ────────────────────────────┐
│ Workbench (root view)                                                      │
│  ├─ groups → workspaces → tabs → PaneNode tree → TerminalView (pane)       │
│  ├─ side bar: workspaces / local sessions                                  │
│  ├─ pages: Git · Session Flow · AI Usage · Extensions · Settings           │
│  └─ picker (folder), launcher, status bar                                  │
│                                                                            │
│ TerminalView ── Backend ── alacritty_terminal::Term + PTY I/O thread       │
│      ▲                                                                     │
│      └── agent signals ◀── Unix socket ($TMPDIR, 0600) ◀── agent hooks    │
└───────────────────────────────┬────────────────────────────────────────────┘
                                │ links
┌──────────────────────── agentty-bridge (library + CLI) ────────────────────┐
│ claude.rs / codex.rs  transcript discovery and parsing                     │
│ usage.rs / pricing.rs token accounting and reports                         │
│ handoff.rs            context documents for migration and sharing          │
│ extensions.rs         skills / agents / commands / plugins / MCP discovery │
│ connectors.rs         API connectors (Keychain secrets, stdio MCP server)  │
│ git.rs                git CLI wrapper for the Git page                     │
└────────────────────────────────────────────────────────────────────────────┘
```

## Terminal pipeline

1. `launch.rs` builds the command line. Everything runs through the user's login shell; agent panes append
   `; exec $SHELL -l` so the pane survives the agent exiting.
2. `terminal/backend.rs` spawns the PTY and alacritty's event loop thread, which parses output into the grid.
3. Events (wakeups, title, bell, exit) cross to the UI thread over a channel and are coalesced into one repaint.
4. `terminal/view.rs` paints the visible grid in `prepaint`: background spans, text runs grouped by style (wide and
   Nerd Font glyphs are isolated so they stay on the cell grid), cursor and IME pre-edit text.

## Agent status

- **Claude Code** is launched with `--session-id <uuid>` and `--settings '{"hooks": …}'` so `UserPromptSubmit`,
  `PreToolUse`, `Stop` and `Notification` hooks report to Agentty. User settings files are untouched.
- **Codex** is launched with `-c notify=[…]`; its turn-complete notification reports `stop`. Pressing Enter in a
  Codex pane marks it as working.
- Hooks write `<pane id>\t<kind>\t<json>` lines to `$AGENTTY_SOCKET`; `agent_signal.rs` parses them and the
  workbench updates the pane. `Stop` and `Notification` set the pane's attention flag until the user clicks it.

## Persistence

`workbench/persist.rs` stores groups, workspaces, tabs and split trees. On launch only the active workspace spawns
processes; others stay dormant until opened. Agent panes resume their session when its transcript still exists.

## Session Flow

Dragging from a node's handle to another node creates an edge. The source transcript is converted by
`handoff::create_share` into a Markdown document under `~/.agentty/handoffs/`, and a prompt asking the target agent to
read it is submitted to the target pane (bracketed paste + Enter).

## Git page

`git_view.rs` resolves repository roots from the working directories of open panes (the active pane's repository is
selected when the page opens). `agentty_bridge::git` shells out to the user's `git` with `GIT_TERMINAL_PROMPT=0`, so
credentials, hooks and signing follow the user's configuration and nothing can block on a prompt. All commands run on
background threads; the page polls status every 3 seconds only while visible. Branch names are validated with
`git check-ref-format` and commit ids must be hex, so user input is never parsed as an option.

## Mini mode, menu bar and notifications

- `workbench/mini.rs` folds the main window into the top-right corner with AppKit frame animation (`native.rs`),
  orders it out, and opens a non-activating `PopUp` window with `MiniView`. The panel observes the workbench;
  notices arriving while it is open become speech bubbles. Restoring grows the main window back from the panel.
- `status_item.rs` owns an `NSStatusItem`. Menu clicks are queued from Objective-C and drained by a GPUI task that
  also advances the spinner (every 120 ms while an agent works, 700 ms otherwise).
- `notifications.rs` uses `UNUserNotificationCenter` inside the app bundle (click → `TrayAction::Focus(pane)`) and
  falls back to `osascript` for unbundled development builds.

## Live links

A live Session Flow edge remembers how many source turns it delivered. When the source agent finishes a turn, only the
new turns are written to an update document and submitted to the target (queued if the target is busy). The target's
next finished turn is its reply to that context and is not forwarded back, so two-way links cannot loop.
