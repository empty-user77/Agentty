---
title: Chat view
description: Show an agent's pane as a desktop-style chat while the CLI keeps running underneath.
---

Chat view draws a Claude Code, Codex, Antigravity or Gemini CLI pane as a chat, the way a desktop chat app does. The CLI keeps running in the pane: switching between chat and terminal keeps the session and everything in it.

## Turning it on

- Click the speech bubble in the pane's title bar, choose **View → Chat View**, or press **⇧⌘J**. Each switches only the pane in front. Each pane keeps its view, chat or terminal, across a restart.
- **Settings → AI settings → How agents open** picks the mode new agents start in (terminal or chat). You also pick it in onboarding. An agent started again in a pane, after the last one quit, opens in this mode.

## The conversation

- Replies are formatted, and code blocks, commands and file reads use the editor's syntax colors.
- Tool steps fold into cards that open downwards; to-do lists show as checklist rows.
- Approvals and questions with numbered options can be answered from the chat. When the agent shows its own menu or a setup screen, its terminal comes up until that is done.
- Select text across paragraphs and code and copy it with **⌘C**. The bar over a selection (or a right-click) quotes it in a reply or starts a side chat with a question about it.
- Messages from other sessions show as a card saying who sent them. One waiting for your approval shows what it says, and you deliver or deny it from the chat; what you send meanwhile goes in afterwards.
- Tables are drawn as tables. Links open: a web address in your browser, a file in the editor at its line.
- Attach files with **+** or by dragging them in; they show as thumbnails and chips.
- **Stop** interrupts the running turn. A message the agent puts back after a stop returns to the composer.

## Skills and background work

Type `/` in the composer to see the skills and commands the agent has. Keep typing to narrow the list, move with the arrow keys, and press Enter or Tab, or click, to pick one. It goes into the composer with room for more words and is not sent: press Enter or the send button when the message is ready. Esc closes the list.

When the agent leaves work running in the background, a line over the composer names it until it finishes, so a quiet chat isn't mistaken for a finished one.

## The start screen

A new session shows the agent and model, the folder (opens the files panel), the branch (opens Git), a few ways to begin that fill the composer without sending, and the agent's earlier sessions in this folder, picked up again with a click.
