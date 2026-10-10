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

## Bytecode decoder and provenance

The bytecode path uses [`PumbaaDev/luau-decompiler`](https://github.com/PumbaaDev/luau-decompiler), pinned to commit `ee07098c31e11601a9ca434bc12932a4ded65f7b`, through its `luau-core` Rust library. The project is MIT-licensed; see the upstream `LICENSE`. The real 200-byte `p03_unary.luac` test fixture is stored as portable hex in `tests/fixtures/p03_unary.luac.hex` and decoded in the test suite. Tests assert the bytecode version, successful decompilation, non-empty output, JSON response integration, and rejection of malformed input.

This is a real decoder integration, not an original decompiler implementation from scratch. It is deterministic, offline at runtime, and does not call an AI API. It supports the upstream core’s stated Roblox Luau bytecode v3–v9 scope; obfuscated opcode shuffles, VM wrappers, and other formats may require additional format-specific work.

## Architecture
- src/lib.rs: Rust engine with a small C-style WebAssembly ABI.
- engine-worker.js: isolates analysis from the page UI.
- app.js and index.html: interface and result display.
- Deploy with Cloudflare Pages, using this directory as root and public as output directory.

## Precision contract
The engine never executes the analyzed script, guesses missing payloads, or labels partial output as a complete decompilation. The first release is the base for adding a proper tokenizer/parser, control-flow analysis, constant propagation, and format-specific bytecode decoders backed by fixtures and tests.
