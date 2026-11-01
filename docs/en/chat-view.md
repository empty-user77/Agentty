---
title: Chat view
description: Show an agent's pane as a desktop-style chat while the CLI keeps running underneath.
---

Chat view draws a Claude Code, Codex, Antigravity or Gemini CLI pane as a chat, the way a desktop chat app does. The CLI keeps running in the pane: switching between chat and terminal keeps the session and everything in it.

## Turning it on

- Click the speech bubble in the pane's title bar, choose **View → Chat View**, or press **⇧⌘J**. Each switches only the pane in front.
- **Settings → AI settings → How agents open** picks the mode new agents start in (terminal or chat). You also pick it in onboarding. An agent started again in a pane, after the last one quit, opens in this mode.

## The conversation

- Replies are formatted, and code blocks, commands and file reads use the editor's syntax colors.
- Tool steps fold into cards that open downwards; to-do lists show as checklist rows.
- Approvals and questions with numbered options can be answered from the chat. When the agent shows its own menu or a setup screen, its terminal comes up until that is done.
- Select text across paragraphs and code and copy it with **⌘C**. The bar over a selection (or a right-click) quotes it in a reply or starts a side chat with a question about it.
- Attach files with **+** or by dragging them in; they show as thumbnails and chips.
- **Stop** interrupts the running turn. A message the agent puts back after a stop returns to the composer.

## The start screen

A new session shows the agent and model, the folder (opens the files panel), the branch (opens Git), a few ways to begin that fill the composer without sending, and the agent's earlier sessions in this folder, picked up again with a click.
