// Usage: npx tsx ts/bench-node.ts
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { runBench } from "./bench-core.ts";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..");
const corpus = new Uint8Array(readFileSync(join(root, "out", "corpus.bin")));
const wasm = readFileSync(join(root, "rust", "psparser.wasm"));
const oracle: Array<{ python_s: number; bytes: number }> = JSON.parse(readFileSync(join(root, "out", "oracle.json"), "utf8"));

const result = await runBench(`node ${process.version}`, corpus, wasm.buffer.slice(wasm.byteOffset, wasm.byteOffset + wasm.byteLength));
const pythonS = oracle.reduce((a, r) => a + r.python_s, 0);
const out = { ...result, python: { totalMs: pythonS * 1000, mbPerS: result.mb / pythonS } };
writeFileSync(join(root, "out", "bench-node.json"), JSON.stringify(out, null, 2));
console.log(JSON.stringify(out, null, 2));
