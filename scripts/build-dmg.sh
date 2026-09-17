#!/usr/bin/env bash
#
# build-dmg.sh — Agentty macOS app bundle + DMG builder.
#
# Usage:
#   ./scripts/build-dmg.sh            # dev: ad-hoc (or "Cosmica Dev" self-signed) signature, no notarization
#   ./scripts/build-dmg.sh prod       # prod: Developer ID signature + notarization + stapling
#   ./scripts/build-dmg.sh publish    # prod + upload to GitHub release feed (draft)
#
# Prod credentials (never committed) come from the environment or `.env.agentty-prod`:
#   AGENTTY_IDENTITY        "NAME (TEAMID)"  (the part after "Developer ID Application: ")
#   AGENTTY_TEAM_ID         10-character Apple team id
#   AGENTTY_APPLE_ID        Apple ID email
#   AGENTTY_APPLE_PASSWORD  app-specific password
# See docs/release.md.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

MODE="${1:-dev}"
RELEASE_REPO="${AGENTTY_RELEASE_REPO:-empty-user77/agentty-releases}"
APP_NAME="Agentty"
DIST="$ROOT/dist"
ARCH="$(uname -m)"

log() { printf "\033[1;36m[build]\033[0m %s\n" "$*"; }
ok()  { printf "\033[1;32m[  ok ]\033[0m %s\n" "$*"; }
warn() { printf "\033[1;33m[warn ]\033[0m %s\n" "$*"; }
die() { printf "\033[1;31m[error]\033[0m %s\n" "$*" >&2; exit 1; }

# ─── 1. Mode and credentials ───
PUBLISH="false"
case "$MODE" in
  dev)
    NOTARIZE="false"
    if security find-identity -v -p codesigning 2>/dev/null | grep -q '"Cosmica Dev"'; then
      SIGN_ID="Cosmica Dev"
    else
      SIGN_ID="-"  # ad-hoc
    fi
    log "DEV build (signature: $SIGN_ID)"
    ;;
  prod|publish)
    [[ "$MODE" == "publish" ]] && PUBLISH="true"
    if [[ -z "${AGENTTY_IDENTITY:-}" && -f "$ROOT/.env.agentty-prod" ]]; then
      log "Loading credentials from .env.agentty-prod"
      set -a; source "$ROOT/.env.agentty-prod"; set +a
    fi
    : "${AGENTTY_IDENTITY:?Set AGENTTY_IDENTITY (or create .env.agentty-prod)}"
    : "${AGENTTY_TEAM_ID:?Set AGENTTY_TEAM_ID}"
    : "${AGENTTY_APPLE_ID:?Set AGENTTY_APPLE_ID}"
    : "${AGENTTY_APPLE_PASSWORD:?Set AGENTTY_APPLE_PASSWORD (app-specific password)}"
    SIGN_ID="Developer ID Application: $AGENTTY_IDENTITY"
    security find-identity -v -p codesigning 2>/dev/null | grep -qF "$SIGN_ID" || die "Certificate '$SIGN_ID' not found in keychain"
    NOTARIZE="true"
    if [[ "$PUBLISH" == "true" ]]; then
      command -v gh >/dev/null || die "gh CLI not installed (brew install gh)"
      gh auth status >/dev/null 2>&1 || die "gh CLI not authenticated (gh auth login)"
    fi
    log "PROD build (signature: $SIGN_ID, notarization: on)"
    ;;
  *) die "Unknown mode '$MODE' (dev | prod | publish)" ;;
esac

# ─── 2. Version ───
VERSION="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="agentty-app"))')"
BUILD="$(date +%Y%m%d%H%M%S)"
[[ -n "$VERSION" ]] || die "Could not read version"
log "Version $VERSION (build $BUILD, $ARCH)"

# ─── 3. Compile ───
log "cargo build --release"
cargo build --release -p agentty-app
BIN="$ROOT/target/release/agentty"
[[ -x "$BIN" ]] || die "release binary not found at $BIN"

# ─── 4. Assemble .app ───
APP="$DIST/$APP_NAME.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/agentty"
cp "$ROOT/crates/agentty-app/assets/brand/Agentty.icns" "$APP/Contents/Resources/Agentty.icns"
sed -e "s/@VERSION@/$VERSION/" -e "s/@BUILD@/$BUILD/" "$ROOT/packaging/macos/Info.plist.in" > "$APP/Contents/Info.plist"
cp "$ROOT/LICENSE" "$APP/Contents/Resources/LICENSE.txt"
cp "$ROOT/THIRD_PARTY_NOTICES.md" "$APP/Contents/Resources/THIRD_PARTY_NOTICES.md"
plutil -lint "$APP/Contents/Info.plist" >/dev/null || die "Info.plist is invalid"
ok "Bundle assembled: $APP"

# ─── 5. Sign the app ───
log "Signing app"
SIGN_ARGS=(--force --timestamp --options runtime --entitlements "$ROOT/packaging/macos/entitlements.plist" --sign "$SIGN_ID")
[[ "$SIGN_ID" == "-" ]] && SIGN_ARGS=(--force --options runtime --entitlements "$ROOT/packaging/macos/entitlements.plist" --sign "-")
codesign "${SIGN_ARGS[@]}" "$APP/Contents/MacOS/agentty"
codesign "${SIGN_ARGS[@]}" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP" 2>&1 | sed 's/^/  /'
ok "App signed"

# ─── 6. DMG ───
DMG_NAME="$APP_NAME-$VERSION-$ARCH.dmg"
[[ "$MODE" != "dev" ]] && DMG_NAME="$APP_NAME-$VERSION-release$BUILD-$ARCH.dmg"
DMG="$DIST/$DMG_NAME"
STAGE="$(mktemp -d)"
VOLNAME="$APP_NAME $VERSION"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
mkdir -p "$STAGE/.background"
cp "$ROOT/packaging/macos/dmg/background.tiff" "$STAGE/.background/background.tiff"
rm -f "$DMG"
RW_DMG="$(mktemp -d)/rw.dmg"
hdiutil create -volname "$VOLNAME" -srcfolder "$STAGE" -ov -format UDRW -fs HFS+ "$RW_DMG" >/dev/null
rm -rf "$STAGE"

# Styled installer window: background, icon positions, volume icon. Finder scripting needs the
# Automation permission; without it the DMG still works, just unstyled.
MOUNT_DIR="$(mktemp -d)/mnt"
mkdir -p "$MOUNT_DIR"
hdiutil attach "$RW_DMG" -readwrite -noverify -noautoopen -mountpoint "$MOUNT_DIR" >/dev/null
cp "$ROOT/crates/agentty-app/assets/brand/Agentty.icns" "$MOUNT_DIR/.VolumeIcon.icns"
if command -v SetFile >/dev/null 2>&1; then SetFile -a C "$MOUNT_DIR" || true; fi
if osascript <<APPLESCRIPT >/dev/null 2>&1
with timeout of 60 seconds
  tell application "Finder"
    set dmgFolder to POSIX file "$MOUNT_DIR" as alias
    open dmgFolder
    set theWindow to container window of dmgFolder
    set current view of theWindow to icon view
    set toolbar visible of theWindow to false
    set statusbar visible of theWindow to false
    set the bounds of theWindow to {200, 120, 860, 568}
    set viewOptions to the icon view options of theWindow
    set arrangement of viewOptions to not arranged
    set icon size of viewOptions to 100
    set text size of viewOptions to 13
    set background picture of viewOptions to file ".background:background.tiff" of dmgFolder
    set position of item "$APP_NAME.app" of dmgFolder to {170, 220}
    set position of item "Applications" of dmgFolder to {490, 220}
    update dmgFolder without registering applications
    delay 2
    close theWindow
  end tell
end timeout
APPLESCRIPT
then
  ok "Installer window styled"
else
  warn "Could not style the installer window (allow Terminal to control Finder in Privacy & Security → Automation)"
fi
chmod -Rf go-w "$MOUNT_DIR" 2>/dev/null || true
sync
hdiutil detach "$MOUNT_DIR" -quiet || hdiutil detach "$MOUNT_DIR" -force -quiet
hdiutil convert "$RW_DMG" -format UDZO -imagekey zlib-level=9 -o "$DMG" >/dev/null
rm -f "$RW_DMG"
if [[ "$SIGN_ID" != "-" ]]; then
  codesign --force --timestamp --sign "$SIGN_ID" "$DMG"
fi
ok "DMG created: $DMG ($(du -h "$DMG" | cut -f1))"

# ─── 7. Notarize + staple ───
if [[ "$NOTARIZE" == "true" ]]; then
  log "Submitting to Apple notary service (usually 1–5 minutes)…"
  xcrun notarytool submit "$DMG" \
    --apple-id "$AGENTTY_APPLE_ID" --password "$AGENTTY_APPLE_PASSWORD" --team-id "$AGENTTY_TEAM_ID" \
    --wait --timeout 30m | sed 's/^/  /'
  xcrun stapler staple "$APP" | sed 's/^/  /'
  xcrun stapler staple "$DMG" | sed 's/^/  /'
  log "Verifying Gatekeeper acceptance"
  spctl -a -vvv -t install "$DMG" 2>&1 | sed 's/^/  /'
  spctl -a -vvv -t exec "$APP" 2>&1 | sed 's/^/  /'
  xcrun stapler validate "$DMG" | sed 's/^/  /'
  ok "Notarized and stapled"
fi

# ─── 8. Checksums + zip (for release feeds) ───
ZIP="$DIST/$APP_NAME-$VERSION-$ARCH.zip"
rm -f "$ZIP"
ditto -c -k --keepParent "$APP" "$ZIP"
(cd "$DIST" && shasum -a 256 "$(basename "$DMG")" "$(basename "$ZIP")" > "$APP_NAME-$VERSION-SHA256SUMS.txt")
ok "Checksums: $DIST/$APP_NAME-$VERSION-SHA256SUMS.txt"

# ─── 9. Publish (draft) ───
if [[ "$PUBLISH" == "true" ]]; then
  TAG="v$VERSION"
  for asset in "$DMG" "$ZIP"; do
    [[ "$(basename "$asset")" == *"$VERSION"* ]] || die "refusing to upload $asset: name lacks version $VERSION"
  done
  if gh release view "$TAG" --repo "$RELEASE_REPO" >/dev/null 2>&1; then
    log "Release $TAG exists — refreshing assets"
  else
    gh release create "$TAG" --repo "$RELEASE_REPO" --draft --title "Agentty $TAG" --notes "Agentty $TAG" >/dev/null
    ok "Draft release $TAG created"
  fi
  gh release upload "$TAG" --repo "$RELEASE_REPO" --clobber "$DMG" "$ZIP" "$DIST/$APP_NAME-$VERSION-SHA256SUMS.txt"
  ok "Uploaded to https://github.com/$RELEASE_REPO/releases (draft — review notes, then publish)"
fi

ok "Done"
