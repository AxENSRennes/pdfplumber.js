#!/usr/bin/env node
// Builds the online demo (demo/) into a static folder, demo/dist/ by
// default, served as it is (GitHub Pages: .github/workflows/pages.yml).
// Every URL in it is relative, so the folder works under any path
// (https://<user>.github.io/pdfplumber.js/).
//
//   node scripts/build.mjs             # the package, dist/ (first)
//   node scripts/build-demo.mjs [--out DIR]
//
// Output:
//   index.html, app.js, style.css      the page (demo/)
//   pdfplumber/                        the browser build, as the README's
//                                      esbuild recipe copies it: index.js
//                                      and its chunks, worker.js,
//                                      pdfplumber.wasm, cmaps/ (app.js calls
//                                      configure({ wasm, cmaps, worker }))
//   samples/                           the sample PDFs app.js lists (SAMPLES),
//                                      from pdfplumber's test suite
//                                      (upstream/pdfplumber-python, MIT; U.S.
//                                      government documents)
//   LICENSE, THIRD_PARTY_NOTICES       the notices of what is redistributed
import { copyFileSync, cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);
const outArg = args.indexOf("--out");
const OUT = resolve(outArg >= 0 ? args[outArg + 1] : join(ROOT, "demo/dist"));
const DIST = join(ROOT, "dist");
const SITE = join(ROOT, "demo");

function fail(message) {
  console.error(`build-demo: ${message}`);
  process.exit(1);
}

for (const f of ["browser/index.js", "browser/worker.js", "pdfplumber.wasm", "cmaps"])
  if (!existsSync(join(DIST, f))) fail(`dist/${f} is missing: run \`node scripts/build.mjs\` first`);

// the sample PDFs: in this checkout's submodule, or in the main checkout's
// (a git worktree has no submodules of its own; PDFPLUMBER_ROOT)
const sampleDirs = [ROOT, process.env.PDFPLUMBER_ROOT]
  .filter(Boolean)
  .map((root) => join(root, "upstream/pdfplumber-python/tests/pdfs"));
const sampleDir = sampleDirs.find((d) => existsSync(d));
if (!sampleDir)
  fail("upstream/pdfplumber-python is not checked out: `git submodule update --init upstream/pdfplumber-python`");
const app = readFileSync(join(SITE, "app.js"), "utf8");
const block = app.match(/const SAMPLES = \[([\s\S]*?)\n\];/);
if (!block) fail("demo/app.js has no SAMPLES list");
const samples = [...block[1].matchAll(/file: "([^"]+\.pdf)"/g)].map((m) => m[1]);
if (!samples.length) fail("demo/app.js lists no sample");

rmSync(OUT, { recursive: true, force: true });
mkdirSync(join(OUT, "pdfplumber"), { recursive: true });
mkdirSync(join(OUT, "samples"), { recursive: true });

for (const f of ["index.html", "app.js", "style.css"]) copyFileSync(join(SITE, f), join(OUT, f));

// index.js and the chunks it imports, worker.js (one file); not internal.js
for (const f of readdirSync(join(DIST, "browser")))
  if (f === "index.js" || f === "worker.js" || /^chunk-.*\.js$/.test(f))
    copyFileSync(join(DIST, "browser", f), join(OUT, "pdfplumber", f));
copyFileSync(join(DIST, "pdfplumber.wasm"), join(OUT, "pdfplumber/pdfplumber.wasm"));
cpSync(join(DIST, "cmaps"), join(OUT, "pdfplumber/cmaps"), { recursive: true });

for (const name of samples) {
  const from = join(sampleDir, name);
  if (!existsSync(from)) fail(`sample ${name} is not in ${sampleDir}`);
  copyFileSync(from, join(OUT, "samples", name));
}

for (const f of ["LICENSE", "THIRD_PARTY_NOTICES"]) copyFileSync(join(ROOT, f), join(OUT, f));
// GitHub Pages: no Jekyll processing
writeFileSync(join(OUT, ".nojekyll"), "");

function size(path) {
  const s = statSync(path);
  return s.isDirectory() ? readdirSync(path).reduce((n, f) => n + size(join(path, f)), 0) : s.size;
}
console.log(
  `${relative(process.cwd(), OUT) || "."}: ${(size(OUT) / 1e6).toFixed(1)} MB ` +
    `(engine ${(size(join(OUT, "pdfplumber/pdfplumber.wasm")) / 1e6).toFixed(2)} MB, ${samples.length} samples)`,
);
