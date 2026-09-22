---
name: security-audit
description: Security and logic audit of what is about to be committed or pushed in Agentty — flow correctness, GitHub/Vercel/Supabase integration safety, object-level authorization on the app's local interfaces, leak paths for keys, tokens, personal data and local files, and dev/prod separation. Run it before every `git commit` and `git push` (a hook refuses both until the staged tree was reviewed), and whenever the user asks for a security review or audit.
---

# Security audit

Two halves. `scripts/security-audit.py` and `scripts/check-secrets.py` are the mechanical half and
run on their own from the git hooks and CI. This skill is the half that needs judgment: read the
change and decide whether it is safe. The hook `.claude/hooks/require-security-audit.py` refuses
`git commit` and `git push` until step 5 recorded the tree you reviewed.

## Rules

- Never print, copy or paraphrase a real credential. Name where it is ("the token in
  `~/.claude.json`"), never what it is. Do not `cat` env files, the Keychain or `~/.claude.json`.
- Never weaken a guard to get a commit through: no `--no-verify`, no edits that loosen
  `check-secrets.py`, `security-audit.py`, `.githooks/` or `.claude/hooks/`. A false positive is
  fixed by making the value obviously fake, or by an `audit: ok — <reason>` comment on that line.
- A finding you cannot fix now is reported to the user before the commit, not after.
- Review what will actually be committed: stage first, in its own command. `git commit -a` and
  `git add … && git commit …` are refused, because the tree would differ from the one reviewed.

## Steps

1. **Scope.** `git diff --cached --stat`, then read the staged diff (`git diff --cached`). For a push,
   `git log --oneline @{u}..HEAD` and the diff of that range. Read whole functions, not only hunks,
   when a hunk touches one of the sensitive areas below.
2. **Mechanical checks.** Run both and fix what they block:
   `python3 scripts/check-secrets.py --staged` and `python3 scripts/security-audit.py --staged`.
3. **Review** the change against the seven areas. Skip an area only when the diff cannot touch it
   (a docs-only change still gets area 4).
4. **Report** findings as: severity (critical / high / medium / low) · `file:line` · scenario
   (precondition → action → impact) · verified or inferred · minimal fix. Fix critical and high
   findings before committing, or stop and ask the user. Say plainly when nothing was found and what
   you did not verify.
5. **Record** the reviewed tree — only after steps 1–4 are really done:
   `python3 scripts/security-audit.py --mark`. Staging anything afterwards means reviewing again.

## The seven areas

### 1. Logic and flow
- Every state machine reaches an end from every state: failure, retry, cancel, panel closed and
  reopened, app restarted mid-step. "Try again" must repeat the whole action, not half of it.
- The plugin SDK and the UI dispatch events concurrently. A second click while a step runs must not
  repeat a costly or irreversible action (create a repository, a Supabase project, a deploy).
- State read from disk (`projects.json`, settings, layout) is refreshed after it is written; nothing
  decides from a stale copy.
- User-controlled text (idea title, folder and file names, session titles, prompts) reaching a
  command line goes through `shell_quote` or an argv array — never string concatenation into `-c`.
  Remember panes run `zsh -l -i -c`: history expansion, newlines and leading dashes matter.

### 2. Integration security: GitHub, Vercel, Supabase, agent CLIs
- Secrets travel on stdin or in the environment, not in argv, not in error messages, not in prompts
  sent to agents. CLI output shown in a panel or forwarded to an agent has known secrets masked.
- Launch writes only the public Supabase key (anon / publishable). `service_role`, `sb_secret_`,
  `--reveal` and database passwords never reach the panel, `projects.json`, Vercel by default, git.
- Before `git add -A` in a user's project: `.gitignore` covers env files, keys, local tool state;
  tracked or staged env files stop the save. New kinds of local files get a gitignore line first.
- What is published is what the owner expects: a public repository or a static deploy must not
  expose `docs/idea/**`, attachments, `.claude/**` or env files.
- Downloads (CLI binaries, npm packages, ECC) are pinned or verified where possible, fetched over
  TLS, installed inside the plugin's data folder or the project — never into the home folder, never
  executed from a world-writable place. Third-party text (ECC skills, web pages, repository files)
  is data for an agent, not instructions for Agentty.
- Agentty never starts an agent with permission checks off, and templates for users' projects keep
  allow rules narrow (no `curl`, `rm`, `sudo`, `git push`, `ssh`, `open`, `osascript`).

### 3. Authorization on local interfaces (BOLA)
Who may act on which object? For every handler that takes an id or a path from outside:
- The signal socket (`agent_signal.rs`): which commands exist, is the sender tied to the pane id it
  names, can another local process spoof status, inject text, open URLs or drive the browser?
- Plugin RPC (`plugins/mod.rs`, `workbench/plugin_host.rs`): the permission is checked on the host
  side for every method, and the pane / workspace / session / plugin id in the request belongs to
  what that plugin was given. No method reads another plugin's data folder.
- `agentty://` links and anything a web page can trigger: never auto-submit, never install, never
  turn a query value into a path, a command argument or a prompt without the user confirming.
- The in-app browser: no bridge from page JavaScript into the app; browser control is reachable only
  from the agent it was given to.
- The debug driver stays unreachable without `AGENTTY_DEBUG=1` in the app's own environment.

### 4. Leak paths
- Into git: real credentials, env files, signing material, transcripts, a real home folder path,
  personal email addresses, customer or teammate names, screenshots with any of these.
- Out of the app: analytics carry only allow-listed, non-identifying properties (`metrics.rs`);
  update checks and other requests send no project, path, prompt or user data; logs never contain
  secrets or prompts.
- On disk: files with conversation data, prompts or credentials are `0600`, their folders `0700`;
  nothing sensitive in world-readable temp files; redaction (`redact_args`, `redact_url`) covers the
  argument shapes in use.
- Through agents: a prompt or tool rule that lets an agent read outside the project (`cat`, `cp`,
  `node`, browser tools) plus any outbound channel is an exfiltration path — weigh new allow rules
  and new third-party instruction sources together.

### 5. Dev / prod separation
- All app state goes through `fsutil::data_dir()` (`AGENTTY_DATA_DIR` or `~/.agentty`); no new
  hard-coded `~/.agentty`, no state in shared temp folders keyed by a fixed name.
- Anything global by nature (Keychain service names, URL scheme, sockets, `~/AgenttyProjects`, CLI
  logins) is either namespaced by the data dir or deliberately shared — say which, in the code.
- Production-only values (analytics ids, signing identity, notarization credentials) enter at build
  time from the environment or `.env.agentty-prod`; they are never committed, never defaulted in
  code, never required by a dev build. Inherited `AGENTTY_*` variables are cleared at startup so a
  dev build started from a production pane does not talk to it.

### 6. Everything else
Dependencies added or upgraded (why, from where, pinned?), new network endpoints, new permissions
or entitlements, new files created in users' projects, error messages that reveal paths or
internals to a web page, denial of service through unbounded reads (file sizes, line lengths, list
lengths from a plugin or a transcript), and tests that would hide a regression in any of the above.

### 7. The repository is going to be public
This repository is private today and open source later, and **its whole history goes with it** — a
credential committed and deleted is still there, and so is every commit message, branch name and
pull request body. Judge each change as if it were already public:

- Would this line be fine on a page anyone can read? A pasted log, a stack trace, a terminal dump
  or a screenshot carries absolute paths, machine names and sometimes an environment.
- Does it name something private — another project of the maintainer's, an internal service, a
  repository nobody else can see?
- Does it assume the repository is private? `.github/workflows/ci.yml` says so in a comment, and a
  self-hosted runner reachable from `pull_request` is the thing that assumption protects: once the
  repository is public, a fork's pull request runs its own code on the maintainer's machine.
  `security-audit.py` warns about this (SA05) and blocks it outright if such a workflow also holds
  secrets. The guard is
  `if: github.event.pull_request.head.repo.full_name == github.repository` on the job.
- A commit message and a pull request body are as public as the code. Nothing goes in one that
  would not go in a file.

## Where the sensitive code lives

| Area | Files |
|---|---|
| Agent command lines, hooks injected into agents | `crates/agentty-app/src/launch.rs`, `shell_integration.rs` |
| Idea projects: template, prompt, permissions, harness download | `crates/agentty-bridge/src/idea.rs`, `crates/agentty-app/src/workbench/idea.rs` |
| Local socket and its commands | `crates/agentty-app/src/agent_signal.rs`, `workbench/browser_control.rs`, `browser_mcp.rs`, `browser_cli.rs` |
| Plugins: install, permissions, RPC, links, UI tree | `crates/agentty-bridge/src/plugins/`, `crates/agentty-app/src/plugins/`, `workbench/plugin_host.rs`, `workbench/plugin_panel.rs` |
| Publishing: GitHub, Vercel, Supabase | `plugins/launch/` |
| Secrets at rest, redaction | `crates/agentty-bridge/src/connectors.rs` |
| Analytics, updates, network | `crates/agentty-bridge/src/metrics.rs`, `update.rs`, `http.rs` |
| Data locations | `crates/agentty-bridge/src/fsutil.rs`, every `data_dir()` caller |
| Release credentials | `scripts/build-dmg.sh`, `scripts/release-preflight.sh`, `.github/workflows/` |
