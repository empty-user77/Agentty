# Agentty — rules for AI assistants and contributors

## Secrets (highest priority, no exceptions)

1. **Never put a real credential in the repository** — not in code, tests, fixtures, docs, comments, commit messages or
   screenshots. This includes API keys, tokens, passwords, private keys, cookies and session IDs.
2. **Never copy values from the user's environment.** `~/.claude.json`, `~/.codex/config.toml`, `.env*`, the Keychain,
   MCP server arguments, shell history and terminal output may contain real credentials. If you see one, do not repeat
   it in code, tests or messages — refer to it as "the Figma token in ~/.claude.json" instead.
3. **Test fixtures use obviously fake values** that contain a marker such as `example`, `not_a_real`, `fake`, `dummy`
   or `placeholder` — e.g. `figd_example_not_a_real_key`, `ghp_example_not_a_real_token`. Build credential-shaped
   strings at runtime if a test needs a realistic length.
4. **Do not print secrets** in commands you run (no `cat ~/.claude.json`, `env`, `security find-generic-password -w`,
   unmasked `claude mcp list` output). Redact before displaying (`sed -E 's/figd_[A-Za-z0-9_-]+/figd_***/g'`).
5. **Never bypass the checks**: no `git commit --no-verify` / `-n`, no changing `core.hooksPath`, no editing or
   deleting `.githooks/`, `scripts/check-secrets.py` or `.claude/hooks/`. If the scanner flags something, remove the
   value; if it is a false positive, make the value obviously fake — do not weaken the scanner without the user's OK.
6. **If a secret is ever committed**: stop, tell the user immediately which file/commit, recommend revoking the
   credential, and ask before rewriting history or force-pushing.

App code follows the same principle: secrets live in the macOS Keychain (`agentty-bridge/src/connectors.rs`), UI shows
redacted values (`redact_args`, `redact_url`), and files with conversation data are created `0600`.

## Enforcement (already installed)

| Layer | What it does |
|---|---|
| `.githooks/pre-commit` | `scripts/check-secrets.py --staged` blocks commits that add credentials |
| `.githooks/pre-push` | scans every commit being pushed (catches `--no-verify` commits) |
| `.claude/hooks/guard-secrets.py` | Claude Code PreToolUse hook: blocks writing credentials, credential literals in shell commands, and hook bypasses |
| `.claude/settings.json` | registers the hook; denies reading `.env.agentty-prod` and `--no-verify` commits |
| CI (`.github/workflows/ci.yml`) | `check-secrets.py --all` on every push and pull request |

After cloning run `scripts/install-hooks.sh` once (sets `core.hooksPath=.githooks`).

## Git

- Commit or push only when the user explicitly asks. Conventional commit messages (`feat:`, `fix:`, …).
- Never force-push or rewrite history without explicit approval.

## Project conventions

- Rust workspace: `crates/agentty-app` (GPUI UI) and `crates/agentty-bridge` (sessions, usage, git, connectors).
- The Rust toolchain is pinned in `rust-toolchain.toml` (used locally and in CI). Never work with an older local toolchain.
- Before handing work back or pushing, run exactly what CI runs and make it pass:
  `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`,
  `cargo build --release -p agentty-app`, `python3 scripts/check-secrets.py --all`.
- After pushing, check the CI run (`gh run list` / `gh run watch`) and fix it if it fails.
- Every user-facing string goes through `i18n.rs` in all four languages (en / ko / ja / zh).
- Reply to the user in Korean.

## Releases

- Releases go to `empty-user77/agentty-releases` following `.claude/skills/release/SKILL.md`.
- **Release notes, release titles and `CHANGELOG.md` are always written in English**, even though conversation with
  the user is in Korean.
- Never publish a draft release (it ships an auto-update to every user) without the user's explicit OK.
- Run `scripts/release-preflight.sh X.Y.Z` before bumping, tagging or pushing; nothing irreversible while it shows ✗.
- Say "built", "notarized" or "released" only after `scripts/verify-release.sh X.Y.Z [--published]` passes. Report
  release status as a ✅/❌/⏳ checklist that leads with what is not done.
- If a release command is denied, don't retry variants or edit permission files: give the user the exact `!` line.
