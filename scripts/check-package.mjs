#!/usr/bin/env node
// The npm package as a consumer gets it (docs/js-runtime.md, "Paquet JS"):
// `npm pack` of this checkout (dist/ built), then
//   - its files: what must ship (the license and notices, the Node and
//     browser builds, the module, every CMap, the docs) and what must not
//     (the oracle module, sources); the relative links of the shipped
//     Markdown files resolve to shipped files; the worker script is one
//     file (no chunk shared with the page's);
//   - installed into a new project (npm, offline): its declarations
//     type-check in a Node project without the DOM library (with and
//     without `await using`) and in a browser project; it runs from
//     `import` and from `require()`, and its subpaths resolve;
//   - with `--bundles`: two browser applications bundling it, with esbuild
//     and with Vite, following the README's recipes (dist/bundler-tests/,
//     run in Chromium by js/test/browser/bundlers.spec.ts).
//
//   node scripts/check-package.mjs [--bundles]
//
// PDFPLUMBER_ROOT gives the PDFs; the project lives in a temporary
// directory, removed at the end.
import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, posix, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const PDFPLUMBER_ROOT = process.env.PDFPLUMBER_ROOT ?? ROOT;
const bundles = process.argv.includes("--bundles");
const npm = process.platform === "win32" ? "npm.cmd" : "npm";
const tsc = join(ROOT, "node_modules/typescript/bin/tsc");

const fail = (msg) => {
  console.error(`check-package: ${msg}`);
  process.exitCode = 1;
};

const tmp = mkdtempSync(join(tmpdir(), "pdfplumber-js-package-"));
try {
  // ------------------------------------------------------------ the files
  const [packed] = JSON.parse(execFileSync(npm, ["pack", "--json", "--pack-destination", tmp], { cwd: ROOT, encoding: "utf8" }));
  const files = new Set(packed.files.map((f) => f.path));
  const required = [
    "package.json",
    "README.md",
    "LICENSE",
    "THIRD_PARTY_NOTICES",
    "dist/src/index.js",
    "dist/src/index.d.ts",
    "dist/src/internal.js",
    "dist/browser/index.js",
    "dist/browser/internal.js",
    "dist/browser/worker.js",
    "dist/pdfplumber.wasm",
    "docs/public-api.md",
    "docs/js-runtime.md",
  ];
  for (const f of required) if (!files.has(f)) fail(`${f} is not in the package`);
  const names = readFileSync(join(ROOT, "js/src/generated/cmap-names.ts"), "utf8").match(/"[^"]+"/g).length;
  const cmaps = [...files].filter((f) => /^dist\/cmaps\/.+\.pdfcmap\.zlib$/.test(f)).length;
  if (cmaps !== names) fail(`${cmaps} CMap files in the package, ${names} names`);
  for (const f of files) {
    if (/^(js|crates|oracle|scripts|target|corpus|upstream)\//.test(f) || f === "dist/pdfplumber-oracle.wasm" || f.startsWith("dist/bundler-tests/"))
      fail(`${f} should not be in the package`);
  }
  for (const md of [...files].filter((f) => f.endsWith(".md"))) {
    const text = readFileSync(join(ROOT, md), "utf8");
    for (const [, target] of text.matchAll(/\]\(([^)\s]+)\)/g)) {
      if (/^[a-z]+:/i.test(target) || target.startsWith("#")) continue;
      const path = posix.normalize(posix.join(posix.dirname(md), target.split("#")[0]));
      if (!files.has(path)) fail(`${md} links to ${target}, which is not in the package`);
    }
  }
  const worker = readFileSync(join(ROOT, "dist/browser/worker.js"), "utf8");
  if (/\bimport\s*(?:[\w{}*\s,]+from\s*)?["'][./]/.test(worker) || /\bimport\(\s*["']/.test(worker))
    fail("dist/browser/worker.js imports another file: it must be one file");

  // ----------------------------------------------------- a project using it
  const project = join(tmp, "consumer");
  mkdirSync(project);
  writeFileSync(join(project, "package.json"), JSON.stringify({ name: "consumer", private: true, type: "module" }));
  execFileSync(npm, ["install", "--offline", "--no-audit", "--no-fund", "--ignore-scripts", join(tmp, packed.filename)], {
    cwd: project,
    stdio: "inherit",
  });

  // the declarations, as a consumer's tsc reads them (skipLibCheck off)
  const pdf = join(PDFPLUMBER_ROOT, "upstream/pdfplumber-python/tests/pdfs/pdffill-demo.pdf");
  const members = `
import * as pp from "pdfplumber-js";
export async function members(input: pp.PDFInput): Promise<void> {
  const pdf: pp.PDFPlumberDocument = await pp.open(input, { laparams: {}, onWarning: (w: pp.PDFPlumberWarning) => void w.message });
  const [page] = await pdf.pages;
  const text: string = await page.extractText({ layout: true });
  const words: pp.Word[] = await page.extract_words({ extra_attrs: ["fontname"] });
  const lines: pp.SearchResult[] = await page.extractTextLines({ strip: null });
  const hits: pp.SearchResult[] = await page.search(/a/, { return_chars: false });
  const crop: pp.PDFPlumberPage = await page.crop([0, 0, page.width, page.height / 2], { strict: false });
  const filtered = await crop.filter((o: pp.PDFObject) => o.object_type === "char");
  const tables: pp.Table[] = await filtered.findTables({ vertical_strategy: "text" });
  for (const t of tables) void (await t.extract());
  const finder: pp.TableFinderResult<pp.Table> = await page.debugTablefinder();
  const tree: pp.PDFStructTree = await pdf.structTree();
  for (const el of tree) void (await el.toDict(), await el.allMcids());
  const found: pp.PDFStructElement[] = await tree.findAll("P");
  const stream: pp.PDFStream | undefined = ((await page.images)[0] as { stream?: pp.PDFStream } | undefined)?.stream;
  void [text, words, lines, hits, finder, found, stream, await pdf.metadata, await pdf.toJson(), pp.pyFloat(1), pp.pyTuple(1, 2)];
  pp.configure({ cmaps: { baseUrl: "/cmaps/" } });
  await pdf.close();
  await pdf.release();
}
`;
  writeFileSync(join(project, "members.ts"), members);
  writeFileSync(
    join(project, "dispose.ts"),
    `import { open } from "pdfplumber-js";\nexport async function quickStart(): Promise<string> {\n  await using pdf = await open("report.pdf");\n  const [page] = await pdf.pages;\n  return page.extractText();\n}\n`,
  );
  writeFileSync(
    join(project, "browser.ts"),
    `import { configure, open } from "pdfplumber-js";\nconfigure({ worker: true, wasm: new WebAssembly.Module(new Uint8Array(8)) });\nexport const p = open(new Blob([]));\n`,
  );
  const types = join(ROOT, "node_modules/@types");
  const tsconfigs = {
    // a Node project: @types/node, no DOM library, the default lib of es2022
    "tsconfig.node.json": {
      compilerOptions: { target: "es2022", module: "nodenext", moduleResolution: "nodenext", strict: true, noEmit: true, types: ["node"], typeRoots: [types] },
      files: ["members.ts", "dispose.ts"],
    },
    // a browser project bundled: the DOM library, bundler resolution
    "tsconfig.browser.json": {
      compilerOptions: { target: "es2022", module: "esnext", moduleResolution: "bundler", lib: ["es2022", "dom"], strict: true, noEmit: true, types: [] },
      files: ["members.ts", "browser.ts"],
    },
  };
  for (const [name, config] of Object.entries(tsconfigs)) {
    writeFileSync(join(project, name), JSON.stringify(config, null, 1));
    try {
      execFileSync(process.execPath, [tsc, "-p", name], { cwd: project, encoding: "utf8" });
    } catch (e) {
      fail(`the declarations do not type-check in ${name}:\n${e.stdout}`);
    }
  }

  // the package at run time, from import and from require()
  writeFileSync(
    join(project, "run.mjs"),
    `import { open } from "pdfplumber-js";\nconst pdf = await open(${JSON.stringify(pdf)});\nconst [page] = await pdf.pages;\nconst text = await page.extractText();\nawait pdf.close();\nawait pdf.release();\nif (!text.length) throw new Error("no text");\nconsole.log("import: " + text.length + " characters");\n`,
  );
  writeFileSync(
    join(project, "run.cjs"),
    `const pp = require("pdfplumber-js");\nif (typeof pp.open !== "function") throw new Error("require: no open()");\nfor (const p of ["pdfplumber-js/package.json", "pdfplumber-js/worker", "pdfplumber-js/pdfplumber.wasm", "pdfplumber-js/cmaps/GBK-EUC-H.pdfcmap.zlib"]) require.resolve(p);\nconsole.log("require: ok");\n`,
  );
  for (const script of ["run.mjs", "run.cjs"]) {
    try {
      process.stdout.write(execFileSync(process.execPath, [script], { cwd: project, encoding: "utf8" }));
    } catch (e) {
      fail(`${script} failed:\n${e.stdout}${e.stderr}`);
    }
  }

  // ------------------------------------------------- applications bundling it
  if (bundles) {
    const out = join(ROOT, "dist/bundler-tests");
    rmSync(out, { recursive: true, force: true });
    const app = readFileSync(join(ROOT, "js/test/bundlers/app.js"), "utf8");
    const pkg = join(project, "node_modules/pdfplumber-js/dist");
    const page = (settings, script) =>
      `<!doctype html><meta charset="utf-8"><title>bundled</title>\n<script>window.SETTINGS = ${JSON.stringify(settings)};</script>\n<script type="module" src="${script}"></script>\n`;
    // esbuild: the URLs are left as they are; the files copied, configure()
    const esb = join(project, "esbuild-app");
    mkdirSync(esb);
    writeFileSync(join(esb, "main.js"), app);
    const { build } = await import(join(ROOT, "node_modules/esbuild/lib/main.js"));
    await build({ entryPoints: [join(esb, "main.js")], bundle: true, format: "esm", platform: "browser", outdir: join(out, "esbuild"), logLevel: "warning" });
    mkdirSync(join(out, "esbuild/assets"));
    cpSync(join(pkg, "pdfplumber.wasm"), join(out, "esbuild/assets/pdfplumber.wasm"));
    cpSync(join(pkg, "cmaps"), join(out, "esbuild/assets/cmaps"), { recursive: true });
    cpSync(join(pkg, "browser/worker.js"), join(out, "esbuild/assets/worker.js"));
    const settings = { wasm: "assets/pdfplumber.wasm", cmaps: { baseUrl: "assets/cmaps/" }, worker: "assets/worker.js" };
    writeFileSync(join(out, "esbuild/index.html"), page(settings, "main.js"));
    // Vite: the module and the worker found by Vite; the CMaps copied to
    // the public directory, configure({ cmaps })
    const vite = join(project, "vite-app");
    mkdirSync(join(vite, "public"), { recursive: true });
    writeFileSync(join(vite, "main.js"), app);
    writeFileSync(join(vite, "index.html"), page({ cmaps: { baseUrl: "cmaps/" }, worker: true }, "./main.js"));
    cpSync(join(pkg, "cmaps"), join(vite, "public/cmaps"), { recursive: true });
    const { build: viteBuild } = await import(join(ROOT, "node_modules/vite/dist/node/index.js"));
    await viteBuild({ root: vite, base: "./", logLevel: "warn", build: { outDir: join(out, "vite"), emptyOutDir: true } });
    if (!existsSync(join(out, "vite/index.html"))) fail("the Vite build wrote no index.html");
    console.log(`bundles: ${out}/{esbuild,vite}`);
  }
} finally {
  rmSync(tmp, { recursive: true, force: true });
}
if (!process.exitCode) console.log("check-package: the package is complete, type-checks and runs");
