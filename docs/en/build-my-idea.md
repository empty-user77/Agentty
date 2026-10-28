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

Agentty opens an agent in that folder with the first prompt already prepared, and you watch the result take shape in the [in-app browser](/docs/in-app-browser). **How** picks who builds it:

- **Agent team** (the default when Claude Code or Codex builds it and orchestration is on in Settings): the agent leads an [orchestration](/docs/session-flow). Its team goes from the idea to the code one stage at a time, each stage leaving a document in `docs/design/` for the next:
  1. Idea analysis: the problem, the users, alternatives, risks, and the assumptions it made
  2. Idea derivation: the value, the features ranked, and the scope of the demo
  3. PRD: user stories with acceptance criteria, screens and flows
  4. High-level design: stack, architecture, data model, pages
  5. Low-level design: files, components and interfaces, and the work split into build units
  6. Team composition: the project's harness, and one developer per build unit
  7. Development: the foundation first, the build units side by side, then a quality check in the browser and a review against the PRD

  Inside a stage several agents work side by side (users, market and feasibility in the analysis, for example); the
  stages pass the work on in order. They show as waves on the Orchestration page.
- **One agent**: the agent plans and builds it on its own.

When it is ready to go online, [Launch](/docs/launch) publishes it.
