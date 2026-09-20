#!/usr/bin/env bash
# Builds the plugin and puts the module where agentty-plugin.json says it is.
set -euo pipefail
cd "$(dirname "$0")"

rustup target add wasm32-unknown-unknown >/dev/null
cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/blogger_agentos.wasm blogger-agentos.wasm
chmod 644 blogger-agentos.wasm

printf 'blogger-agentos.wasm  %s bytes\n' "$(wc -c < blogger-agentos.wasm | tr -d ' ')"
echo 'Install it in Agentty: Plugins → Install from folder → pick this folder.'
