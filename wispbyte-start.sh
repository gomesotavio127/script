#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
ENGINE_DIR="$ROOT/deobfuscator-engine"
PUBLIC_DIR="$ENGINE_DIR/public"
ASSET_BASE="${DEOBFUSCATOR_ASSETS_URL:-https://deobfuscator-rust-engine.vercel.app}"

mkdir -p "$PUBLIC_DIR"

# Prefer precompiled assets to avoid compiling Rust on a small free container.
if [[ ! -s "$PUBLIC_DIR/engine.wasm" || ! -s "$PUBLIC_DIR/index.html" || ! -s "$PUBLIC_DIR/app.js" || ! -s "$PUBLIC_DIR/engine-worker.js" ]]; then
  if command -v curl >/dev/null 2>&1; then
    TMP_DIR="$(mktemp -d)"
    echo "[WispByte] Tentando obter os assets pré-compilados..."
    if curl -fLsS --connect-timeout 10 --max-time 90 "$ASSET_BASE/index.html" -o "$TMP_DIR/index.html" &&
       curl -fLsS --connect-timeout 10 --max-time 90 "$ASSET_BASE/app.js" -o "$TMP_DIR/app.js" &&
       curl -fLsS --connect-timeout 10 --max-time 90 "$ASSET_BASE/engine-worker.js" -o "$TMP_DIR/engine-worker.js" &&
       curl -fLsS --connect-timeout 10 --max-time 120 "$ASSET_BASE/engine.wasm" -o "$TMP_DIR/engine.wasm" &&
       [[ -s "$TMP_DIR/index.html" && -s "$TMP_DIR/app.js" && -s "$TMP_DIR/engine-worker.js" && -s "$TMP_DIR/engine.wasm" ]]; then
      cp "$TMP_DIR/index.html" "$TMP_DIR/app.js" "$TMP_DIR/engine-worker.js" "$TMP_DIR/engine.wasm" "$PUBLIC_DIR/"
      echo "[WispByte] Assets pré-compilados obtidos com sucesso."
    else
      echo "[WispByte] Downloads indisponíveis; o script tentará compilar Rust localmente."
    fi
    rm -rf "$TMP_DIR"
  fi
fi

if [[ ! -s "$PUBLIC_DIR/engine.wasm" || ! -s "$PUBLIC_DIR/index.html" || ! -s "$PUBLIC_DIR/app.js" || ! -s "$PUBLIC_DIR/engine-worker.js" ]]; then
  echo "[WispByte] Build local Rust/WebAssembly iniciado."
  if ! command -v curl >/dev/null 2>&1; then
    echo "[WispByte] curl não está disponível e não há assets pré-compilados." >&2
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
    export CARGO_BUILD_JOBS=1
    export CARGO_PROFILE_RELEASE_LTO=false
    export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16
    cargo build --manifest-path Cargo.toml --target wasm32-unknown-unknown --release --jobs 1
  )

  cp "$ENGINE_DIR/index.html" "$ENGINE_DIR/app.js" "$ENGINE_DIR/engine-worker.js" "$PUBLIC_DIR/"
  cp "$ENGINE_DIR/target/wasm32-unknown-unknown/release/deobfuscator_engine.wasm" "$PUBLIC_DIR/engine.wasm"
  echo "[WispByte] Build local concluído: $(wc -c < "$PUBLIC_DIR/engine.wasm") bytes de WASM."
fi

exec node "$ROOT/wispbyte-server.js"
