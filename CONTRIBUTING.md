# Contributing to Agentty

Thanks for helping! This guide covers how to build, test and submit changes.

## Getting started

```sh
cargo run -p agentty-app          # run the app (debug)
cargo test --workspace            # unit and integration tests
cargo fmt --all                   # format
cargo clippy --workspace --all-targets -- -D warnings
```

Requirements: macOS 13+, Rust 1.98 (pinned in `rust-toolchain.toml`) and Xcode command line tools. GPUI compiles its Metal shaders at runtime
(`runtime_shaders` feature), so the separate Metal toolchain is not required.

## Project layout

```
crates/
  agentty-app/        GUI application (GPUI)
    src/terminal/     PTY backend, key encoding, grid renderer
    src/workbench/    workspaces, tabs, split panes, sidebar, pages
    src/usage_view.rs AI usage dashboard
    src/i18n.rs       UI strings (en, ko, ja, zh)
  agentty-bridge/     Claude Code / Codex transcript reader, usage accounting, handoffs (library + CLI)
docs/                 architecture and configuration docs
```

## Guidelines

- **Keep it native and fast.** Avoid work on the UI thread that scales with transcript size; use background tasks.
- **Never modify users' agent configuration.** Integrations must be opt-in and reversible (e.g. `--settings` flags).
- **Translate every user-facing string.** Add a key to `crates/agentty-app/src/i18n.rs` with all four languages; the
  test suite fails if a translation is empty.
- **Tests.** Pure logic (layout trees, parsers, formatting) should have unit tests next to the code.
- **Commits** follow [Conventional Commits](https://www.conventionalcommits.org/): `feat:`, `fix:`, `refactor:`,
  `docs:`, `test:`, `chore:`, `perf:`, `ci:`.
- **License.** By contributing you agree that your contributions are licensed under GPL-3.0-or-later.

## Pull requests

1. Open an issue first for larger changes so we can agree on the approach.
2. Keep PRs focused; include screenshots or recordings for UI changes.
3. Make sure `cargo fmt`, `cargo clippy` and `cargo test` pass — CI runs the same checks.

## Secrets

Run `scripts/install-hooks.sh` once after cloning. The pre-commit and pre-push hooks run `scripts/check-secrets.py` and
block credentials (API keys, tokens, private keys). Use obviously fake values in tests (`figd_example_not_a_real_key`)
and never bypass the hooks. See [CLAUDE.md](CLAUDE.md) for the full rules.
