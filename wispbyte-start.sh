#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
ENGINE_DIR="$ROOT/deobfuscator-engine"
PUBLIC_DIR="$ENGINE_DIR/public"
ASSET_BASE="${DEOBFUSCATOR_ASSETS_URL:-https://deobfuscator-rust-engine.vercel.app}"

mkdir -p "$PUBLIC_DIR"

# Always serve the UI files from this branch so interface fixes reach WispByte.
for asset in index.html app.js engine-worker.js; do
  if [[ ! -s "$ENGINE_DIR/$asset" ]]; then
    echo "[WispByte] Arquivo de interface ausente: $ENGINE_DIR/$asset" >&2
    exit 1
  fi
  cp "$ENGINE_DIR/$asset" "$PUBLIC_DIR/$asset"
done

# Reuse the published WASM engine when possible. Only download the binary;
# never overwrite the branch's current HTML or JavaScript with older Vercel files.
if [[ ! -s "$PUBLIC_DIR/engine.wasm" ]] && command -v curl >/dev/null 2>&1; then
  TMP_DIR="$(mktemp -d)"
  echo "[WispByte] Tentando obter o WebAssembly pré-compilado..."
  if curl -fLsS --connect-timeout 10 --max-time 120 "$ASSET_BASE/engine.wasm" -o "$TMP_DIR/engine.wasm" &&
     [[ -s "$TMP_DIR/engine.wasm" ]]; then
    cp "$TMP_DIR/engine.wasm" "$PUBLIC_DIR/engine.wasm"
    echo "[WispByte] WebAssembly pré-compilado obtido com sucesso."
  else
    echo "[WispByte] WASM pré-compilado indisponível; será tentada a compilação local."
  fi
  rm -rf "$TMP_DIR"
fi

if [[ ! -s "$PUBLIC_DIR/engine.wasm" ]]; then
  echo "[WispByte] Build local Rust/WebAssembly iniciado."
  if ! command -v curl >/dev/null 2>&1; then
    echo "[WispByte] curl não está disponível e não há WASM pré-compilado." >&2
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

  cp "$ENGINE_DIR/target/wasm32-unknown-unknown/release/deobfuscator_engine.wasm" "$PUBLIC_DIR/engine.wasm"
  echo "[WispByte] Build local concluído: $(wc -c < "$PUBLIC_DIR/engine.wasm") bytes de WASM."
fi

exec node "$ROOT/wispbyte-server.js"
