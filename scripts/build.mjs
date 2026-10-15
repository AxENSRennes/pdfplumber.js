#!/usr/bin/env node
// The whole package in dist/: WebAssembly module, CMaps, JavaScript.
import { execFileSync } from "node:child_process";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
for (const script of ["build-wasm.mjs", "build-cmaps.mjs", "build-js.mjs"])
  execFileSync(process.execPath, [join(ROOT, "scripts", script)], { stdio: "inherit" });
