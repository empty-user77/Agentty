# Cosmica for Agentty

Connects [Cosmica](https://www.cosmica.ink/) notes with Agentty.

- **Save this session** (top of the panel): "Save to Cosmica" asks the focused Claude Code / Codex
  agent to write a structured summary into the folder picked below it (Cosmica's folders, `Agentty`
  by default, made on first save; "+ New folder" makes another). A working agent gets the request
  queued. "Log only" stores the raw conversation without AI.
- **Continue from a note**: click a note to continue it in a new tab — in the folder and with the
  agent a saved summary records, else the focused pane's. A few recent notes show first; search finds
  the rest. Row buttons insert into the focused terminal or pick another place.
- **Continue in Agentty** from Cosmica (note menu) opens
  `agentty://plugin/cosmica/continue?path=…`: a new tab with the note typed in, started by Enter.

Cosmica's settings are read automatically: only `notes.path` and the local server port from
`~/.cosmica/config.json`. When Cosmica is running, notes are saved through its local API so they are
indexed and synced right away; otherwise the file is written and Cosmica picks it up on its next start.

Permissions: `prompt.inject` (send note prompts), `terminal.write` (ask the focused agent for a
summary), `session.read` (conversation log), `workspace.read` (the folder of the focused pane, so a
note continues where you are).
