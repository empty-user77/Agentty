#!/usr/bin/env bash
#
# release-preflight.sh <version> — checks everything a release needs BEFORE bumping, tagging or pushing.
#
# Prints one line per check and exits non-zero if any required check fails. Never prints credential values:
# `.env.agentty-prod` is only sourced in a subshell to test that variables are set.
#
#   ./scripts/release-preflight.sh 0.1.3
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VERSION="${1:-}"
RELEASE_REPO="${AGENTTY_RELEASE_REPO:-empty-user77/agentty-releases}"
SOURCE_REPO="${AGENTTY_SOURCE_REPO:-empty-user77/Agentty}"
TAP_REPO="${AGENTTY_TAP_REPO:-empty-user77/homebrew-agentty}"
ENV_FILE="$ROOT/.env.agentty-prod"
FAILED=0

pass() { printf "\033[1;32m  ✓\033[0m %s\n" "$*"; }
fail() { printf "\033[1;31m  ✗\033[0m %s\n" "$*"; FAILED=1; }
note() { printf "\033[1;33m  !\033[0m %s\n" "$*"; }

[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "usage: $0 X.Y.Z" >&2; exit 2; }
echo "Release preflight for v$VERSION"

# ─── Source tree ───
branch="$(git rev-parse --abbrev-ref HEAD)"
[[ "$branch" == "main" ]] && pass "on main" || fail "on '$branch', expected main"
if [[ -z "$(git status --porcelain)" ]]; then pass "working tree clean"; else note "working tree has changes (commit them before tagging)"; fi
git fetch -q origin main 2>/dev/null
behind="$(git rev-list --count HEAD..origin/main 2>/dev/null || echo "?")"
[[ "$behind" == "0" ]] && pass "up to date with origin/main" || fail "behind origin/main by $behind commit(s)"
if git rev-parse -q --verify "refs/tags/v$VERSION" >/dev/null || git ls-remote --exit-code --tags origin "v$VERSION" >/dev/null 2>&1; then
  fail "tag v$VERSION already exists"
else
  pass "tag v$VERSION is free"
fi
hooks="$(git config --get core.hooksPath || true)"
[[ "$hooks" == ".githooks" ]] && pass "secret-scanning git hooks installed" || fail "git hooks not installed — run scripts/install-hooks.sh"

# ─── GitHub ───
if gh auth status >/dev/null 2>&1; then
  pass "gh authenticated"
  for repo in "$SOURCE_REPO" "$RELEASE_REPO" "$TAP_REPO"; do
    [[ "$(gh api "repos/$repo" --jq .permissions.push 2>/dev/null)" == "true" ]] && pass "active gh account can push to $repo" \
      || fail "active gh account cannot push to $repo (gh auth switch)"
  done
  ci="$(gh run list --repo "$SOURCE_REPO" --workflow ci.yml --branch main --event push --limit 1 --json status,conclusion --jq '.[0] | "\(.status) \(.conclusion)"' 2>/dev/null)"
  [[ "$ci" == "completed success" ]] && pass "latest CI on main is green" || fail "latest CI on main: ${ci:-unknown}"
  latest="$(gh api "repos/$RELEASE_REPO/releases/latest" --jq .tag_name 2>/dev/null || echo "none")"
  newer="$(python3 -c 'import sys
p=lambda s: tuple(int(x) for x in s.lstrip("v").split("."))
print(sys.argv[2]=="none" or p(sys.argv[1])>p(sys.argv[2]))' "$VERSION" "$latest")"
  [[ "$newer" == "True" ]] && pass "v$VERSION is newer than published $latest" || fail "v$VERSION is not newer than published $latest"
  # The tag starts the "Release packages" workflow (Windows installer, Linux packages) on the self-hosted runners;
  # with a runner offline it waits forever and the release can't go out with those files.
  runners="$(gh api "repos/$SOURCE_REPO/actions/runners" --jq '.runners[] | "\(.status) \([.labels[].name] | join(","))"' 2>/dev/null)"
  if [[ -z "$runners" ]]; then
    note "cannot list $SOURCE_REPO's self-hosted runners (needs admin access); check they are online before tagging"
  else
    for label in Windows macOS; do
      grep -q "^online .*\b$label\b" <<<"$runners" && pass "self-hosted $label runner online" \
        || fail "self-hosted $label runner is offline (the Release packages workflow needs it)"
    done
  fi
else
  fail "gh not authenticated (gh auth login)"
fi

# ─── Credentials (names only, never values) ───
if [[ -f "$ENV_FILE" ]]; then
  pass ".env.agentty-prod present"
  [[ "$(stat -f %Lp "$ENV_FILE")" == "600" ]] && pass ".env.agentty-prod is private (0600)" || note ".env.agentty-prod should be chmod 600"
  git check-ignore -q "$ENV_FILE" && pass ".env.agentty-prod is gitignored" || fail ".env.agentty-prod is NOT gitignored"
  missing="$(bash -c '
    set -a; source "$1"; set +a
    for n in IDENTITY TEAM_ID APPLE_ID APPLE_PASSWORD; do l="COSTERM_$n"; c="AGENTTY_$n"
      [ -z "${!c:-}" ] && [ -n "${!l:-}" ] && export "$c=${!l}"; done
    for v in AGENTTY_IDENTITY AGENTTY_TEAM_ID AGENTTY_APPLE_ID AGENTTY_APPLE_PASSWORD AGENTTY_GA_MEASUREMENT_ID AGENTTY_GA_API_SECRET; do
      [ -n "${!v:-}" ] || printf "%s " "$v"; done
    security find-identity -v -p codesigning 2>/dev/null | grep -qF "Developer ID Application: ${AGENTTY_IDENTITY:-}" || printf "KEYCHAIN_IDENTITY "
  ' _ "$ENV_FILE")"
  if [[ -z "$missing" ]]; then
    pass "signing, notarization and GA variables set; Developer ID certificate in keychain"
  else
    for m in $missing; do
      [[ "$m" == KEYCHAIN_IDENTITY ]] && fail "Developer ID certificate for AGENTTY_IDENTITY not in keychain (unlock login keychain?)" || fail "$m is empty"
    done
  fi
else
  fail ".env.agentty-prod missing in $ROOT (copy it from the previous checkout; see .env.example)"
fi

# ─── Tools ───
for tool in cargo python3 xcrun hdiutil ditto codesign spctl; do
  command -v "$tool" >/dev/null && pass "$tool available" || fail "$tool not found"
done
xcrun --find notarytool >/dev/null 2>&1 && pass "notarytool available" || fail "notarytool not found (install Xcode command line tools)"
[[ -f .github/workflows/release-packages.yml ]] && pass "release-packages workflow present" || fail ".github/workflows/release-packages.yml missing"
command -v jq >/dev/null && pass "jq available" || fail "jq not found (brew install jq; verify-release.sh needs it)"
docker info >/dev/null 2>&1 && pass "Docker is running (Linux packages build in it on this Mac)" \
  || note "Docker is not running here; if this Mac is the runner, start Docker Desktop before tagging"

echo
if [[ "$FAILED" == 0 ]]; then
  echo "Preflight passed — safe to bump, tag and build v$VERSION."
else
  echo "Preflight FAILED — fix the ✗ items before bumping or tagging."
fi
exit "$FAILED"
