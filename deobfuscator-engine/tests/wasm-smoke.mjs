import assert from "node:assert/strict";
import fs from "node:fs";

const wasmPath = process.argv[2] || "deobfuscator-engine/target/wasm32-unknown-unknown/release/deobfuscator_engine.wasm";
const fixturePath = process.argv[3] || "deobfuscator-engine/tests/fixtures/p03_unary.luac.hex";
const wasm = fs.readFileSync(wasmPath);
const hex = fs.readFileSync(fixturePath, "utf8").replace(/\s+/g, "");
const fixture = Buffer.from(hex, "hex");
assert.equal(fixture.length, 200, "real bytecode fixture length should remain stable");

const { instance } = await WebAssembly.instantiate(wasm, {});
const api = instance.exports;
assert.ok(api.memory && api.alloc && api.dealloc && api.engine_run && api.result_len && api.free_result,
  "Rust WASM ABI exports are present");

function run(input, mode) {
  const ptr = api.alloc(input.length);
  new Uint8Array(api.memory.buffer, ptr, input.length).set(input);
  let outputPtr = 0;
  let outputLen = 0;
  try {
    outputPtr = api.engine_run(ptr, input.length, mode);
    outputLen = api.result_len();
    const raw = new Uint8Array(api.memory.buffer, outputPtr, outputLen).slice();
    return JSON.parse(new TextDecoder().decode(raw));
  } finally {
    if (outputPtr && outputLen) api.free_result(outputPtr, outputLen);
    api.dealloc(ptr, input.length);
  }
}

const bytecodeResult = run(fixture, 1);
assert.equal(bytecodeResult.ok, true);
assert.equal(bytecodeResult.mode, "bytecode");
assert.equal(bytecodeResult.metrics.bytecode_version, "6");
assert.ok(bytecodeResult.output.length > 0, "real bytecode should produce recovered source");
assert.ok(/function|return|local/.test(bytecodeResult.output), "output should look like recovered Luau, not a header report");

const sourceResult = run(Buffer.from("return 42 -- sample\n", "utf8"), 0);
assert.equal(sourceResult.ok, true);
assert.equal(sourceResult.mode, "source");
assert.ok(sourceResult.output.includes("return 42"));
console.log("WASM smoke test passed: real Luau bytecode decompilation + source normalization.");
