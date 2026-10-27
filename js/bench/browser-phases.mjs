#!/usr/bin/env node
// The phases of js/bench/phases.mjs in browsers (Chromium, Firefox and
// WebKit of Playwright), through the public API of the browser bundle
// (dist/browser), with the core in the page and in a worker
// (js/bench/phases.html): for each browser and mode, the total of each
// phase in ms per page, written as JSON lines to --out (default
// $PDFPLUMBER_ROOT/.cache/bench/browser-phases.jsonl) and on stdout.
// Playwright's Firefox is a Nightly build, not a speed reference
// (docs/architecture.md, "Performance").
//   node js/bench/browser-phases.mjs [--only chromium,firefox,webkit] [--modes page,worker] < list.tsv
// list.tsv: `path<TAB>password` lines (paths under $PDFPLUMBER_ROOT, served
// at /corpus/...). Needs `node scripts/build.mjs`; WebKit without its
// system libraries runs through PDFPLUMBER_WEBKIT (scripts/webkit-local-libs.sh).
import { appendFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium, firefox, webkit } from "playwright";
import { startServer } from "../test/static-server.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const DATA = process.env.PDFPLUMBER_ROOT ?? ROOT;
const args = process.argv.slice(2);
const opt = (n, d) => (args.includes(n) ? args[args.indexOf(n) + 1] : d);
const only = opt("--only", "chromium,firefox,webkit").split(",");
const modes = opt("--modes", "page,worker").split(",");
const outFile = opt("--out", join(DATA, ".cache/bench/browser-phases.jsonl"));
mkdirSync(dirname(outFile), { recursive: true });

const list = readFileSync(0, "utf8")
  .split("\n")
  .filter((l) => l && !l.startsWith("#"))
  .map((l) => {
    const [path, password] = l.split("\t");
    return { url: `/corpus/${relative(DATA, resolve(path)).split("\\").join("/")}`, password: password || "" };
  });
const tmp = mkdtempSync(join(tmpdir(), "browser-phases-"));
const listFile = join(tmp, "list.json");
writeFileSync(listFile, JSON.stringify(list));
const { server, url } = await startServer(ROOT, { mounts: { "/phases-list.json": listFile, "/corpus": DATA } });

const summarize = (rows) => {
  const pages = rows.reduce((n, r) => n + r[1], 0);
  const totals = [0, 0, 0, 0, 0, 0, 0];
  for (const r of rows) for (let k = 0; k < 7; k++) totals[k] += r[k + 2];
  const ms = totals.map((x) => Number(((x * 1000) / pages).toFixed(2)));
  return { files: rows.length, pages, open: ms[0], objects: ms[1], text: ms[2], words: ms[3], tables: ms[4], objectsAgain: ms[5], extractTables: ms[6] };
};

for (const [name, type] of [
  ["chromium", chromium],
  ["firefox", firefox],
  ["webkit", webkit],
]) {
  if (!only.includes(name)) continue;
  for (const mode of modes) {
    process.stderr.write(`${name} ${mode}...\n`);
    let browser;
    let result;
    try {
      const executablePath = name === "webkit" ? process.env.PDFPLUMBER_WEBKIT : undefined;
      browser = await type.launch(executablePath ? { executablePath } : {});
      const p = await browser.newPage();
      await p.goto(`${url}/js/bench/phases.html${mode === "worker" ? "?worker" : ""}`);
      await p.waitForFunction(() => window.__result, null, { timeout: 60 * 60_000, polling: 1000 });
      const r = await p.evaluate(() => window.__result);
      result = r.error
        ? { browser: `${name} ${browser.version()}`, mode, error: r.error }
        : { browser: `${name} ${browser.version()}`, mode, ...summarize(r.rows) };
    } catch (e) {
      result = { browser: name, mode, error: String(e.message ?? e).split("\n")[0] };
    } finally {
      await browser?.close();
    }
    appendFileSync(outFile, `${JSON.stringify(result)}\n`);
    console.log(JSON.stringify(result));
  }
}
rmSync(tmp, { recursive: true, force: true });
server.closeAllConnections();
server.close();
process.exit(0);
