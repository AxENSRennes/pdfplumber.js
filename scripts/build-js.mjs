#!/usr/bin/env node
// Builds the JavaScript package (docs/js-runtime.md, "Build"):
//   dist/src/      Node ESM + type declarations (tsc, js/tsconfig.json)
//   dist/browser/  browser ESM bundles (esbuild): index.js (public API),
//                  worker.js (worker mode), internal.js (tests, benchmark),
//                  sharing their chunks
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

await build({
  entryPoints: {
    index: join(ROOT, "js/src/browser.ts"),
    worker: join(ROOT, "js/src/worker/worker.ts"),
    internal: join(ROOT, "js/src/browser-internal.ts"),
  },
  outdir: join(ROOT, "dist/browser"),
  bundle: true,
  splitting: true,
  format: "esm",
  platform: "browser",
  target: ["es2022"],
  minify: !process.argv.includes("--no-minify"),
  legalComments: "none",
  logLevel: "warning",
});
console.log("built dist/src and dist/browser");
