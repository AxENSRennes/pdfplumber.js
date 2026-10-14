// Usage: npx tsx ts/bench-node.ts
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { runBench } from "./bench-core.ts";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const load = (name: string) => new Uint8Array(readFileSync(join(root, "out", `${name}.bin`)));
const wasm = readFileSync(join(root, "rust", "tables.wasm"));
const result = await runBench(`node ${process.version}`, { corpus: load("corpus"), fuzz: load("fuzz") }, wasm.buffer.slice(wasm.byteOffset, wasm.byteOffset + wasm.byteLength));
const python = Object.fromEntries(
  ["corpus", "fuzz"].map((name) => {
    const oracle: Array<{ python_s: number }> = JSON.parse(readFileSync(join(root, "out", `${name}_oracle.json`), "utf8"));
    return [name, oracle.reduce((a, r) => a + r.python_s, 0) * 1000];
  })
);
const out = { ...result, pythonMs: python };
writeFileSync(join(root, "out", "bench-node.json"), JSON.stringify(out, null, 2));
for (const [name, c] of Object.entries(out.corpora)) {
  console.log(`${name}: ${c.jobs} pages, ${c.edges} bords, ${c.tables} tables | python ${Math.round(python[name])} ms`);
  for (const [mode, m] of Object.entries(c.modes)) console.log(`  ${mode.padEnd(48)} ${Math.round(m.medianMs)} ms  ${JSON.stringify(m.runsMs)}`);
}
