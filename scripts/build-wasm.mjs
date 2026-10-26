#!/usr/bin/env node
// Builds dist/pdfplumber.wasm: crates/wasm for wasm32-unknown-unknown with
// the `dist` profile (LTO, one codegen unit, panic=abort), then wasm-opt
// (binaryen, from npm) when installed; `--no-opt` skips it
// (docs/js-runtime.md, "Taille").
//
// `--oracle` builds dist/pdfplumber-oracle.wasm instead: the same module
// with the oracle hooks of the harness (feature `oracle-hooks`,
// crates/wasm/src/oracle.rs; docs/architecture.md, "Vague 5"), in its own
// target directory; never shipped (package.json "files").
//
//   node scripts/build-wasm.mjs [--no-opt] [--oracle]
import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const oracle = process.argv.includes("--oracle");
const home = process.env.HOME ?? "";
const cargo = existsSync(join(home, ".cargo/bin/cargo")) ? join(home, ".cargo/bin/cargo") : "cargo";
const targetDir = oracle ? join(ROOT, "target/oracle") : join(ROOT, "target");
const args = ["build", "--target", "wasm32-unknown-unknown", "--profile", "dist", "-p", "pdfplumber-wasm"];
if (oracle) args.push("--features", "oracle-hooks", "--target-dir", targetDir);
execFileSync(cargo, args, {
  cwd: ROOT,
  stdio: "inherit",
  env: { ...process.env, CARGO_BUILD_JOBS: process.env.CARGO_BUILD_JOBS ?? "2" },
});
const built = join(targetDir, "wasm32-unknown-unknown/dist/pdfplumber_wasm.wasm");
const name = oracle ? "dist/pdfplumber-oracle.wasm" : "dist/pdfplumber.wasm";
const out = join(ROOT, name);
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
console.log(`${name}: ${statSync(out).size} bytes (cargo: ${statSync(built).size})`);
