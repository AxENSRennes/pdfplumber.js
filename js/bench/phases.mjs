#!/usr/bin/env node
// The phases of crates/pdftrace/examples/plumber_phases.rs (and its Python
// twin plumber_phases.py) through the public API of the dist build, in
// Node (WebAssembly, every result decoded into JavaScript values): for each
// file (`path<TAB>password` lines on stdin), `open(path, {pages: [1..20],
// password})` and `await pdf.pages`, then for each page `await
// page.objects` (the interpretation, the layout and `process_object`, then
// the objects through the boundary), `extractText()`, `extractWords()`,
// `findTables()`, then `page.objects` again (cached in the core: the cost
// of the boundary alone, encoding, copy and decoding of the page's
// objects). Writes, per file, `path pages open objects text words tables
// objects-again` (seconds), then a summary line `# total ...` with the
// peak RSS of the process and the size of the module's linear memory.
//
//   node js/bench/phases.mjs [--dist DIR] < list.tsv
import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const ROOT = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const i = process.argv.indexOf("--dist");
const DIST = i > 0 ? resolve(process.argv[i + 1]) : join(ROOT, "dist");
const api = await import(pathToFileURL(join(DIST, "src/index.js")).href);
const internal = await import(pathToFileURL(join(DIST, "src/internal.js")).href);

const lines = readFileSync(0, "utf8").split("\n").filter((l) => l && !l.startsWith("#"));
const totals = [0, 0, 0, 0, 0, 0];
let pagesTotal = 0;
for (const line of lines) {
  const [path, password] = line.split("\t");
  const t = [0, 0, 0, 0, 0, 0];
  const started = performance.now();
  let pdf;
  let pages;
  try {
    pdf = await api.open(path, { pages: Array.from({ length: 20 }, (_, k) => k + 1), password: password || undefined });
    pages = await pdf.pages;
  } catch {
    console.log(`${path}\t0\t${((performance.now() - started) / 1000).toFixed(4)}\t0\t0\t0\t0\t0`);
    if (pdf) await pdf.release();
    continue;
  }
  t[0] = performance.now() - started;
  for (const p of pages) {
    const phases = [() => p.objects, () => p.extractText(), () => p.extractWords(), () => p.findTables(), () => p.objects];
    for (let k = 0; k < phases.length; k++) {
      const s = performance.now();
      try {
        await phases[k]();
      } catch {
        // an exception ends the phase (as in the Rust and Python twins)
      }
      t[k + 1] += performance.now() - s;
    }
  }
  await pdf.release();
  pagesTotal += pages.length;
  t.forEach((x, k) => (totals[k] += x));
  console.log(`${path}\t${pages.length}\t${t.map((x) => (x / 1000).toFixed(4)).join("\t")}`);
}
const core = await internal.getCore();
const rss = process.resourceUsage().maxRSS * 1024;
console.log(
  `# total\t${pagesTotal}\t${totals.map((x) => (x / 1000).toFixed(3)).join("\t")}\tmaxRSS ${rss}\twasm memory ${core.memoryBytes?.() ?? "?"}`,
);
