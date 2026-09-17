---
name: release
description: Release a new Agentty version to github.com/empty-user77/agentty-releases — version bump, changelog, CI checks, tag, signed/notarized DMG, English release notes, draft release, publish and update-feed verification. Use when the user asks to release, ship, publish or cut a version (출시, 배포, 릴리즈).
---

# Releasing Agentty

Agentty's source lives in `empty-user77/Agentty`; binaries and the auto-update feed live in
`empty-user77/agentty-releases`. Installed apps poll `releases/latest` of that repo, so **publishing a release ships an
update to every user**.

## Rules

1. **Release notes are always in English** — the GitHub release title, body and the `CHANGELOG.md` entry — even though
   the conversation with the user is in Korean. Report progress to the user in Korean.
2. Never publish the draft (`--draft=false`) without the user's explicit OK in this conversation. Creating/refreshing
   a draft is fine once the user asked for a release.
3. Follow CLAUDE.md secret rules: never read or print `.env.agentty-prod`; only `source` it inside the build command.
   Release notes must not contain credentials, personal paths or internal hostnames.
4. Never bypass checks (`--no-verify`, skipping CI, uploading an unnotarized build). If a step fails, stop and fix it.
5. Tags are `vX.Y.Z` (SemVer) and must be higher than the latest published release, or the updater ignores them.

## Procedure

### 1. Preflight
```sh
git status --short            # must be clean, on main, up to date with origin
gh auth status
gh run list --branch main --limit 1   # latest CI must be green
gh release list -R empty-user77/agentty-releases --limit 5
```
If `agentty-releases` has no commits yet (first release), it needs one before a tag can exist: add a short English
`README.md` (what Agentty is, download link to the latest release, link to the source repo) with
`gh api -X PUT repos/empty-user77/agentty-releases/contents/README.md -f message="docs: add README" -f content="$(base64 < README.md)"`.

### 2. Version and changelog
- Pick the version with the user (first release: the current `0.1.0`; otherwise `scripts/bump-version.sh patch|minor|major|x.y.z`).
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
Watch CI (`gh run watch`) and fix failures before building.

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
Download `Agentty-X.Y.Z-release<build>-arm64.dmg`, open it and drag **Agentty** into Applications.
Requires macOS 13+ on Apple silicon. Signed with Developer ID and notarized by Apple.
Existing installs update automatically.

**Checksums:** see `Agentty-X.Y.Z-SHA256SUMS.txt`.
**Source:** https://github.com/empty-user77/Agentty/tree/vX.Y.Z
```
Plain, factual English; no Korean, no internal jargon, no secrets. Show the notes to the user before publishing.

### 6. Build, notarize and upload a draft
`.env.agentty-prod` may still use legacy `COSTERM_*` names for the Apple credentials; map them without printing:
```sh
rm -rf dist/Agentty*          # old artifacts; keep the notes file
bash -c 'set -a; source ./.env.agentty-prod; set +a
for n in IDENTITY TEAM_ID APPLE_ID APPLE_PASSWORD; do l="COSTERM_$n"; c="AGENTTY_$n"
  if [ -z "${!c:-}" ] && [ -n "${!l:-}" ]; then export "$c=${!l}"; fi; done
AGENTTY_RELEASE_NOTES=dist/release-notes-vX.Y.Z.md ./scripts/build-dmg.sh publish' > <scratchpad>/release.log 2>&1
```
Check the log ends with `Notarized and stapled`, `source=Notarized Developer ID` and the upload line. Confirm GA
analytics credentials are present (`[ -n "$AGENTTY_GA_MEASUREMENT_ID" ]` after sourcing, never echo the values).

Verify the draft:
```sh
gh release view vX.Y.Z -R empty-user77/agentty-releases --json isDraft,name,body,assets \
  --jq '{isDraft, name, assets: [.assets[].name]}'
```
Assets must be exactly: the `-arm64.dmg`, the `-arm64.zip` and `-SHA256SUMS.txt`, all with version `X.Y.Z`.

### 7. Publish (only after the user says so)
```sh
gh release edit vX.Y.Z -R empty-user77/agentty-releases --draft=false --latest
```

### 8. Verify the update feed
```sh
gh api repos/empty-user77/agentty-releases/releases/latest --jq '.tag_name, [.assets[].name]'
curl -sL -o /tmp/agentty.dmg "<dmg browser_download_url>" && shasum -a 256 /tmp/agentty.dmg   # matches SHA256SUMS
```
Then report to the user (in Korean): version, release URL, asset names, and that installed apps will pick it up within
an hour (or at next launch).

## Rollback
If a published release is broken: mark it as a draft again (`gh release edit vX.Y.Z --draft=true`) so the updater
stops offering it, fix, and ship `vX.Y.(Z+1)` — never reuse a version number that was already published.
