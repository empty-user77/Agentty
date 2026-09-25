#!/usr/bin/env bash
#
# update-homebrew-cask.sh <version> [--check] — points the Homebrew cask at a published release.
#
# The cask lives in the tap empty-user77/homebrew-agentty (Casks/agentty.rb), so users install with
#   brew install --cask empty-user77/agentty/agentty
# It installs the notarized app zip of the release; its sha256 comes from the release's published SHA256SUMS (which
# verify-release.sh --published checked against the downloaded files). Run it after the release is published — a
# draft's assets are not downloadable, and the cask must never point at a release users cannot get yet.
#
#   scripts/update-homebrew-cask.sh 0.1.22           # write the cask and commit it to the tap
#   scripts/update-homebrew-cask.sh 0.1.22 --check   # only check the tap serves 0.1.22 with the right sha256
set -euo pipefail

VERSION="${1:?usage: update-homebrew-cask.sh <version> [--check]}"
VERSION="${VERSION#v}"
CHECK="false"
[[ "${2:-}" == "--check" ]] && CHECK="true"
RELEASE_REPO="${AGENTTY_RELEASE_REPO:-empty-user77/Agentty}"
TAP_REPO="${AGENTTY_TAP_REPO:-empty-user77/homebrew-agentty}"
CASK_PATH="Casks/agentty.rb"
ZIP="Agentty-$VERSION-arm64.zip"

die() { printf "\033[1;31m[error]\033[0m %s\n" "$*" >&2; exit 1; }
ok()  { printf "\033[1;32m[  ok ]\033[0m %s\n" "$*"; }

[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "version must be X.Y.Z, got '$VERSION'"

json="$(gh release view "v$VERSION" -R "$RELEASE_REPO" --json isDraft,isPrerelease 2>/dev/null)" \
  || die "release v$VERSION not found on $RELEASE_REPO"
[[ "$(jq -r '.isDraft or .isPrerelease' <<<"$json")" == "false" ]] \
  || die "v$VERSION is a draft or pre-release — publish it before pointing Homebrew at it"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
gh release download "v$VERSION" -R "$RELEASE_REPO" -p "Agentty-$VERSION-SHA256SUMS.txt" -D "$tmp" >/dev/null 2>&1 \
  || die "could not download Agentty-$VERSION-SHA256SUMS.txt"
sha="$(awk -v f="$ZIP" '$2 == f || $2 == "*"f { print $1 }' "$tmp/Agentty-$VERSION-SHA256SUMS.txt")"
[[ "$sha" =~ ^[0-9a-f]{64}$ ]] || die "$ZIP is not listed in the release's SHA256SUMS"

current="$(gh api "repos/$TAP_REPO/contents/$CASK_PATH" 2>/dev/null || true)"

if [[ "$CHECK" == "true" ]]; then
  [[ -n "$current" ]] || die "$TAP_REPO has no $CASK_PATH"
  body="$(jq -r .content <<<"$current" | base64 --decode)"
  grep -q "^  version \"$VERSION\"$" <<<"$body" || die "the Homebrew cask is not at $VERSION"
  grep -q "^  sha256 \"$sha\"$" <<<"$body" || die "the Homebrew cask's sha256 does not match $ZIP"
  ok "Homebrew cask serves $VERSION ($ZIP)"
  exit 0
fi

cat > "$tmp/agentty.rb" <<EOF
cask "agentty" do
  version "$VERSION"
  sha256 "$sha"

  url "https://github.com/$RELEASE_REPO/releases/download/v#{version}/Agentty-#{version}-arm64.zip"
  name "Agentty"
  desc "Terminal for AI coding agents"
  homepage "https://www.agentty.run/"

  livecheck do
    url :url
    strategy :github_latest
  end

  auto_updates true
  depends_on arch: :arm64
  depends_on macos: :ventura

  app "Agentty.app"

  # ~/.agentty is left alone on purpose: it holds the agents' git worktrees, which may contain unsaved work.
  zap trash: [
    "~/Library/Preferences/run.agentty.app.plist",
    "~/Library/Saved Application State/run.agentty.app.savedState",
  ]
end
EOF

args=(-X PUT "repos/$TAP_REPO/contents/$CASK_PATH" -f message="agentty $VERSION" -f content="$(base64 < "$tmp/agentty.rb" | tr -d '\n')")
if [[ -n "$current" ]]; then
  [[ "$(jq -r .content <<<"$current" | base64 --decode)" == "$(cat "$tmp/agentty.rb")" ]] && { ok "Homebrew cask already at $VERSION"; exit 0; }
  args+=(-f sha="$(jq -r .sha <<<"$current")")
fi
gh api "${args[@]}" >/dev/null || die "could not commit $CASK_PATH to $TAP_REPO (does the tap exist, and can this gh account push to it?)"
ok "Homebrew cask updated to $VERSION — brew install --cask ${TAP_REPO%%/*}/agentty/agentty"
