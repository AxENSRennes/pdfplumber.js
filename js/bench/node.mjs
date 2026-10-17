#!/usr/bin/env node
// The result-encoding benchmark in Node (see encoding-bench.mjs).
//   node js/bench/node.mjs [CORPUS.pjb] [--passes N]
// CORPUS defaults to $PDFPLUMBER_ROOT/.cache/bench/pages_real.pjb (written by
// `cargo run --release -p pdfplumber-wasm --example bench_corpus -- FILE`).
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const args = process.argv.slice(2);
const passes = args.includes("--passes") ? Number(args[args.indexOf("--passes") + 1]) : 5;
const corpusPath =
  args.find((a) => a.endsWith(".pjb")) ??
  join(process.env.PDFPLUMBER_ROOT ?? ROOT, ".cache/bench/pages_real.pjb");

await import(join(ROOT, "dist/src/index.js"));
const lib = await import(join(ROOT, "dist/src/internal.js"));
const { runEncodingBench } = await import("./encoding-bench.mjs");
const out = await runEncodingBench(lib, new Uint8Array(readFileSync(corpusPath)), {
  passes,
  log: (m) => process.stderr.write(`${m}\n`),
});
console.log(JSON.stringify({ runtime: `Node ${process.versions.node}`, ...out }, null, 1));
