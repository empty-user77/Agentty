---
name: release
description: Release a new Agentty version to github.com/empty-user77/agentty-releases — preflight, version bump, changelog, CI checks, tag, signed/notarized DMG, English release notes, draft release, independent verification, publish and update-feed check. Use when the user asks to release, ship, publish or cut a version (출시, 배포, 릴리즈).
---

# Releasing Agentty

Agentty's source lives in `empty-user77/Agentty`; binaries and the auto-update feed live in
`empty-user77/agentty-releases`. Installed apps poll `releases/latest` of that repo, so **publishing a release ships an
update to every user**.

## Rules

1. **Release notes are always in English** — the GitHub release title, body and the `CHANGELOG.md` entry — even though
   the conversation with the user is in Korean. Report progress to the user in Korean.
2. **Don't spell out changes users may take as sensitive in release notes or `CHANGELOG.md`** — e.g. analytics or
   data collection, telemetry, removed privacy/opt-out settings, permissions, licensing or pricing. Describe them
   neutrally at a high level ("Settings → General has been simplified") or leave them out; never write anything
   untrue. When a release contains such a change, show the user the notes before uploading the draft. Product docs
   (`README.md`, `docs/metrics.md`) must still describe current behavior accurately.
3. Never publish the draft (`--draft=false`) without the user's explicit OK in this conversation. Creating/refreshing
   a draft is fine once the user asked for a release.
4. Follow CLAUDE.md secret rules: never read or print `.env.agentty-prod`, never `source` it in a command you type.
   The scripts load it themselves (`build-dmg.sh`, `release-preflight.sh`, `verify-release.sh`) and print names or
   pass/fail only. Release notes must not contain credentials, personal paths or internal hostnames.
5. Never bypass checks (`--no-verify`, skipping CI, uploading an unnotarized build). If a step fails, stop and fix it.
6. Tags are `vX.Y.Z` (SemVer) and must be higher than the latest published release, or the updater ignores them.
7. **Nothing irreversible before the preflight passes.** No bump, commit, tag or push until
   `scripts/release-preflight.sh X.Y.Z` reports no ✗. A pushed tag without a buildable release is a half-finished state.
8. **"Built", "signed", "notarized" or "released" are only said after `scripts/verify-release.sh` passes** for that
   state. Build logs are not proof. Every status report starts with what is *not* done yet, as a checklist:

   ```
   - ✅ 소스 커밋·태그·푸시, CI 통과
   - ❌ DMG 빌드·서명·공증 — <이유>
   - ⏳ draft 업로드 대기
   - ⬜ 게시 (사용자 확인 필요)
   ```
   A `cargo build --release` from the CI checks is not a release build — never list it as "build done".

## Permissions and long-running commands

- Run the scripts as plain commands (`./scripts/build-dmg.sh publish`), not wrapped in `bash -c 'source …'`. Commands
  that source production credentials inline are what the permission classifier blocks.
- If Claude Code denies a release command, **do not retry variants or try to edit permission files** (that is blocked
  as self-modification too). Stop and tell the user, in one short message, the exact line to run themselves with the
  `!` prefix, or to switch out of auto mode (Shift+Tab) so the command gets an approval prompt.
- Every command you hand the user must be zsh-safe: no bare globs that may match nothing (`rm -rf dist/Agentty*` fails
  in zsh when nothing matches). Prefer the scripts, which handle cleanup themselves.
- The build takes 5–15 minutes (compile, sign, notarize). Run it with `run_in_background` and a log file in the
  scratchpad; read the log with `tail` when notified. Don't chain `sleep` calls.

## Procedure

### 1. Preflight (before touching anything)
```sh
scripts/release-preflight.sh X.Y.Z
```
It checks: on `main`, up to date, tag free, secret-scanning hooks installed, the active `gh` account can push to both
repos, latest CI green, version newer than the published one, `.env.agentty-prod` present/private/gitignored with all
signing, notarization and GA variables set, the Developer ID certificate in the keychain, and the build tools.

Common fixes:
| ✗ | Fix |
|---|---|
| git hooks not installed | `scripts/install-hooks.sh` |
| `.env.agentty-prod` missing | Ask the user to copy it from the previous checkout (e.g. `! cp -p <old repo>/.env.agentty-prod .`). Never create it from values you saw elsewhere. |
| account cannot push | `gh auth switch -u empty-user77` |
| certificate not in keychain | unlock the login keychain |

Uncommitted feature work is only a warning: commit it (conventional message, with the Co-Authored-By trailer) before
step 2 so the release commit contains only the bump and changelog.

If `agentty-releases` has no commits yet (first release), it needs one before a tag can exist: add a short English
`README.md` (what Agentty is, download link to the latest release, link to the source repo) with
`gh api -X PUT repos/empty-user77/agentty-releases/contents/README.md -f message="docs: add README" -f content="$(base64 < README.md)"`.

### 2. Version and changelog
- Pick the version with the user (`scripts/bump-version.sh patch|minor|major|x.y.z`).
- In `CHANGELOG.md`, rename `## [Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD` (today's date) and add a fresh empty
  `## [Unreleased]` above it. Keep Keep-a-Changelog sections (Added / Changed / Fixed / Security).
- Cross-check against `git log <previous tag>..HEAD --oneline` so nothing user-visible is missing.

### 3. Checks (exactly what CI runs)
```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release -p agentty-app
python3 scripts/check-secrets.py --all
```

### 4. Commit and tag the source
```sh
git commit -am "chore: release vX.Y.Z"      # with the Co-Authored-By trailer
git tag -a vX.Y.Z -m "Agentty vX.Y.Z"
git push origin main && git push origin vX.Y.Z
```
Watch CI (`gh run watch <id> --exit-status`) and fix failures before building.

### 5. Release notes (English)
Write `dist/release-notes-vX.Y.Z.md` (dist/ is gitignored) from the changelog entry, for end users:

```markdown
## Highlights
- <2–5 short bullets on what matters most>

## Added
- ...

## Fixed
- ...

## Install
Download `Agentty-X.Y.Z-release<build>-arm64.dmg` below, open it and drag **Agentty** into Applications.
Requires macOS 13+ on Apple silicon. Signed with Developer ID and notarized by Apple.
Existing installs update automatically.

**Checksums:** see `Agentty-X.Y.Z-SHA256SUMS.txt`.
**Source:** https://github.com/empty-user77/Agentty/tree/vX.Y.Z
```
Plain, factual English; no Korean, no internal jargon, no secrets. Show the notes to the user before publishing.

### 6. Build, notarize and upload a draft
```sh
AGENTTY_RELEASE_NOTES=dist/release-notes-vX.Y.Z.md ./scripts/build-dmg.sh publish > <scratchpad>/release.log 2>&1
```
(background; `build-dmg.sh` loads `.env.agentty-prod`, maps legacy `COSTERM_*` names and refuses to publish without
GA credentials.) The log should end with `Notarized and stapled`, `Uploaded to …` and `Done`, but that is not the proof:

### 7. Verify the draft independently
```sh
scripts/verify-release.sh X.Y.Z
```
Checks app version, deep/strict signature, team and hardened runtime, `spctl` "Notarized Developer ID" and stapled
tickets for app and DMG, checksums, GA credentials compiled into the binary, and that the draft holds exactly the DMG,
zip and SHA256SUMS. Report the result as the checklist from rule 8, then ask the user to review the draft.

### 8. Publish (only after the user says so)
```sh
gh release edit vX.Y.Z -R empty-user77/agentty-releases --draft=false --prerelease=false
gh release edit vX.Y.Z -R empty-user77/agentty-releases --latest
```
Two separate calls: combining `--draft=false --latest` fails with HTTP 422 ("Latest release cannot be draft or
prerelease") and has left the release published as a **pre-release**, which the update feed ignores. Always check with
step 9.

### 9. Verify the update feed
```sh
scripts/verify-release.sh X.Y.Z --published
```
Adds: release is public, `releases/latest` serves vX.Y.Z, and the downloaded DMG matches the published checksums.
Then report (in Korean): version, release URL, asset names, and that installed apps pick it up within an hour (or at
next launch).

## Rollback
If a published release is broken: mark it as a draft again (`gh release edit vX.Y.Z --draft=true`) so the updater
stops offering it, fix, and ship `vX.Y.(Z+1)` — never reuse a version number that was already published.
