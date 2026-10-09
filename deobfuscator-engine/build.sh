#!/usr/bin/env bash
set -euo pipefail

# Some hosted builders provide cargo but do not create ~/.cargo/env.
if ! command -v cargo >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
  export PATH="$HOME/.cargo/bin:$PATH"
fi

if [ -f "$HOME/.cargo/env" ]; then
  # shellcheck disable=SC1091
  source "$HOME/.cargo/env"
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "Rust cargo was not found after toolchain setup." >&2
  exit 1
fi

if ! command -v rustup >/dev/null 2>&1; then
  echo "rustup is required to install the WebAssembly target." >&2
  exit 1
fi

rustup target add wasm32-unknown-unknown
cargo test --manifest-path Cargo.toml
cargo build --manifest-path Cargo.toml --target wasm32-unknown-unknown --release

mkdir -p public
cp index.html app.js engine-worker.js public/
cp target/wasm32-unknown-unknown/release/deobfuscator_engine.wasm public/engine.wasm
printf 'Built engine.wasm (%s bytes)\n' "$(wc -c < public/engine.wasm)"
