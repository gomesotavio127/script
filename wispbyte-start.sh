#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
ENGINE_DIR="$ROOT/deobfuscator-engine"
PUBLIC_DIR="$ENGINE_DIR/public"

if [[ ! -s "$PUBLIC_DIR/engine.wasm" || ! -s "$PUBLIC_DIR/index.html" ]]; then
  echo "[WispByte] Build do motor Rust/WebAssembly iniciado."

  if ! command -v curl >/dev/null 2>&1; then
    echo "[WispByte] curl não está disponível na imagem Node.js; não é possível instalar Rust automaticamente." >&2
    exit 1
  fi

  if ! command -v cargo >/dev/null 2>&1 || ! command -v rustup >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
  fi

  if [[ -f "$HOME/.cargo/env" ]]; then
    # shellcheck disable=SC1091
    source "$HOME/.cargo/env"
  fi

  if ! command -v cargo >/dev/null 2>&1 || ! command -v rustup >/dev/null 2>&1; then
    echo "[WispByte] cargo/rustup não ficou disponível depois da instalação." >&2
    exit 1
  fi

  rustup target add wasm32-unknown-unknown

  (
    cd "$ENGINE_DIR"
    # A compilação serial e sem LTO reduz o pico de memória em instâncias pequenas.
    export CARGO_BUILD_JOBS=1
    export CARGO_PROFILE_RELEASE_LTO=false
    export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16
    cargo build --manifest-path Cargo.toml --target wasm32-unknown-unknown --release --jobs 1
  )

  mkdir -p "$PUBLIC_DIR"
  cp "$ENGINE_DIR/index.html" "$ENGINE_DIR/app.js" "$ENGINE_DIR/engine-worker.js" "$PUBLIC_DIR/"
  cp "$ENGINE_DIR/target/wasm32-unknown-unknown/release/deobfuscator_engine.wasm" "$PUBLIC_DIR/engine.wasm"
  echo "[WispByte] Build concluído: $(wc -c < "$PUBLIC_DIR/engine.wasm") bytes de WASM."
else
  echo "[WispByte] Reutilizando os arquivos WebAssembly já compilados."
fi

exec node "$ROOT/wispbyte-server.js"
