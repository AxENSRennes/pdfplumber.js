#!/usr/bin/env node
// Builds dist/pdfplumber.wasm: crates/wasm for wasm32-unknown-unknown with
// the `dist` profile (LTO, one codegen unit, panic=abort), then wasm-opt
// (binaryen, from npm) when installed; `--no-opt` skips it
// (docs/js-runtime.md, "Taille").
//
//   node scripts/build-wasm.mjs [--no-opt]
import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const home = process.env.HOME ?? "";
const cargo = existsSync(join(home, ".cargo/bin/cargo")) ? join(home, ".cargo/bin/cargo") : "cargo";
execFileSync(cargo, ["build", "--target", "wasm32-unknown-unknown", "--profile", "dist", "-p", "pdfplumber-wasm"], {
  cwd: ROOT,
  stdio: "inherit",
  env: { ...process.env, CARGO_BUILD_JOBS: process.env.CARGO_BUILD_JOBS ?? "2" },
});
const built = join(ROOT, "target/wasm32-unknown-unknown/dist/pdfplumber_wasm.wasm");
const out = join(ROOT, "dist/pdfplumber.wasm");
mkdirSync(dirname(out), { recursive: true });
const wasmOpt = join(ROOT, "node_modules/binaryen/bin/wasm-opt");
if (!process.argv.includes("--no-opt") && existsSync(wasmOpt)) {
  execFileSync(
    process.execPath,
    [wasmOpt, "-O3", "--strip-debug", "--strip-producers", "--all-features", built, "-o", out],
    { stdio: "inherit" },
  );
} else {
  copyFileSync(built, out);
}
console.log(`dist/pdfplumber.wasm: ${statSync(out).size} bytes (cargo: ${statSync(built).size})`);
