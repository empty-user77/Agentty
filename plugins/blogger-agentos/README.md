# Blogger AgentOS

The skeleton of an **AgentOS**: a plugin that runs work through Agentty's agents instead of doing
it itself. Three steps — outline, draft, edit — each one asked of a Claude Code session you can
read and take over, each one checked before the next is offered.

It is a skeleton on purpose. A real AgentOS for a trade is mostly its prompts, and those are
written by whoever knows the trade; what is here is the machine that walks them, and it is about
250 lines.

## What it shows

| | |
|---|---|
| A step | `prompt/inject` sends the step's skill to the session and remembers its pane id |
| Waiting | `pane/status` says when that agent stopped working; `host/timer` looks again if it stays quiet |
| Reading | `session/get` takes the last thing the agent wrote |
| A rule | the step checks it (three sections, long enough, no `TODO` left) before calling it done |
| Keeping it | `storage/*` holds the run, so closing Agentty does not lose it |

The user stays in front of it: nothing moves to the next step until they press the button, and
every step runs in a session they can open, read and take over.

## Build and install

```sh
./build.sh
```

Then in Agentty: **Plugins → Install from Folder…** and pick this folder.

## What a real one would add

Its own prompts, written for the trade. Rules that mean something in it. Probably a place to edit
them (`storage/*` keeps an edited prompt), and a way to publish the result (`net.request`, if it
asks for it). `docs/plugins/agentos.md` in the Agentty repository is the design.
