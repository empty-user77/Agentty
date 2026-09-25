#!/usr/bin/env bash
# mirror-release-to-legacy.sh <version> — copies a release of empty-user77/Agentty to the old channel,
# empty-user77/agentty-releases, as a DRAFT.
#
# Apps up to v0.1.21 check agentty-releases for updates. v0.2.0 is the bridge: published in both places, it
# moves those apps to the new channel, and from then on they update from empty-user77/Agentty. Only v0.2.0
# goes to the old channel; it stays that channel's latest release for good, so an app that has been closed
# for months still finds its way over.
#
# The files are downloaded from the new channel, checked against its SHA256SUMS file and uploaded unchanged.
# Publishing the draft ships an update to every v0.1.x user — only with the user's explicit OK:
#   gh release edit vX.Y.Z -R empty-user77/agentty-releases --draft=false --latest
set -euo pipefail

VERSION="${1:?usage: mirror-release-to-legacy.sh <version>}"
VERSION="${VERSION#v}"
TAG="v$VERSION"
SOURCE="${AGENTTY_RELEASE_REPO:-empty-user77/Agentty}"
LEGACY="${AGENTTY_LEGACY_RELEASE_REPO:-empty-user77/agentty-releases}"

die() { echo "✗ $*" >&2; exit 1; }

gh release view "$TAG" --repo "$SOURCE" --json isDraft --jq .isDraft >/dev/null 2>&1 \
  || die "$TAG does not exist on $SOURCE yet"
if gh release view "$TAG" --repo "$LEGACY" >/dev/null 2>&1; then
  die "$TAG already exists on $LEGACY (delete that draft first if it is incomplete)"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "Downloading $TAG from $SOURCE…"
gh release download "$TAG" --repo "$SOURCE" --dir "$tmp"

sums="$tmp/Agentty-$VERSION-SHA256SUMS.txt"
[[ -f "$sums" ]] || die "Agentty-$VERSION-SHA256SUMS.txt is missing from the $SOURCE release"
(cd "$tmp" && shasum -a 256 -c "$(basename "$sums")") || die "a file does not match SHA256SUMS"
listed="$(awk '{print $2}' "$sums" | sed 's/^\*//' | sort)"
present="$(cd "$tmp" && ls | grep -v -- "-SHA256SUMS.txt$" | sort)"
[[ "$listed" == "$present" ]] || die "the release's files and its SHA256SUMS list differ"

notes="$tmp/notes.md"
gh release view "$TAG" --repo "$SOURCE" --json body --jq .body > "$notes"
{
  echo
  echo "---"
  echo "Agentty now publishes its releases at https://github.com/$SOURCE/releases. This version moves"
  echo "automatic updates there; later versions are published only in the new place."
} >> "$notes"

echo "Creating a draft $TAG on $LEGACY…"
gh release create "$TAG" --repo "$LEGACY" --draft --title "Agentty $TAG" --notes-file "$notes" >/dev/null
(cd "$tmp" && gh release upload "$TAG" --repo "$LEGACY" $(ls | grep -v '^notes\.md$'))

echo "✓ Draft $TAG on $LEGACY with $(echo "$present" | wc -l | tr -d ' ') files and their checksums."
echo "  Verify:  AGENTTY_RELEASE_REPO=$LEGACY scripts/verify-release.sh $VERSION"
echo "  Publish (ships an update to every v0.1.x user — only with the user's OK):"
echo "  gh release edit $TAG -R $LEGACY --draft=false --latest"
