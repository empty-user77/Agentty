# Cosmica for Agentty

Connects [Cosmica](https://www.cosmica.ink/) notes with Agentty.

- **Notes → prompt**: search your Cosmica notes in the panel, insert one into the focused terminal or
  continue it in a new workspace, a new tab or an open workspace.
- **Continue in Agentty** from Cosmica (note menu) opens
  `agentty://plugin/cosmica/continue?path=…` and lands here.
- **Session → Cosmica**: the notebook button above a Claude Code / Codex pane asks the agent to write a
  structured summary into `Agentty/` in your Cosmica notes. "Save conversation log" stores the raw
  conversation without AI.

Cosmica's settings are read automatically: only `notes.path` and the local server port from
`~/.cosmica/config.json`. When Cosmica is running, notes are saved through its local API so they are
indexed and synced right away; otherwise the file is written and Cosmica picks it up on its next start.

Permissions: `prompt.inject` (send note prompts), `terminal.write` (ask the focused agent for a
summary), `session.read` (conversation log), `workspace.read` (the folder of the focused pane, so a
note continues where you are).
