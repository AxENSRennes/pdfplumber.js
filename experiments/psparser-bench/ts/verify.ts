// Checks a parser implementation against the Python oracle, job by job.
// Usage: npx tsx ts/verify.ts [ts|wasm] [--corpus corpus|fuzz] [--show N]
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { parseJobTs, readCorpus, serializeJob } from "./canon.ts";

const here = dirname(fileURLToPath(import.meta.url));
const out = join(here, "..", "out");
const impl = process.argv[2] ?? "ts";
const corpusIdx = process.argv.indexOf("--corpus");
const corpus = corpusIdx > 0 ? process.argv[corpusIdx + 1] : "corpus";
const showIdx = process.argv.indexOf("--show");
const show = showIdx > 0 ? Number(process.argv[showIdx + 1]) : -1;

const jobs = readCorpus(new Uint8Array(readFileSync(join(out, `${corpus}.bin`))));
const oracle: Array<{ id: string; sha256: string; outcome: string }> = JSON.parse(readFileSync(join(out, corpus === "corpus" ? "oracle.json" : `${corpus}_oracle.json`), "utf8"));
const run = impl === "wasm" ? (await (await import("./wasm.ts")).loadWasmParser(readFileSync(join(here, "..", "rust", "psparser.wasm")))).parseJobDecoded : parseJobTs;

if (show >= 0) {
  process.stdout.write(serializeJob(jobs[show], run));
  process.exit(0);
}

let ok = 0;
const failures: string[] = [];
for (let i = 0; i < jobs.length; i += 1) {
  const text = serializeJob(jobs[i], run);
  const sha = createHash("sha256").update(text).digest("hex");
  if (sha === oracle[i].sha256) ok += 1;
  else failures.push(`${i} ${oracle[i].id} (python outcome ${oracle[i].outcome})`);
}
console.log(`${impl} [${corpus}]: ${ok}/${jobs.length} jobs byte-identical to pdfminer`);
for (const f of failures.slice(0, 20)) console.log("  FAIL", f);
process.exit(failures.length ? 1 : 0);
