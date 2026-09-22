# Building and releasing Agentty

```sh
./scripts/build-dmg.sh           # dev: local signature, no notarization → dist/Agentty-<ver>-arm64.dmg
./scripts/build-dmg.sh prod      # Developer ID signature + notarization + stapling + Gatekeeper check
./scripts/build-dmg.sh publish   # prod + draft release on empty-user77/agentty-releases
./scripts/bump-version.sh patch  # 0.1.0 → 0.1.1 (minor | major | x.y.z)
./scripts/release-preflight.sh 0.1.3        # before bumping/tagging: repo, gh, CI, credentials, certificate, tools
./scripts/verify-release.sh 0.1.3           # after the build: signature, notarization, checksums, GA, draft assets
./scripts/verify-release.sh 0.1.3 --published  # after publishing: update feed and downloaded installers' checksums
./scripts/fetch-release-packages.sh 0.1.3   # after tagging: Windows installer and Linux packages from the workflow
./scripts/build-linux-packages.sh --check   # local: .deb and .rpm in Docker, then install each in a clean container
pwsh scripts/package-windows.ps1 -Installer # on Windows: zip + Inno Setup installer
```

## Windows installer and Linux packages

Each release also carries, built by the **Release packages** workflow (`.github/workflows/release-packages.yml`, on
`v*` tag pushes, on pull requests that change the packaging, and by hand with
`gh workflow run release-packages.yml -f ref=<ref>`):

| File | Built by | On |
|---|---|---|
| `Agentty-X.Y.Z-windows-x64-setup.exe` | `scripts/package-windows.ps1 -Installer` (Inno Setup, `packaging/windows/agentty.iss`) | self-hosted Windows PC |
| `Agentty-X.Y.Z-windows-x64-setup.zip` | the same installer zipped, for browsers that block unsigned `.exe` downloads | self-hosted Windows PC |
| `Agentty-X.Y.Z-linux-amd64.deb`, `Agentty-X.Y.Z-linux-x86_64.rpm` | `scripts/build-linux-packages.sh` (nfpm, `packaging/linux/nfpm.yaml`) | the Mac, in an x86_64 AlmaLinux 9 container |

- The Windows PC needs Inno Setup 6 (`winget install JRSoftware.InnoSetup`); without it the job fails and says so. The
  installer is per user (`PrivilegesRequired=lowest`, `%LOCALAPPDATA%\Programs\Agentty`), requires Windows 10 1809
  (`MinVersion=10.0.17763`) and registers what `install.ps1` does. It is not code-signed.
- The Linux build runs under emulation (`--platform linux/amd64`), so it is slow; named volumes keep the toolchain and
  `target/`. AlmaLinux 9's glibc 2.34 makes the binary run on RHEL 9, Debian 12, Ubuntu 22.04 and newer. nfpm is
  downloaded at a pinned version and checked against a pinned SHA-256. `--check` installs each package with `apt` /
  `dnf` in a clean container and fails when a shared library is missing.
- The workflow uploads the files as run artifacts only; it holds no token for `agentty-releases`.
  `scripts/fetch-release-packages.sh X.Y.Z` downloads them into `dist/`, and `build-dmg.sh publish` puts them into
  `SHA256SUMS.txt` and the draft release. `publish` stops before building if one is missing, unless
  `AGENTTY_MAC_ONLY=1` (a macOS-only release, only with the maintainer's OK).

## What `prod` does

1. `cargo build --release -p agentty-app` in a private `git worktree` of HEAD — not this checkout. The repository is
   shared with agent sessions, and an edit landing while cargo runs would be signed and notarized along with
   everything else. `prod` and `publish` therefore refuse to start when the working tree has uncommitted changes, or
   when a `v<version>` tag exists that is not HEAD. That worktree has its own `target/`, so a release build starts
   cold and takes a few minutes. `dev` still builds this checkout, which is the point of a dev build.
2. Assembles `dist/Agentty.app` (`packaging/macos/Info.plist.in`, `Agentty.icns`, license files)
3. Signs with **hardened runtime** and `packaging/macos/entitlements.plist`
4. Creates and signs `dist/Agentty-<ver>-release<timestamp>-arm64.dmg`
5. Submits to Apple with `xcrun notarytool submit --wait`, then staples app and DMG
6. Verifies with `spctl` (must report `source=Notarized Developer ID`) and `stapler validate`
7. Writes a zip and `SHA256SUMS.txt`

## Credentials

Set these in the environment or in `.env.agentty-prod` in the repository root (gitignored, `chmod 600`; see
`.env.example`). The file does not move with a fresh clone or a renamed checkout — copy it over, then run
`release-preflight.sh`. Legacy `COSTERM_*` names are still accepted. `publish` refuses to run without
`AGENTTY_GA_MEASUREMENT_ID` and `AGENTTY_GA_API_SECRET`, which are compiled into the binary.

| Variable | Value |
|---|---|
| `AGENTTY_IDENTITY` | `NAME (TEAMID)` — the text after `Developer ID Application: ` |
| `AGENTTY_TEAM_ID` | 10-character team id |
| `AGENTTY_APPLE_ID` | Apple ID email |
| `AGENTTY_APPLE_PASSWORD` | app-specific password (appleid.apple.com → App-Specific Passwords) |

`.env.agentty-prod` may reuse a Developer ID the maintainer already has, so no secret is duplicated. The certificate must be in the login keychain:

```sh
security find-identity -v -p codesigning | grep "Developer ID Application"
```

## Publishing

`publish` requires an authenticated `gh` CLI. It creates (or refreshes) a **draft** release `v<version>` on
`empty-user77/agentty-releases` with the DMG, zip, Windows and Linux files and checksums. `AGENTTY_RELEASE_NOTES=<file.md>` sets the release
notes. Review the draft on GitHub, then publish.

**Release notes are always written in English.** The full procedure (version bump, changelog, tag, notes, draft,
publish, update-feed check) is in [`.claude/skills/release/SKILL.md`](../.claude/skills/release/SKILL.md).

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

On Windows, a copy installed by the setup program (or by `install.ps1` into the same folder) downloads
`*-windows-x64-setup.exe` the same way — GitHub over HTTPS only, into a new temp folder of its own, verified against
`*-SHA256SUMS.txt` — then starts it with `/SILENT /SUPPRESSMSGBOXES /NORESTART /NOCANCEL /RELAUNCH /WAITPID=<pid>` and
quits. The installer waits for that process, renames an `agentty.exe` that other Agentty processes (MCP servers) still
run instead of closing them, installs and starts the new version. Linux builds announce the update and open the
release page; the packages are installed with the package manager.
