# Deobfuscator Lab — Rust Engine v1

A conservative, deterministic Lua/Luau static-analysis core compiled to WebAssembly and hosted as a Cloudflare Pages site. No external AI inference or source-code execution is used.

## V1 behavior
- Removes line and long comments while preserving line breaks.
- Normalizes quoted strings when the supported Lua escape sequences can be decoded without ambiguity.
- Converts long-bracket strings to escaped quoted strings while preserving Lua's initial-newline rule.
- Preserves malformed/unknown escapes verbatim and emits warnings.
- Checks matching parentheses, square brackets, and braces; this is not a full parser or proof of syntax validity.
- Reports heuristic source indicators without claiming that obfuscation was proven.
- Inspects recognizable Lua 5.1–5.4 bytecode signatures and an RSB1 marker. V1 does not disassemble instructions, recover constants/functions, or decompile Lua/Luau bytecode.

## Build
Run bash build.sh from this directory. The script installs a minimal Rust toolchain if needed, runs unit tests, compiles wasm32-unknown-unknown, and outputs static assets in public/.

## Architecture
- src/lib.rs: Rust engine with a small C-style WebAssembly ABI.
- engine-worker.js: isolates analysis from the page UI.
- app.js and index.html: interface and result display.
- Deploy with Cloudflare Pages, using this directory as root and public as output directory.

## Precision contract
The engine never executes the analyzed script, guesses missing payloads, or labels partial output as a complete decompilation. The first release is the base for adding a proper tokenizer/parser, control-flow analysis, constant propagation, and format-specific bytecode decoders backed by fixtures and tests.
