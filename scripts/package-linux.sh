#!/usr/bin/env bash
# Builds dist/agentty-<version>-linux-<arch>.tar.gz: the release binary, a desktop entry, the icon
# and install.sh (installs into ~/.local). Run on Linux (or in CI's Linux job).
set -euo pipefail
cd "$(dirname "$0")/.."

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
arch=$(uname -m)
name="agentty-$version-linux-$arch"
stage="target/package/$name"

cargo build --release -p agentty-app
rm -rf "$stage"
mkdir -p "$stage/bin" "$stage/share/applications" "$stage/share/icons/hicolor/256x256/apps"
cp target/release/agentty "$stage/bin/agentty"
cp packaging/linux/run.agentty.Agentty.desktop "$stage/share/applications/"
cp crates/agentty-app/assets/brand/logo-256.png "$stage/share/icons/hicolor/256x256/apps/agentty.png"
cp packaging/linux/install.sh LICENSE THIRD_PARTY_NOTICES.md "$stage/"

mkdir -p dist
tar -C target/package -czf "dist/$name.tar.gz" "$name"
(cd dist && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
echo "dist/$name.tar.gz"
