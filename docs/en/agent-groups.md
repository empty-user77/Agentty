---
title: Agent groups
description: A team of agents with a role each, started together in one project.
---

An agent group opens several agents in one tab of a project, each with a role: one builds, one
reviews, one deploys. Every agent wears its name and role as badges on its terminal, knows the
others' roles, and is linked with them from the start, so what one does the others know.

## Start a group

Open the command palette (**⇧⌘P**) in the project and pick **Start agent group: …**. Only the groups
this project may use are listed. The first agent is the main one: its pane starts enlarged, with the
others working beside it.

In a folder Claude Code has not been told to trust yet, only the main agent starts first. Answer its
folder question there; the others start after it, without asking again.

## How the agents work together

- **Every request is read in the agent's role.** "Deploy e-ticket" said to a deploy agent means: do
  the deployment work for e-ticket, the way this project deploys.
- **Work in another agent's role is handed to it.** The agent runs `agentty group send "<agent>"
  "<request>"`; the request arrives in that agent's terminal (after its current turn, if it is
  busy), where you can follow it.
- **Work in nobody's role is not done.** The agent tells you which roles the group has.
- **Shared sessions.** Claude Code agents are introduced to each other and message each other
  directly with Claude Code's own session messaging; other agents get live
  [Session Flow](session-flow) links both ways.

`agentty group list` shows an agent the members, their roles and whether they are working.

## A group is a package

A group is one JSON file. Share the file, and whoever gets it picks **Import agent group…** in the
command palette and can start it right away. Installed groups are in `~/.agentty/agent-groups/`
(**Open agent groups folder** in the palette); edit a file there to change a group, or copy one to
make another. A sample group, *Dev team*, is there to start from.

```json
{
  "name": "E-ticket team",
  "description": "Build and ship the e-ticket service",
  "scope": "acme/eticket",
  "notes": "Deploy with ./scripts/deploy.sh <env>. Staging first.",
  "agents": [
    { "name": "Dev", "role": "Implements features and fixes bugs", "badge": "Build", "agent": "claude" },
    { "name": "Deploy", "role": "Builds and deploys to staging and production", "badge": "Release",
      "agent": "claude", "instructions": "Never deploy to production without the user's OK." }
  ]
}
```

| Field | Meaning |
|---|---|
| `name` | The group's name, shown in the palette. |
| `description` | One line about the group. |
| `scope` | `"any"` for every project, or `"owner/repo"`: only in the project whose `origin` remote is that repository. |
| `notes` | What every agent should know: how to build and deploy, where things are. |
| `agents` | 1 to 6 agents; the first is the main one. |
| `agents[].name` | Shown on the pane and used to hand work over. Names are distinct. |
| `agents[].role` | One line: what this agent does. |
| `agents[].badge` | A word or two for the role badge (up to 16 characters); the start of `role` when missing. |
| `agents[].agent` | `claude` (default) or `codex`. |
| `agents[].instructions` | Extra rules for this agent. |

A group's roles, notes and instructions become instructions to your agents. Before you import a
group someone shared, read it as you would a script: it tells the agents what to do in your project.

## Good to know

- Group badges and links last as long as the app runs; after a restart the agents' tabs come back
  without them. Start the group again for a fresh team.
- Closing an agent's pane ends its part: handing work to it says it is closed.
