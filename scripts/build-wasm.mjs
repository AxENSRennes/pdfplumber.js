#!/usr/bin/env node
// Builds dist/pdfplumber.wasm: crates/wasm for wasm32-unknown-unknown with
// the `dist` profile (LTO, one codegen unit, panic=abort), then wasm-opt
// (binaryen, a devDependency); `--no-opt` skips it (docs/js-runtime.md,
// "Size"). Without `--no-opt`, a missing wasm-opt is an error: a stale
// node_modules (before `npm ci`) would ship the unoptimized module
// (3.2 MB instead of 2.6: the function names included) without a word,
// and the size budget, counted in brotli bytes, does not catch it.
//
// `-O3` without binaryen's own inlining (`NO_INLINING`; docs/js-runtime.md,
// "Size"): LLVM has already inlined (opt-level 3, LTO); inlining again made
// the module 0.6 % smaller raw but 2.9 % larger in brotli and 2.1 % in gzip,
// the bytes a page downloads, and no faster in Node, Chromium, WebKit or
// Firefox, whose optimizing tiers inline themselves.
//
// `--oracle` builds dist/pdfplumber-oracle.wasm instead: the same module
// with the oracle hooks of the harness (feature `oracle-hooks`,
// crates/wasm/src/oracle.rs; docs/verification.md,
// "Parity Through The JavaScript API"), in its own target directory; never
// shipped (package.json "files").
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
const optimize = !process.argv.includes("--no-opt");
// no function is small enough, or alone enough to its caller, to be inlined
const NO_INLINING = [
  "--always-inline-max-function-size=0",
  "--flexible-inline-max-function-size=0",
  "--one-caller-inline-max-function-size=0",
];
if (optimize && !existsSync(wasmOpt)) {
  console.error(
    `build-wasm: ${wasmOpt} is missing (binaryen, a devDependency): run \`npm ci --offline\`, ` +
      "or pass --no-opt for an unoptimized module",
  );
  process.exit(1);
}
if (optimize) {
  execFileSync(
    process.execPath,
    [wasmOpt, "-O3", ...NO_INLINING, "--strip-debug", "--strip-producers", "--all-features", built, "-o", out],
    { stdio: "inherit" },
  );
} else {
  copyFileSync(built, out);
}
console.log(
  `${name}: ${statSync(out).size} bytes (cargo: ${statSync(built).size}${optimize ? ", wasm-opt -O3 without inlining" : ", --no-opt"})`,
);
