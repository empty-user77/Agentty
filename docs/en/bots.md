---
title: Bots
description: Named agents that keep their instructions and memory, hand work to each other, start on a schedule and take messages from Telegram, Slack or Discord.
---

A bot is an agent with a name, a purpose and a memory. It runs on your computer as Claude Code or Codex, on the subscription you already use. You talk to it in a chat, other bots hand it work by name, and it can start work by itself at a set time or when a message arrives from Telegram, Slack or Discord. From your phone you see what each bot is doing, switch it on or off, give it work and read its reports.

Bots work while Agentty is open. With Agentty closed nothing runs; a scheduled run missed in the last 24 hours is made up once when you open it again.

## Making a bot

1. In the sidebar, press **+** beside **Bots** (at the foot, under the workspaces).
2. Pick a template, or **Start from scratch**. There are templates for everyday life (Everyday buddy, My computer, Mail assistant, Meeting assistant, Day planner), for looking things up and learning (News briefing, Researcher, Writing helper, Study coach, Shopping compare) and for code (Reviewer, Bug hunter, CI watch). A template fills in the bot's prompt, its rules and its skills, and for some a time or a watch that starts it by itself.
3. Choose its character and colour, check the name, the handle, the agent it runs as and the project folder it works in, and press **Create**.

Each bot has a character. It is shown wherever the bot appears: in the sidebar, in its conversation, in rooms and in the activity. You can change the character and its colour later in the bot's settings, or use a picture of your own there and in **New bot** (PNG, JPEG, WebP or GIF, up to 5 MB).

To do without bots, switch **Use bots** off in the sidebar's options menu: their section goes, and their schedules and channels stop. The bots themselves are kept.

The handle is what you write after `@`: lowercase letters, digits and hyphens. It cannot be changed later.

A bot's row in the sidebar says what it is doing, and opens its chat: what you wrote on the right, what the bot said on the left, and between them what made it start without you (a schedule, a watch, another bot). Write to it in the box at the bottom. The chat is kept in the bot's folder, so it is still there after a restart. The gear at the top shows when the bot starts by itself, what it remembers and its settings; the button beside it opens the agent session the bot works in, an ordinary session shown as a chat with attachments, approvals and the terminal behind it. That session is a workspace of the bot's own, which the list of workspaces leaves out.

## The Bots page

Press the **Bots** bar in the sidebar to fold the section. The button beside it opens the page with every bot, the activity, the rooms and the templates.

- **Give work**: pick a bot at the top of the page, write what it should do and press Enter. A message that starts with `@handle` goes to that bot.
- **Each row** says what the bot is working on, what it waits for, or its last answer, and when it starts by itself next.
- **Activity** lists who handed what to whom, newest first: you, a schedule, a channel, another bot. A row opens the conversation it happened in.

The messages of a bot's chat and of a room select and copy like a plain chat's: drag across them and press **⌘C**, or use the bar over a selection to quote it in a reply. A right click offers copying the whole message.

## What a bot is made of

Open a bot's chat and press the gear at the top. Four cards show what the bot is made of:

| Card | Holds | File |
| --- | --- | --- |
| **Prompt** | What the bot is for and how it should answer | `SOUL.md` |
| **Rules** | What always holds, whoever asks: what it must and must not do | `RULES.md` |
| **Context** | Background it should know: who you are, what you are working on. The bot's own memory is shown with it | `CONTEXT.md`, `MEMORY.md` |
| **Tools** | What it may do (its permission) and the skills it works with | `skills/` |

Press **Write** or **Edit** on a card to change its text there, then **Save**. A change applies from the bot's next conversation: the agent reads these when a conversation starts.

Each bot is a folder of files you can read and edit, in Agentty's data folder under `bots/<handle>/`. Besides the files above it holds `bot.json` (its definition) and `chat.json` (its chat), both written by Agentty.

The bot adds to its memory when it learns something worth keeping. It is given the first 16 KB of its memory; when the file grows past that, Agentty asks the bot to tidy it and keeps the file as it was in `MEMORY.before-tidy.md`.

## A bot that uses your computer

The **My computer** template makes a bot that works this computer for you. It opens and uses apps, reads your messengers (KakaoTalk, Messages, Slack, Telegram) and sends a reply for you, works your iPhone through the Mac's iPhone Mirroring app, finds files and keeps reminders. It starts with the skills **Computer control**, **Messenger** and **iPhone Mirroring**; any bot can be given them in the skills hub.

- **How it works**: the agent looks at the screen (the window's text, or a picture of it), does one step (a click, typing, a shortcut) and looks again. Where the agent has computer-use tools it uses those; otherwise the system's own scripting.
- **What macOS asks for**: the first time, macOS asks you to let Agentty control the computer. Switch Agentty on in System Settings > Privacy & Security > **Accessibility**, and **Screen & System Audio Recording** for pictures of the screen. Without them the bot says what is missing and stops.
- **What it will not do unasked**: it shows you the exact text and the recipient before it sends a message, and waits for your yes each time. It never types a password, a passcode or a payment detail: it asks you to do that step.
- **iPhone**: needs macOS 15 or later and an iPhone that is locked and near the Mac. The camera and the microphone do not work through mirroring.

While the bot works an app, the keyboard and the pointer are its to use: leave them alone until it says it is done.

## Skills hub

A skill is a written method for one kind of work: how to sort an inbox, how to research with sources, how to review a change. Press **Skills hub** on the **Tools** card to give the bot skills or take them away.

- **Included**: 19 skills that ship with Agentty, for controlling the computer, everyday life, work, research, learning and coding.
- **My skills**: the skills your agents already have on this computer (`~/.claude/skills`, `~/.codex/skills`, `~/.agents/skills`). Adding one copies it into the bot's folder; the original is left as it is.

Search by name or pick a kind on the left. The bot is told the names of its skills and reads one before doing the work it covers. A skill says how to work; it never widens what the bot is allowed to do.

## Permission

| Mode | What the bot may do |
| --- | --- |
| **Ask first** | The agent's own approval flow. It asks, you answer. |
| **Read only** | Reads and runs what it needs to judge work. Edits no files. |
| **Files only** | Reads and writes files in its folder. No shell, no web, no other tools. |

A bot is never started with permission prompts skipped, whatever your setting for ordinary tabs says. Only the bot's settings change its permission: no message can.

## Bots working together

A bot hands work to another from its shell:

```sh
agentty bot list
agentty bot send reviewer --prompt "Review the parser branch and list what must change."
agentty bot result reviewer
agentty bot status
```

`send` returns at once. The other bot's answer arrives later in the sender's conversation, as a message that starts with `[Agentty]`. Any agent session in Agentty can use the command, not only bots.

### Rooms

A room is one conversation with several bots. Choose **New room**, name it and pick its members.

- What you write goes to the bot you name with `@handle`, or to the first member.
- A bot's answer that names another member with `@handle` gives that bot the next turn. An answer that names nobody gives the turn back to you.
- The bot taking a turn is shown what was said since its last one.

### Limits

| Limit | Value |
| --- | --- |
| Hand-overs between bots without you writing | 8 |
| Messages waiting for one bot | 32 |
| Bots at work at once | 4 |

A chain that reaches the hand-over limit stops and shows under **Needs you**.

## Watches

A bot can start work when something happens, without being asked. In the bot's settings, under **Watches**, switch on what it should look out for:

- **New issues**, **New pull requests** and **Failed CI runs** of the repository in the bot's folder. These need the GitHub CLI (`gh`), signed in.
- **New commits on the remote**: the remote's default branch moved.
- **New files in its folder**: a file or folder appeared directly in the bot's folder.

Agentty looks every five minutes with a fixed `gh` or `git` command, or by listing the folder. The agent is only started when something is new, so a quiet repository costs nothing. The first look only notes what is already there. If a watch cannot look (no `gh`, not signed in, not a repository), the bot's settings say why.

The Bug hunter, Reviewer and CI watch templates come with their watch switched on.

## Schedules

In a bot's settings, add a time and what to do. Pick **Hourly**, **Daily**, **Weekdays** or **Weekly**, or write a five-field cron expression such as `0 9 * * 1-5`. Runs are at least 15 minutes apart.

## Telegram, Slack and Discord

Your computer connects out to the service and waits for messages. No port is opened and no relay server is involved.

A channel only listens to the senders you list. A message from anyone else is dropped and noted in the bot's `audit.log`. A channel with nobody on its list is off.

### Telegram

1. Create a bot with [@BotFather](https://t.me/BotFather) and copy its token.
2. In the bot's settings, paste the token under **Telegram** and save. It is stored in the system keychain.
3. Add your numeric Telegram user ID under **Allowed senders** and save.

With several bots on Telegram, start a message with `/handle` to choose one.

### Slack

1. Create a Slack app with **Socket Mode** on, an app-level token (`xapp-…`) with `connections:write`, and a bot token (`xoxb-…`) with `app_mentions:read`, `chat:write` and `im:history`. Subscribe to the `app_mention` and `message.im` events.
2. Paste both tokens under **Slack** and save.
3. Add the Slack member IDs (`U…`) that may write under **Allowed senders**.

Mention the app with the bot's handle (`@YourApp reviewer check the PR`), or write to it directly. The answer goes into the thread.

### Discord

1. In the [Discord Developer Portal](https://discord.com/developers/applications), create an application, add a bot to it and copy the bot's token. Invite the bot to your server, or open a direct message with it.
2. Paste the token under **Discord** and save.
3. Add your Discord user ID under **Allowed senders** (turn on Developer Mode in Discord, then **Copy User ID** on your profile).

Write to the bot directly, or mention it in a server (`@YourBot reviewer check the PR`). Only direct messages and messages that mention the bot are read.

### Running the bots from a channel

A sender on a bot's list can also ask Agentty itself, about the bots that list them. Send the word alone, with or without `/` or `!` in front:

| Message | What it does |
| --- | --- |
| `bots` | Every bot and what it is doing |
| `status <bot>` | One bot, and what it last said |
| `on <bot>` / `off <bot>` | Switches it on or off. A bot that is off takes no work, and can be switched on again from the same chat |
| `stop <bot>` | Stops what it is doing |
| `/<bot> <message>` | Gives that bot work. With one bot on the channel, the message alone is enough |
| `help` | This list |

A bot works on this computer with the permission you gave it in Agentty, wherever the message came from. When its agent stops at an approval or a question, the chat is told what it asks, with the options numbered: answer with the number alone (`1`), or `/<bot> 1` when several bots listen. Nothing in a message can widen what the bot may do.

A number is taken only from the chat that was told the question, and only while the screen still shows the options it was told. An option that goes on holding, such as "don't ask again", is not offered in a channel: pick it in Agentty. A number, or `on` / `off` / `stop`, sent while Agentty was not running is not acted on later.

### Reports

A bot's answer goes back to the chat the message came from. What it does by itself (a schedule, a watch, work another bot handed back) is reported to the direct chat you last wrote to it from, when **Report what it does by itself here** is on for that channel. A group chat is never used for reports. Taking a sender off the list forgets where reports went.

## Needs you

The top of the Bots page lists every bot stopped at an approval or a question, and every chain that was stopped. **Answer** opens the session that asks.

## From your phone

The remote page lists the bots under the workspaces, each with its switch: turn a bot on or off there. Tap a bot to open its chat: what was written to it, what it answered and what it reported by itself, and a box to give it work. While it works the chat says what it is doing, and when it waits for an approval or a question, **Answer** takes you to the session that asks. A bot whose conversation is in a workspace left out of remote access is not listed.

Making a bot, its prompt, rules, skills, permission and channels are changed in Agentty only.
