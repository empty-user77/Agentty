#!/usr/bin/env bash
# Builds the plugin and puts the module where agentty-plugin.json says it is.
set -euo pipefail
cd "$(dirname "$0")"

rustup target add wasm32-unknown-unknown >/dev/null
cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/hello_rust.wasm hello-rust.wasm

printf 'hello-rust.wasm  %s bytes\n' "$(wc -c < hello-rust.wasm | tr -d ' ')"
echo 'Install it in Agentty: Plugins → Install from folder → pick this folder.'
