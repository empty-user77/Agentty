#!/usr/bin/env bash
#
# fetch-release-packages.sh <version> — after the vX.Y.Z tag is pushed, waits for the "Release packages" workflow run of
# that tag and copies its Windows and Linux files into dist/, where build-dmg.sh publish picks them up:
#
#   dist/Agentty-X.Y.Z-windows-x64-setup.exe   dist/Agentty-X.Y.Z-windows-x64-setup.zip
#   dist/Agentty-X.Y.Z-linux-amd64.deb         dist/Agentty-X.Y.Z-linux-x86_64.rpm
#
# The run must have built the tagged commit. Nothing is uploaded from here.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VERSION="${1:-}"
SOURCE_REPO="${AGENTTY_SOURCE_REPO:-empty-user77/Agentty}"
WORKFLOW="release-packages.yml"
DIST="$ROOT/dist"

die() { printf "\033[1;31m[error]\033[0m %s\n" "$*" >&2; exit 1; }
ok() { printf "\033[1;32m[  ok ]\033[0m %s\n" "$*"; }

[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "usage: $0 X.Y.Z" >&2; exit 2; }
TAG="v$VERSION"
FILES=(
  "Agentty-$VERSION-windows-x64-setup.exe"
  "Agentty-$VERSION-windows-x64-setup.zip"
  "Agentty-$VERSION-linux-amd64.deb"
  "Agentty-$VERSION-linux-x86_64.rpm"
)

commit="$(git rev-parse "$TAG^{commit}" 2>/dev/null)" || die "tag $TAG not found locally (git fetch --tags)"
run=""
for _ in $(seq 1 30); do
  run="$(gh run list -R "$SOURCE_REPO" --workflow "$WORKFLOW" --event push --branch "$TAG" --limit 20 \
    --json databaseId,headSha --jq "[.[] | select(.headSha == \"$commit\")][0].databaseId // empty")"
  [[ -n "$run" ]] && break
  sleep 10
done
[[ -n "$run" ]] || die "no $WORKFLOW run for $TAG ($commit) — was the tag pushed? Start one by hand: gh workflow run $WORKFLOW -R $SOURCE_REPO -f ref=$TAG"
echo "Waiting for run $run (https://github.com/$SOURCE_REPO/actions/runs/$run)…"
gh run watch "$run" -R "$SOURCE_REPO" --exit-status --interval 60 >/dev/null || die "run $run failed: gh run view $run -R $SOURCE_REPO --log-failed"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
gh run download "$run" -R "$SOURCE_REPO" -D "$tmp" >/dev/null
mkdir -p "$DIST"
for name in "${FILES[@]}"; do
  found="$(find "$tmp" -type f -name "$name" | head -n 1)"
  [[ -n "$found" ]] || die "run $run has no $name"
  cp "$found" "$DIST/$name"
  ok "dist/$name ($(du -h "$DIST/$name" | cut -f1))"
done
