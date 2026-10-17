#!/usr/bin/env node
// Sizes of the built package (docs/js-runtime.md, "Taille"): raw, gzip -9 and
// brotli (quality 11) of the WebAssembly module, the browser bundles, the
// Node build and the CMaps. With --budget, exits 1 when the module or the
// browser entry exceed the budget below.
//   node scripts/size.mjs [--budget] [--json]
import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { brotliCompressSync, constants, gzipSync } from "node:zlib";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const DIST = join(ROOT, "dist");

// The budget (brotli bytes): the page layer will add the interpreter, fonts
// and the document code, so the budget leaves room for them.
export const BUDGET = { wasmBrotli: 1_000_000, browserJsBrotli: 40_000 };

const sizes = (buf) => ({
  raw: buf.length,
  gzip: gzipSync(buf, { level: 9 }).length,
  brotli: brotliCompressSync(buf, { params: { [constants.BROTLI_PARAM_QUALITY]: 11 } }).length,
});

function files(dir, filter = () => true) {
  const out = [];
  for (const f of readdirSync(dir)) {
    const p = join(dir, f);
    if (statSync(p).isDirectory()) out.push(...files(p, filter));
    else if (filter(p)) out.push(p);
  }
  return out;
}

const sum = (list) =>
  list.reduce((a, p) => {
    const s = sizes(readFileSync(p));
    return { raw: a.raw + s.raw, gzip: a.gzip + s.gzip, brotli: a.brotli + s.brotli };
  }, { raw: 0, gzip: 0, brotli: 0 });

const browserEntry = ["index.js", ...readdirSync(join(DIST, "browser")).filter((f) => f.startsWith("chunk-"))].map((f) =>
  join(DIST, "browser", f),
);
const report = {
  "dist/pdfplumber.wasm": sizes(readFileSync(join(DIST, "pdfplumber.wasm"))),
  "dist/browser (index.js and its chunks)": sum(browserEntry),
  "dist/browser/worker.js (worker mode)": sizes(readFileSync(join(DIST, "browser/worker.js"))),
  "dist/src (Node, .js)": sum(files(join(DIST, "src"), (p) => p.endsWith(".js"))),
  "dist/cmaps (all)": sum(files(join(DIST, "cmaps"))),
  "dist/cmaps: UniJIS-UCS2-H + to-unicode-Adobe-Japan1": sum(
    ["UniJIS-UCS2-H", "to-unicode-Adobe-Japan1"].map((n) => join(DIST, "cmaps", `${n}.pdfcmap.zlib`)),
  ),
};

if (process.argv.includes("--json")) console.log(JSON.stringify(report, null, 1));
else
  for (const [k, v] of Object.entries(report))
    console.log(`${k.padEnd(56)} ${String(v.raw).padStart(9)} raw ${String(v.gzip).padStart(9)} gzip ${String(v.brotli).padStart(9)} brotli`);

if (process.argv.includes("--budget")) {
  const wasm = report["dist/pdfplumber.wasm"].brotli;
  const js = report["dist/browser (index.js and its chunks)"].brotli;
  const over = [];
  if (wasm > BUDGET.wasmBrotli) over.push(`wasm ${wasm} > ${BUDGET.wasmBrotli}`);
  if (js > BUDGET.browserJsBrotli) over.push(`browser JS ${js} > ${BUDGET.browserJsBrotli}`);
  if (over.length) {
    console.error(`size budget exceeded: ${over.join(", ")}`);
    process.exit(1);
  }
  console.log("size budget: ok");
}
