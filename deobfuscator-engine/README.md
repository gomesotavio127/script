# Deobfuscator Lab — Rust Engine v1

A conservative, deterministic Lua/Luau static-analysis core compiled to WebAssembly and hosted as a Cloudflare Pages site. No external AI inference or source-code execution is used.

## V1 static source passes
- Removes line and long comments while preserving line breaks; normalizes supported quoted and long-bracket strings.
- Tokenizes source without executing it, then folds pure numeric, boolean, comparison and string-concatenation expressions.
- Statically folds bounded pure string calls: `string.char`, `string.reverse`, `string.sub`, `string.rep` and literal `table.concat`.
- Propagates simple literal local constants until reassignment or a control-flow boundary.
- Removes `if` branches only when every condition needed for the decision is statically known; removes `while false do ... end` loops.
- Reports the number of rewrites and warns when no supported transform applied. The output is explicitly partial; this is not a complete Lua parser or a full devirtualizer.
- Optional, user-triggered retrieval of public Jnkie loader URLs is handled by a strict server-side allowlist; payload text is bounded, HTML block pages are rejected, and no script is executed.
- Recognizes unsupported bytecode/container signatures and does not fabricate source.

## Bytecode
- The bytecode path uses the MIT-licensed `luau-core` Rust library to parse and decompile standard Roblox Luau bytecode versions 3–9.
- Obfuscated opcode shuffles, custom virtual machines, encrypted/compressed wrappers, and other formats require format-specific decoders and remain unsupported unless their structure is independently recovered.

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
The engine never executes the analyzed script, guesses missing payloads, or labels partial output as a complete decompilation. The explicit loader-import feature only retrieves text from a narrowly allowlisted public Jnkie download URL. The source-mode passes are intentionally bounded and conservative. They are not a replacement for a full Luau parser or an opcode-level devirtualizer; additional syntax and VM formats should be added only with tests and captured fixtures.


## No-op and protected VM behavior

The source analyzer does not return byte-for-byte unchanged input as recovered output. When no supported rewrite changes the source, the output field is empty and the result reports that no transformation was produced. Luraph v14/v15 headers, plus a conservative headerless VM-table signature, are detected explicitly. Detection is not devirtualization: this WASM backend reports the unsupported VM instead of claiming success or fabricating a recovered script.
