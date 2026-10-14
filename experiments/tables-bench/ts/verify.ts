// Checks an implementation against the Python oracle, job by job.
// Usage: npx tsx ts/verify.ts [ts|wasm] [--corpus corpus|fuzz] [--show N] [--no-ints]
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { readCorpus, serialize } from "./canon.ts";
import { findTables } from "./table.ts";

const here = dirname(fileURLToPath(import.meta.url));
const out = join(here, "..", "out");
const impl = process.argv[2] ?? "ts";
const arg = (name: string) => {
  const i = process.argv.indexOf(name);
  return i > 0 ? process.argv[i + 1] : undefined;
};
const corpus = arg("--corpus") ?? "corpus";
const show = arg("--show");
const noInts = process.argv.includes("--no-ints");

const jobs = readCorpus(new Uint8Array(readFileSync(join(out, `${corpus}.bin`))));
const oracle: Array<{ sha256: string }> = JSON.parse(readFileSync(join(out, `${corpus}_oracle.json`), "utf8"));
if (noInts) for (const job of jobs) for (const e of job.edges) e.ints = 0; // JS numbers only, no int/float distinction

let trace: (i: number) => string;
if (impl === "wasm") {
  const { loadWasmTables } = await import("./wasm.ts");
  const wasm = await loadWasmTables(readFileSync(join(here, "..", "rust", "tables.wasm")));
  trace = (i) => wasm.trace(jobs[i].raw);
} else {
  trace = (i) => serialize(findTables(jobs[i].edges, jobs[i].settings));
}

if (show !== undefined) {
  process.stdout.write(trace(Number(show)));
  process.exit(0);
}
let ok = 0;
const failures: number[] = [];
for (let i = 0; i < jobs.length; i += 1) {
  let text: string;
  try {
    text = trace(i);
  } catch (e) {
    text = `!error:${(e as Error).message}\n`;
  }
  if (createHash("sha256").update(text).digest("hex") === oracle[i].sha256) ok += 1;
  else failures.push(i);
}
console.log(`${impl}${noInts ? " (sans distinction int/float)" : ""} [${corpus}]: ${ok}/${jobs.length} jobs identiques a pdfplumber`);
if (failures.length) console.log("  echecs:", failures.slice(0, 20).join(" "));
process.exit(failures.length ? 1 : 0);
