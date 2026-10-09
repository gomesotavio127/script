#!/usr/bin/env bash
set -euo pipefail

if ! command -v cargo >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
fi
source "$HOME/.cargo/env"
rustup target add wasm32-unknown-unknown
cargo test --manifest-path Cargo.toml
cargo build --manifest-path Cargo.toml --target wasm32-unknown-unknown --release

mkdir -p public
cp index.html app.js engine-worker.js public/
cp target/wasm32-unknown-unknown/release/deobfuscator_engine.wasm public/engine.wasm
printf 'Built engine.wasm (%s bytes)\n' "$(wc -c < public/engine.wasm)"
