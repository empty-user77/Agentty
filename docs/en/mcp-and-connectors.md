---
title: Extensions, MCP and connectors
description: Skills, subagents, commands, MCP servers and your own HTTP APIs — in one place, with the secrets kept by the operating system.
---

**⇧⌘X** opens Extensions: the skills, subagents, commands, plugins and MCP servers of Claude Code and Codex, in one list, with the live connection state of each server.

From there you can add an official MCP server in one click, or insert and run any of them in the active agent. The status bar of an agent pane shows the skills, subagents and MCP servers that agent actually has.

## MCP servers

Agentty registers servers with the agent CLIs rather than proxying them: the server is added to the project's or the agent's own configuration, and the agent talks to it directly.

When a value in a server's URL or arguments looks like a secret, it is shown redacted.

## API connectors

A connector turns any HTTP API into an MCP server your agents can use. You describe the endpoints once — method, path, what the parameters mean — and the agent gets tools it can call.

- Definitions live in `~/.agentty/connectors.json`.
- **Secrets never go in that file.** They are stored in the operating system's credential store: Keychain on macOS, Credential Manager on Windows, Secret Service on Linux. Where none exists, a private file readable only by your account is used.
- Values that may contain a secret are redacted wherever they are displayed.

## Agent sign-in

**Settings → Accounts** decides how new agent tabs sign in, for machines where the CLI's own login can't be used:

| Agent | Methods |
|---|---|
| Claude Code | API key, gateway token, `claude setup-token` OAuth token, Amazon Bedrock, Google Vertex AI |
| Codex | API key, or an imported `auth.json` |

The method and non-secret values are saved in `~/.agentty/agent-auth.json`; keys and tokens go to the credential store. **Test** checks a key against the provider's own endpoint.

Credentials reach only the agent process. The shell that remains after the agent exits does not keep them, and variables belonging to other sign-in methods are removed for the agent, so a key exported in your shell profile cannot override the method you chose.

If a saved method can't be used — the key was removed from the credential store, say — the pane says so and the agent falls back to its own login.

## Agent harnesses

If a project declares a harness, entering it offers **Start with harness**: pick an entry point such as `/implement`, paste a ticket key or link, and the agent starts with it.

A project counts as a harness when it has a `.harness` file, `harness.json` / `.yaml` / `HARNESS.md`, a project skill named `harness` or `harness-*`, a `harness` list in `agentty.json`, or a pattern you added in **Settings → Project**.

A project can name its own entry points:

```json
{
  "harness": [
    { "label": "Start a ticket", "command": "/implement", "input": "Ticket key or URL" },
    { "label": "Fix a bug", "prompt": "Reproduce and fix {input}", "agent": "codex" }
  ]
}
```

`command` gets the input appended; in a `prompt`, `{input}` is replaced by it.
