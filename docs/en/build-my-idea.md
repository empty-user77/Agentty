---
title: Build my idea
description: Turn a few sentences and some attachments into a project folder an agent can start building.
---

"Build my idea" is for the point before a project exists. You describe what you want in a short chat, attach whatever you have, and Agentty creates a folder the agent can start from.

Open it from the start page, the **+** menu, or the command palette. Turn the offer off in **Settings → General**.

## What it creates

A new folder (by default under `~/AgenttyProjects`) containing:

| Path | What |
|---|---|
| `docs/idea/IDEA.md` | What you wrote, in order |
| `docs/idea/attachments/` | Copies of the files you attached |
| `docs/idea/BUILD_GUIDE.md` | How to work: plan first, then build, with a live preview |
| `.claude/settings.json` | Permissions so the agent can install packages and run the dev server without asking about every command |

Everything else is left empty, so project scaffolders (`create-next-app`, `create vite`, …) still accept the folder.

Attachments are limited to 20 files and 20 MB each.

## Then what

Agentty opens an agent in that folder with the first prompt already prepared. From there it is an ordinary project: the agent plans, builds, and you watch the result in the [in-app browser](/docs/in-app-browser).

When it is ready to go online, [Launch](/docs/launch) publishes it.
