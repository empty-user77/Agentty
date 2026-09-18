# Launch for Agentty

Takes a project from a folder on your computer to a live URL: **GitHub login → save the project to a
GitHub repository → Vercel login → environment variables → publish**. One plain-language step at a
time, one primary button, no manual token copying.

Open it from the rocket button above a pane, the command palette (**Launch: Publish this project to
the web**), or `agentty://plugin/launch/open?path=%2FUsers%2Fme%2Fmy-app`.

## What each step does

1. **Project check** — looks at the focused pane's folder (walking up to the nearest `package.json`
   or `.git`) and names the framework (Next.js, Vite, Astro, a static site, …).
2. **Tools** — needs the GitHub CLI (`gh`) and the Vercel CLI. If neither is already on your `PATH`,
   Launch downloads/installs both into its own data folder — no Homebrew, nothing system-wide.
3. **GitHub login** — runs `gh auth login --web` for you; the one-time code is shown big, copied to
   the clipboard, and the browser opens on its own.
4. **Save to GitHub** — commits everything (adding sensible lines to `.gitignore` first) and either
   creates a new private repository (public is a toggle) or pushes to the existing one. Refuses to
   save if a real `.env` file would be uploaded.
5. **Vercel login** — same idea as GitHub. If Vercel's login can't be driven without a browser
   window, Launch opens a terminal with the login command typed in and lets you finish there.
6. **Environment variables** — if `.env`/`.env.local`/`.env.production` exist, their *names* (never
   values) are listed with a toggle to add each one to the live site.
7. **Publish** — `vercel deploy --prod --yes`, with a small live log, then best-effort connects the
   GitHub repo to Vercel so future pushes deploy automatically.
8. **Launched!** — the public URL, with buttons to open it, copy the link, update the site, or open
   the GitHub repo. Remembered per project, so reopening Launch goes straight back here.

Any step that fails shows a short message and two buttons: **Try again**, and **Ask the agent to fix
it** (sends the failing command and its last output to an agent via `prompt/inject`).

## Permissions

`prompt.inject` (the Vercel-login terminal fallback and "Ask the agent to fix it"), `workspace.read`
(the focused pane's folder). Launch never reads or shows any token — GitHub and Vercel sessions live
in `gh`'s and Vercel's own credential stores, not in Agentty.

## Data

`<dataDir>/tools/` — the installed `gh` and `vercel` CLIs (if Launch installed them).
`<dataDir>/projects.json` — per-project state: the repo URL, the last deploy URL and time, and
whether environment variables were already added. Keyed by the project's absolute path.
