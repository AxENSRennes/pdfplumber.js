#!/usr/bin/env node
// The `pdftrace` of the public JavaScript API: writes the canonical trace of
// a plumber_* stage of each file through the public API of the built
// package (dist/src, with dist/pdfplumber-oracle.wasm: the oracle hooks),
// with the command line of crates/pdftrace/src/main.rs, so that
// `python -m oracle.verify --engine js-api` runs it in place of `pdftrace`
// (docs/verification.md, "Parity Through The JavaScript API"):
//
//   pdftrace-js --stage S [--password=P] [--max-pages N] [--cmap-dir DIR] [--oracle-dir DIR] --out DIR F1.pdf ...
//   pdftrace-js --stage S [...] FILE.pdf          (the trace on stdout)
//   pdftrace-js --list-stages
//
// Each file runs in a new core (a new Python process, as each oracle trace
// is built in its own process: `dropCore()`), whose results are written in
// the typed format (`set_format("typed")`), read by the stages from
// `core.lastTyped` after each call of the API: the values the API returns
// are their projection. Exit status: 0, 1 when a file cannot be traced (a
// harness error), 2 for a usage error or a stage not implemented.
import { appendFileSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { canon, sha256Hex } from "./canon.mjs";
import { HarnessError, isPython, lib } from "./common.mjs";

const ROOT = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const DIST = process.env.PDFPLUMBER_JS_DIST ?? join(ROOT, "dist");

const STAGES = {
  plumber_objects: "./plumber_objects.mjs",
  plumber_text: "./plumber_text.mjs",
  plumber_tables: "./plumber_tables.mjs",
  plumber_doc: "./plumber_doc.mjs",
};

function usage(message) {
  if (message) process.stderr.write(`pdftrace-js: ${message}\n`);
  process.stderr.write(
    "usage: pdftrace-js --stage S [--password P] [--max-pages N] [--cmap-dir DIR] [--oracle-dir DIR] [--out DIR] FILE.pdf...\n" +
      "       pdftrace-js --list-stages\n",
  );
  process.exit(2);
}

function parseArgs(argv) {
  const a = { maxPages: 20, password: null, files: [] };
  for (let i = 0; i < argv.length; i++) {
    let arg = argv[i];
    let value;
    const eq = arg.indexOf("=");
    if (arg.startsWith("--") && eq > 0) {
      value = arg.slice(eq + 1);
      arg = arg.slice(0, eq);
    }
    const next = () => {
      if (value !== undefined) return value;
      if (i + 1 >= argv.length) usage(`${arg} needs a value`);
      return argv[++i];
    };
    switch (arg) {
      case "--list-stages":
        a.list = true;
        break;
      case "--stage":
        a.stage = next();
        break;
      case "--password":
        a.password = next();
        break;
      case "--max-pages":
        a.maxPages = Number(next());
        break;
      case "--cmap-dir":
        a.cmapDir = next();
        break;
      case "--oracle-dir":
        a.oracleDir = next();
        break;
      case "--out":
        a.out = next();
        break;
      default:
        if (arg.startsWith("--")) usage(`unknown option ${arg}`);
        a.files.push(arg);
    }
  }
  return a;
}

async function loadLib() {
  const api = await import(join(DIST, "src/index.js"));
  const internal = await import(join(DIST, "src/internal.js"));
  Object.assign(lib, api, internal);
}

async function traceFile(stage, args, path) {
  const bytes = readFileSync(path);
  const sha = sha256Hex(bytes);
  let oracle;
  if (args.oracleDir && stage.ORACLE_INPUT) {
    const name = args.password === null ? `${sha}.txt` : `${sha}.pw-${sha256Hex(Buffer.from(args.password, "utf8")).slice(0, 12)}.txt`;
    oracle = readFileSync(join(args.oracleDir, name), "utf8");
  }
  // a new Python process for each file
  lib.dropCore();
  const t = await lib.internals();
  await t.setFormat("typed");
  const lines = [];
  const file = { path, sha, password: args.password, maxPages: args.maxPages, oracle };
  try {
    await stage.run(lines, file);
    lines.push("!end");
  } catch (e) {
    // terminated: an exception out of the stage ends the trace
    if (!isPython(e)) throw e;
    lines.push(canon(e));
  }
  return { sha, text: lines.map((l) => l + "\n").join("") };
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.list) {
    for (const name of ["psparser", "objects", "streams", "pages", "interp", "fonts", "layout", "interp_text", "fonts_text"])
      console.log(`${name} (not implemented)`);
    for (const name of Object.keys(STAGES)) console.log(name);
    return 0;
  }
  if (!args.stage) usage("--stage is required");
  if (!STAGES[args.stage]) {
    process.stderr.write(`pdftrace-js: stage ${JSON.stringify(args.stage)} is not implemented (the public API has the plumber_* stages)\n`);
    return 2;
  }
  if (!args.files.length) usage("no file");
  const stage = await import(STAGES[args.stage]);
  if (args.oracleDir && !stage.ORACLE_INPUT) usage(`stage ${args.stage} does not read the oracle (no --oracle-dir)`);
  await loadLib();
  const cmapDir = args.cmapDir ?? process.env.PDFPLUMBER_CMAP_DIR;
  lib.configure({
    wasm: join(DIST, "pdfplumber-oracle.wasm"),
    cmaps: cmapDir ? { load: async (name) => new Uint8Array(readFileSync(join(cmapDir, `${name}.pdfcmap`))) } : false,
  });
  let status = 0;
  if (!args.out) {
    const { text } = await traceFile(stage, args, args.files[0]);
    process.stdout.write(text);
    return 0;
  }
  mkdirSync(args.out, { recursive: true });
  for (const path of args.files) {
    const started = performance.now();
    try {
      const { sha, text } = await traceFile(stage, args, path);
      const tmp = join(args.out, `.${sha}.${process.pid}.tmp`);
      writeFileSync(tmp, text);
      renameSync(tmp, join(args.out, `${sha}.txt`));
      appendFileSync(join(args.out, "_times.tsv"), `${sha}\t${((performance.now() - started) / 1000).toFixed(3)}\n`);
    } catch (e) {
      const where = e instanceof HarnessError ? "harness" : "error";
      process.stderr.write(`pdftrace-js: ${path}: ${where}: ${e?.stack ?? e}\n`);
      status = 1;
    }
  }
  return status;
}

main().then(
  (code) => process.exit(code),
  (e) => {
    process.stderr.write(`pdftrace-js: ${e?.stack ?? e}\n`);
    process.exit(1);
  },
);
