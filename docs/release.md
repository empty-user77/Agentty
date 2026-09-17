# Building and releasing Agentty (macOS)

```sh
./scripts/build-dmg.sh           # dev: local signature, no notarization → dist/Agentty-<ver>-arm64.dmg
./scripts/build-dmg.sh prod      # Developer ID signature + notarization + stapling + Gatekeeper check
./scripts/build-dmg.sh publish   # prod + draft release on empty-user77/agentty-releases
./scripts/bump-version.sh patch  # 0.1.0 → 0.1.1 (minor | major | x.y.z)
```

## What `prod` does

1. `cargo build --release -p agentty-app`
2. Assembles `dist/Agentty.app` (`packaging/macos/Info.plist.in`, `Agentty.icns`, license files)
3. Signs with **hardened runtime** and `packaging/macos/entitlements.plist`
4. Creates and signs `dist/Agentty-<ver>-release<timestamp>-arm64.dmg`
5. Submits to Apple with `xcrun notarytool submit --wait`, then staples app and DMG
6. Verifies with `spctl` (must report `source=Notarized Developer ID`) and `stapler validate`
7. Writes a zip and `SHA256SUMS.txt`

## Credentials

Set these in the environment or in `.env.agentty-prod` (gitignored; see `.env.example`):

| Variable | Value |
|---|---|
| `AGENTTY_IDENTITY` | `NAME (TEAMID)` — the text after `Developer ID Application: ` |
| `AGENTTY_TEAM_ID` | 10-character team id |
| `AGENTTY_APPLE_ID` | Apple ID email |
| `AGENTTY_APPLE_PASSWORD` | app-specific password (appleid.apple.com → App-Specific Passwords) |

On the maintainer's machine `.env.agentty-prod` reuses the Developer ID credentials of `cosmica-desktop`,
so no secret is duplicated. The certificate must be in the login keychain:

```sh
security find-identity -v -p codesigning | grep "Developer ID Application"
```

## Publishing

`publish` requires an authenticated `gh` CLI. It creates (or refreshes) a **draft** release `v<version>` on
`empty-user77/agentty-releases` with the DMG, zip and checksums. Review the notes on GitHub, then publish.

## Troubleshooting

| Symptom | Fix |
|---|---|
| `errSecInternalComponent` | Unlock the login keychain |
| notarytool `Invalid credentials` | Regenerate the app-specific password |
| notarytool 403 agreement missing | Accept the latest agreement at developer.apple.com/account |
| `spctl` rejects the app | Check the notarization log: `xcrun notarytool log <id> --apple-id … --team-id …` |

## Icons

App icon, in-app logo and the menu bar template are generated from the transparent logo:

```sh
swift scripts/make-icons.swift path/to/logo-transparent.png /tmp/agentty-icons
iconutil -c icns /tmp/agentty-icons/Agentty.iconset -o crates/agentty-app/assets/brand/Agentty.icns
cp /tmp/agentty-icons/logo-256.png /tmp/agentty-icons/menubar@2x.png crates/agentty-app/assets/brand/
```

## Auto-update

Agentty checks `https://api.github.com/repos/empty-user77/agentty-releases/releases/latest` at launch and every hour.
Drafts and pre-releases are ignored, so **publishing the draft created by `build-dmg.sh publish` is what ships an
update** (the release tag must be `vX.Y.Z` and higher than the running version).

Installing an update:

1. Downloads the `*-arm64.dmg` asset over HTTPS from GitHub only, and verifies it against `*-SHA256SUMS.txt`.
2. Mounts it read-only and checks the app with `codesign --verify --deep --strict`, requires the **same Team ID** as the
   running app, and requires Gatekeeper (`spctl`) acceptance (notarization).
3. Copies it next to the running bundle, quits, swaps the bundles and relaunches.

Development builds (not running from `Agentty.app`) only check; "Install" opens the release page.
