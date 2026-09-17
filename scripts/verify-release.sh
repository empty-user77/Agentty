#!/usr/bin/env bash
#
# verify-release.sh <version> [--published] — independently verifies release artifacts instead of trusting build logs.
#
# Local: app version, code signature and team, notarization (spctl + stapler) of app and DMG, checksums, and that the
# GA credentials were compiled in. Remote: the GitHub release exists with exactly the DMG, zip and checksums.
# --published additionally requires the release to be public and the update feed (releases/latest) to serve it, and
# checks the downloaded DMG against the published checksums. Never prints credential values.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VERSION="${1:-}"
PUBLISHED="false"
[[ "${2:-}" == "--published" ]] && PUBLISHED="true"
RELEASE_REPO="${AGENTTY_RELEASE_REPO:-empty-user77/agentty-releases}"
DIST="$ROOT/dist"
APP="$DIST/Agentty.app"
FAILED=0

pass() { printf "\033[1;32m  ✓\033[0m %s\n" "$*"; }
fail() { printf "\033[1;31m  ✗\033[0m %s\n" "$*"; FAILED=1; }

[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "usage: $0 X.Y.Z [--published]" >&2; exit 2; }
echo "Verifying Agentty v$VERSION"

DMG="$(ls "$DIST"/Agentty-"$VERSION"-release*-arm64.dmg 2>/dev/null | tail -n 1)"
ZIP="$DIST/Agentty-$VERSION-arm64.zip"
SUMS="$DIST/Agentty-$VERSION-SHA256SUMS.txt"

# ─── Local artifacts ───
for f in "$APP" "$DMG" "$ZIP" "$SUMS"; do
  [[ -n "$f" && -e "$f" ]] && pass "exists: ${f#$ROOT/}" || fail "missing: ${f:-dist/Agentty-$VERSION-release*-arm64.dmg}"
done

if [[ -d "$APP" ]]; then
  plist="$(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' "$APP/Contents/Info.plist" 2>/dev/null)"
  [[ "$plist" == "$VERSION" ]] && pass "app version $plist" || fail "app version is '$plist', expected $VERSION"
  codesign --verify --deep --strict "$APP" 2>/dev/null && pass "app signature valid (deep, strict)" || fail "app signature invalid"
  details="$(codesign -dv "$APP" 2>&1)"
  team="$(sed -n 's/^TeamIdentifier=//p' <<<"$details")"
  [[ -n "$team" && "$team" != "not set" ]] && pass "signed by team $team" || fail "no Developer ID team in signature"
  grep -q "flags=.*runtime" <<<"$details" && pass "hardened runtime enabled" || fail "hardened runtime not enabled"
  spctl --assess --type execute -vv "$APP" 2>&1 | grep -q "source=Notarized Developer ID" && pass "app: Notarized Developer ID (spctl)" \
    || fail "app not accepted as Notarized Developer ID"
  xcrun stapler validate "$APP" >/dev/null 2>&1 && pass "app: notarization ticket stapled" || fail "app: no stapled ticket"
fi

if [[ -n "$DMG" && -f "$DMG" ]]; then
  spctl --assess --type open --context context:primary-signature -vv "$DMG" 2>&1 | grep -q "source=Notarized Developer ID" \
    && pass "dmg: Notarized Developer ID (spctl)" || fail "dmg not accepted as Notarized Developer ID"
  xcrun stapler validate "$DMG" >/dev/null 2>&1 && pass "dmg: notarization ticket stapled" || fail "dmg: no stapled ticket"
fi

if [[ -f "$SUMS" ]]; then
  (cd "$DIST" && shasum -a 256 -c "$(basename "$SUMS")" >/dev/null 2>&1) && pass "checksums match" || fail "checksum mismatch"
fi

BIN="$APP/Contents/MacOS/agentty"
if [[ -f "$BIN" && -f "$ROOT/.env.agentty-prod" ]]; then
  ga="$(bash -c '
    set -a; source "$1"; set +a
    for v in AGENTTY_GA_MEASUREMENT_ID AGENTTY_GA_API_SECRET; do
      val="${!v:-}"
      if [ -z "$val" ]; then printf "%s:unset " "$v"
      elif LC_ALL=C grep -a -q -F "$val" "$2"; then printf "%s:ok " "$v"
      else printf "%s:missing " "$v"; fi
    done' _ "$ROOT/.env.agentty-prod" "$BIN")"
  for entry in $ga; do
    [[ "$entry" == *:ok ]] && pass "${entry%%:*} compiled into the binary" || fail "${entry%%:*} ${entry##*:} in the binary"
  done
else
  fail "cannot check GA credentials (binary or .env.agentty-prod missing)"
fi

# ─── GitHub release ───
json="$(gh release view "v$VERSION" -R "$RELEASE_REPO" --json isDraft,assets 2>/dev/null)"
if [[ -z "$json" ]]; then
  fail "release v$VERSION not found on $RELEASE_REPO"
else
  draft="$(jq -r .isDraft <<<"$json")"
  assets="$(jq -r '[.assets[].name] | sort | join(" ")' <<<"$json")"
  expected="$(printf "%s\n" "$(basename "${DMG:-none}")" "$(basename "$ZIP")" "$(basename "$SUMS")" | sort | tr '\n' ' ' | sed 's/ $//')"
  [[ "$assets" == "$expected" ]] && pass "release assets: $assets" || fail "release assets are '$assets', expected '$expected'"
  if [[ "$PUBLISHED" == "true" ]]; then
    [[ "$draft" == "false" ]] && pass "release is published" || fail "release is still a draft"
    latest="$(gh api "repos/$RELEASE_REPO/releases/latest" --jq .tag_name 2>/dev/null)"
    [[ "$latest" == "v$VERSION" ]] && pass "update feed serves v$VERSION" || fail "update feed serves '${latest:-nothing}'"
    tmp="$(mktemp -d)"
    if gh release download "v$VERSION" -R "$RELEASE_REPO" -p "*.dmg" -p "*SHA256SUMS.txt" -D "$tmp" >/dev/null 2>&1 \
      && (cd "$tmp" && shasum -a 256 -c --ignore-missing ./*SHA256SUMS.txt >/dev/null 2>&1); then
      pass "downloaded DMG matches published checksums"
    else
      fail "downloaded DMG does not match published checksums"
    fi
    rm -rf "$tmp"
  else
    [[ "$draft" == "true" ]] && pass "release is a draft (not shipped to users yet)" || fail "release is already published"
  fi
fi

echo
[[ "$FAILED" == 0 ]] && echo "Verification passed for v$VERSION." || echo "Verification FAILED for v$VERSION — do not report it as built/released."
exit "$FAILED"
