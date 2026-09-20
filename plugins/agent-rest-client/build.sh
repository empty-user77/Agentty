#!/usr/bin/env bash
# Builds the plugin and puts the module where agentty-plugin.json says it is.
set -euo pipefail
cd "$(dirname "$0")"

rustup target add wasm32-unknown-unknown >/dev/null
cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/agent_rest_client.wasm agent-rest-client.wasm
# A module is data, not a program to run: cargo's output is executable, the committed file is not.
chmod 644 agent-rest-client.wasm

printf 'agent-rest-client.wasm  %s bytes\n' "$(wc -c < agent-rest-client.wasm | tr -d ' ')"
echo 'Install it in Agentty: Plugins → Install from folder → pick this folder.'
