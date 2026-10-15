#!/usr/bin/env node
// Cargo runner for the wasm32-wasip1 target: runs a test (or any) binary
// under Node's WASI with the host file system mapped at the same paths, so
// the unit tests find the oracle data (`PDFPLUMBER_ROOT`, `.cache/`) exactly
// as natively. Used by scripts/check.sh:
//
//   CARGO_TARGET_WASM32_WASIP1_RUNNER="node scripts/wasi-runner.mjs" \
//     cargo test --target wasm32-wasip1 ...
//
// It proves that the Rust core gives the same results compiled to
// WebAssembly: 32-bit usize, no host libm (f64::powf, f64::mul_add come from
// the Wasm toolchain), the browser's arithmetic.
import { readFile } from "node:fs/promises";
import { argv, env, exit } from "node:process";
import { WASI } from "node:wasi";

const [wasmPath, ...args] = argv.slice(2);
if (!wasmPath) {
  console.error("usage: wasi-runner.mjs FILE.wasm [ARGS...]");
  exit(2);
}
const wasi = new WASI({
  version: "preview1",
  args: [wasmPath, ...args],
  env,
  preopens: { "/": "/" },
  returnOnExit: true,
});
const module = await WebAssembly.compile(await readFile(wasmPath));
const instance = await WebAssembly.instantiate(module, wasi.getImportObject());
exit(wasi.start(instance));
