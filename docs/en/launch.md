---
title: Launch
description: Take a project from a folder on your computer to a live URL — GitHub, Vercel and, when needed, Supabase.
---

Launch is a bundled plugin that publishes a project: **GitHub sign-in → save to a repository → Vercel sign-in → database if the project uses one → environment variables → publish.** One step at a time, one primary button, no copying tokens by hand.

Open it from the rocket button above a pane, or the command palette (**Launch: Publish this project to the web**).

## The steps

1. **Project check** — looks at the focused pane's folder and names the framework (Next.js, Vite, Astro, a static site, …).
2. **Tools** — needs the GitHub CLI and the Vercel CLI. If neither is on your `PATH`, Launch installs both into its own data folder — nothing system-wide.
3. **GitHub sign-in** — skipped when the GitHub CLI is already signed in, or when this computer has an SSH key that can push to the repository. Otherwise it runs the CLI's browser login for you; the one-time code is shown and copied. A login is only asked for when a repository has to be created.
4. **Save to GitHub** — commits everything and creates a new private repository. Public is a toggle.
5. **Vercel sign-in** — same idea.
6. **Database** — only for projects that use Supabase, or after you press **Connect a database**.
7. **Environment variables** — if `.env` files exist, their *names* (never values) are listed with a toggle for each. Values that only make sense locally start switched off.
8. **Publish** — deploys to production, with a live log, and connects the repository to Vercel so future pushes deploy by themselves.

## When the folder is not a web project

Launch opens on a **dashboard** instead: which command-line tools are installed, which of GitHub and Vercel you are signed in to, and every project on your Vercel account with the repository it deploys from. Both sign-ins can be done from there, before there is anything to publish. The dashboard and the publishing steps are a tab apart; the folder of the terminal you are looking at decides which one opens.

## What it refuses to do

- It adds env files, key files and local tool state to `.gitignore` before the first commit, and **refuses to save if a real env file or key file is tracked or staged**.
- A project that already has a remote shows where it points and asks once before anything is pushed there.
- It writes `.vercelignore` first, so idea notes, `.claude/`, migrations and env files are never uploaded.

## Whose accounts

Your own. Launch signs you in through each service's official tool and its own sign-in page; the tokens stay with those tools on your machine. What you publish goes to GitHub, Vercel and Supabase under their terms, and none of it reaches us.

Any step that fails shows a short message with **Try again** and an option to hand the problem to the agent.
