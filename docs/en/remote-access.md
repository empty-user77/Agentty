---
title: Remote access
description: Open your terminals and Agentty workspaces from another device on your Tailscale network.
---

Remote access lets you open your Agentty workspaces from a browser on another device — a phone, tablet, or computer — in your own Tailscale network. You can watch terminal sessions, type commands directly, and send prompts to agents, all over the local network inside your Tailscale account.

## What you see

The page is laid out like Agentty itself:

- **Home** — where the page opens: how many terminals wait for you and how many are working, the ones waiting with what they ask, the agents at work, every workspace (asleep ones included), and your plugins with their state. Tap a terminal or a workspace to go there. **Home** at the top of the ☰ menu brings you back.
- **☰ menu** — your groups and workspaces, in the sidebar's order, with their colors, branch and folder. A red number shows how many terminals there wait for your answer. On a wide screen the list stays open on the left.
- **Tabs** — the chosen workspace's tabs across the top. A dot shows a tab that is working or waiting for you; ▥2 marks a tab split into two panes.
- **Split panes** — when the tab is split, the button at the top right opens the list of its panes, with each one's state and what it asks; tap one to switch to it.
- **The terminal** — the chosen pane's live screen in the middle.
- **Chat workspaces** — a [chat workspace](/docs/chat-workspace) opens as its chat: the conversation, plan usage and a box to write to its lead; **Terminal** switches to the lead's terminal.

**New tabs** — **+** at the end of the tabs (or beside a workspace on Home) opens a new tab in that workspace with a terminal, Claude Code or Codex, started in the workspace's folder. The page switches to it.

**Sleeping workspaces** — a workspace that is asleep in Agentty shows **Wake up**: its saved tabs start again, and the page shows them. Neither changes what your Mac's screen shows.

**Plugins** — Home lists your installed plugins with their state (running, stopped, stopped with an error) and what each automation is doing right now. The page only shows them: plugin workspaces don't open on the page, and plugins can't be started or stopped from it.

You can type into the terminal directly using an on-screen key bar with Esc, Tab, Shift+Tab, arrow keys, Enter, Ctrl+C, a sticky Ctrl key, and quick buttons (1/2/3) when an agent is asking for an answer. Or send a prompt from the box at the bottom — press Enter on a computer, or Shift+Enter to add a line.

**Notify me** on the page switches on browser notifications when a session finishes or asks something while you are looking elsewhere.

While you look at a terminal on the page, it takes the page's size: as many columns and lines as fit your screen, so nothing wraps or scrolls sideways. **A−** and **A+** change the text size (kept on that device). When no page shows the terminal any more, it goes back to its size in the app. The page can be added to your home screen as an app.

With Slack, Discord or Telegram notifications set up (Settings → Notifications), each message also carries **Open now: <address>** while remote access is on. The address opens the page at that very terminal (after Tailscale and the web password, as always). It is sent only with **Include what the agent asks** on, since the address names your Mac and your tailnet.

## Requirements

You need:

- **Tailscale** installed and connected on your Mac (and on the other device, signed in to the same Tailscale account)
- **MagicDNS and HTTPS certificates** enabled for your tailnet (Tailscale admin console → DNS)
- **Tailscale Serve** allowed for your tailnet — if not, Agentty shows a button to turn it on

## Setup

1. Click **Remote access** (the globe-and-monitor icon) in the side menu
2. Set a **Web password** (at least 8 characters, entered twice)
3. Turn on **Remote access**
4. Copy the address that appears (`https://<machine>.<tailnet>.ts.net:8743/`), or tap **Open** to go there directly
5. On the other device, sign in with your web password

That's it. Your sessions are now visible on that device.

## Security

Remote access is only reachable inside your Tailscale network — never on the public internet. Agentty refuses to run if Funnel is turned on for its port. Getting in takes both of these:

1. **Your Tailscale account** — only the account that owns this Mac is let in. Other people who share your tailnet and tagged devices are refused.
2. **The web password** — the one you set on the Remote access page.

Someone else's Tailscale account never gets in, even inside your tailnet. Whoever does get past both checks — signed in to *your* Tailscale account and knowing the web password — can type into your terminals, which means running commands on your Mac. So keep the web password to yourself, don't use it anywhere else, and turn Remote access off when you don't need it.

Only Tailscale can reach Agentty's page at all. Agentty listens where only Tailscale's `serve` gets in — a private socket where Tailscale can open one, otherwise a local port behind a secret path that only Tailscale is told — so another program or another user account on your Mac can't reach it directly. The page runs only while Tailscale does: when Tailscale disconnects, signs out or quits, Agentty takes the page down within seconds, and brings it back once Tailscale is connected again.

Failed sign-in attempts are rate-limited: 5 wrong passwords lock the page for 1 minute. Each wrong password after a lock doubles the wait time, up to 1 hour. You get a desktop notification when a device signs in and when the page is locked. If the page is locked and it wasn't you, **Unlock** on the Remote access page lifts it.

The password is stored only as a hash in your system keychain. Sign-ins last up to 7 days (12 hours idle). Quitting Agentty, changing your password, or tapping **Sign out everywhere** signs every device out at once.

## Keep awake

While Remote access is on, Agentty keeps your Mac from idle sleep so it stays reachable. The display may still sleep. You can turn this off if you prefer.

A sleeping Mac or a closed laptop cannot be reached.

Remote access stays on across restarts until you turn it off. It uses Tailscale port 8743 and does not interfere with anything else you serve with Tailscale. If something else is already using port 8743, Agentty shows an error instead.

## Voice input on the Mac

You can also talk to your agents right in the app. Click the **microphone** at the right end of the status bar (or press **⇧⌘M**) and speak; the status bar shows **Recording 0:05** with a level meter while it listens.

- **⏎ Enter** — stop and send what you said to the terminal straight away, as a prompt.
- **Click the microphone or ⇧⌘M again** — stop and only type the words in, to check or fix them before you send.
- **Esc** — throw the recording away.

The words go to the terminal that was active when you started. The language is detected from your speech, so Korean, English and mixed prompts all work. The first time, macOS asks for microphone access; if you turned it off, Agentty opens the Microphone page of System Settings for you. Without a model yet, the microphone opens the model setup right there (the same one as below).

## Voice prompts

From the remote page you can speak a prompt instead of typing it. Tap the microphone in the prompt bar, say your prompt, then tap again to stop; the words are transcribed and dropped into the prompt box for you to check and send.

The audio is transcribed **on your Mac** by a local model — nothing is sent to any outside service. Install a model once from **Settings → Voice input** (the same card is also on the Remote access page): **Large-v3 Turbo** (recommended, about 574 MB) or the lighter **Small** (about 488 MB). It downloads to your Mac, is verified and kept there, and runs on the Mac's GPU. In the same place you can choose the spoken language (Automatic, Korean, English — a short prompt is sometimes mistaken for another language when detected automatically), pick which installed model is used, and delete a model. Once a model is installed, the microphone button appears on the remote page by itself.

Your browser asks for microphone permission the first time, and the page must be served over HTTPS — it is, through Tailscale.

## Hiding the feature

Don't use remote access at all? Turn off **Settings → General → Remote access feature** (on by default), or press **Disable remote access** on the Remote access page. Its icon leaves the side menu and remote access turns off. Turn the setting back on to bring it back.

To keep one workspace off the page, open its menu (right-click its card) and choose **Hide from remote access**. The page no longer lists it or reaches its terminals.

## Troubleshooting

**Tailscale is not connected**
Connect in the Tailscale app on your Mac.

**HTTPS certificates are off**
Enable MagicDNS and HTTPS in your Tailscale admin console (DNS settings). This creates certificates for your tailnet domain.

**Serve is off**
Agentty shows a button in Settings to turn on Serve. Tap it to open the Tailscale admin console.

**This Tailscale account may not use this Agentty** (shown on the sign-in page)
The other device is signed in to a different Tailscale account. Sign in to the same account as your Mac.

**Page says locked** (shown instead of the sign-in form)
Either you entered the wrong password 5 times, or someone else did. Wait, or check **Remote access → Recent activity** to see what happened. If it wasn't you, press **Unlock** on the Remote access page on your Mac.
