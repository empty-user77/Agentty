# Launch for Agentty

Takes a project from a folder on your computer to a live URL: **GitHub login → save the project to a
GitHub repository → Vercel login → database (Supabase, when the project uses one) → environment
variables → publish**. One plain-language step at a time, one primary button, no manual token
copying.

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
6. **Database** — only for projects that use Supabase (the `@supabase/*` client, a `supabase/`
   folder or `…SUPABASE…` env names), or after pressing **Connect a database (Supabase)**. See
   [Database](#database-supabase) below.
7. **Environment variables** — if `.env`/`.env.local`/`.env.production` exist, their *names* (never
   values) are listed with a toggle to add each one to the live site. Values that only make sense
   on your computer (localhost addresses, the database password) start switched off.
8. **Publish** — `vercel deploy --prod --yes`, with a small live log, then best-effort connects the
   GitHub repo to Vercel so future pushes deploy automatically.
9. **Launched!** — the public URL, with buttons to open it, copy the link, update the site, or open
   the GitHub repo. Remembered per project, so reopening Launch goes straight back here.

Any step that fails shows a short message and two buttons: **Try again**, and **Ask the agent to fix
it** (sends the failing command and its last output to an agent via `prompt/inject`).

## Database (Supabase)

1. **Tool** — the Supabase CLI, from your `PATH` or installed with npm into Launch's data folder.
2. **Login** — `supabase login` shows a verification code in the browser that has to be typed back,
   so the panel opens the login link and has a field for the code (the CLI runs under a
   pseudo-terminal; it refuses this flow without one). The access token stays in the CLI's own
   store.
3. **Project** — pick one of your projects, or **Create a new project**: named after the folder, in
   the region nearest your time zone, with a generated database password.
4. **Keys** — the project URL and the *public* key (`anon`, or a `publishable` key) go to
   `.env.local` (created `0600`, gitignored first) under the names the project already uses, else
   `NEXT_PUBLIC_SUPABASE_URL` / `NEXT_PUBLIC_SUPABASE_ANON_KEY` (`VITE_…`, `PUBLIC_…` by framework);
   `.env.example` gets the names without values. If `.env.local` points at a local Supabase, the
   hosted values go to `.env.production` instead. **The `service_role` / secret keys are never
   requested, read or written.**
5. **Database changes** — `supabase/migrations/*.sql` not applied yet are pushed with `supabase link`
   + `supabase db push`, on request and before **Update site**. The database password is
   `SUPABASE_DB_PASSWORD` in `.env.local` only — generated for projects Launch created, asked for
   once otherwise — and reaches the CLI through the environment. It is never offered to Vercel by
   default, never shown in the panel and never stored in Launch's own state.
6. **Ask the agent to use the database** — sends a prompt asking for a small supabase-js client,
   migrations with row level security on every table, and no service_role key.

## Permissions

`prompt.inject` (the Vercel-login terminal fallback, "Ask the agent to fix it" and "Ask the agent to
use the database"), `workspace.read` (the focused pane's folder). Launch never reads or shows any
token — GitHub, Vercel and Supabase sessions live in each CLI's own credential store, not in Agentty.

## Data

`<dataDir>/tools/` — the installed `gh`, `vercel` and `supabase` CLIs (if Launch installed them).
`<dataDir>/projects.json` — per-project state: the repo URL, the last deploy URL and time, whether
environment variables were already added, and for Supabase the project ref and the names of the
migrations already applied. No keys, no passwords. Keyed by the project's absolute path.
