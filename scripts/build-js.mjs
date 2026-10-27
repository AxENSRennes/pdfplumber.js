#!/usr/bin/env node
// Builds the JavaScript package (docs/js-runtime.md, "JavaScript Package"):
//   dist/src/      Node ESM + type declarations (tsc, js/tsconfig.json)
//   dist/browser/  browser ESM bundles (esbuild): index.js (public API) and
//                  internal.js (tests, benchmark), sharing their chunks;
//                  worker.js (worker mode), one file of its own
// The WebAssembly module (scripts/build-wasm.mjs) and the CMaps
// (scripts/build-cmaps.mjs) go next to them, in dist/.
//
//   node scripts/build-js.mjs [--no-minify]
import { execFileSync } from "node:child_process";
import { rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");

rmSync(join(ROOT, "dist/src"), { recursive: true, force: true });
rmSync(join(ROOT, "dist/browser"), { recursive: true, force: true });
execFileSync(process.execPath, [join(ROOT, "node_modules/typescript/bin/tsc"), "-p", join(ROOT, "js/tsconfig.json")], {
  stdio: "inherit",
});

const common = {
  outdir: join(ROOT, "dist/browser"),
  bundle: true,
  format: "esm",
  platform: "browser",
  target: ["es2022"],
  minify: !process.argv.includes("--no-minify"),
  legalComments: "none",
  logLevel: "warning",
};
await build({
  ...common,
  entryPoints: { index: join(ROOT, "js/src/browser.ts"), internal: join(ROOT, "js/src/browser-internal.ts") },
  splitting: true,
});
// the worker on its own, without shared chunks: one file, which an
// application can copy next to its bundle (docs/js-runtime.md,
// "JavaScript Package")
await build({ ...common, entryPoints: { worker: join(ROOT, "js/src/worker/worker.ts") }, splitting: false });
console.log("built dist/src and dist/browser");
