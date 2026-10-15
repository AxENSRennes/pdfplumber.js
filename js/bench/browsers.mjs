#!/usr/bin/env node
// The result-encoding benchmark in browsers: Chromium, Firefox and WebKit of
// Playwright, plus a system Firefox (`--firefox PATH`, default
// /usr/bin/firefox when present: Playwright's Firefox is a Nightly build
// that runs Wasm much slower, docs/decision-experiments.md).
//   node js/bench/browsers.mjs [CORPUS.pjb] [--passes N] [--only chromium,firefox,webkit,system-firefox]
// Needs `node scripts/build.mjs` (dist/) and the corpus of
// crates/wasm/examples/bench_corpus.rs.
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium, firefox, webkit } from "playwright";
import { startServer } from "../test/static-server.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const args = process.argv.slice(2);
const opt = (n, d) => (args.includes(n) ? args[args.indexOf(n) + 1] : d);
const passes = Number(opt("--passes", 5));
const only = opt("--only", "chromium,firefox,webkit,system-firefox").split(",");
const systemFirefox = opt("--firefox", "/usr/bin/firefox");
const corpus =
  args.find((a) => a.endsWith(".pjb")) ?? join(process.env.PDFPLUMBER_ROOT ?? ROOT, ".cache/bench/pages_real.pjb");

let pending;
const { server, url } = await startServer(ROOT, {
  mounts: { "/corpus.pjb": corpus },
  onResult: (body) => pending?.(JSON.parse(body)),
});
const page = `${url}/js/bench/page.html?passes=${passes}`;
const results = [];

for (const [name, type] of [
  ["chromium", chromium],
  ["firefox", firefox],
  ["webkit", webkit],
]) {
  if (!only.includes(name)) continue;
  process.stderr.write(`${name}...\n`);
  let browser;
  try {
    // PDFPLUMBER_WEBKIT: the launcher of scripts/webkit-local-libs.sh
    const executablePath = name === "webkit" ? process.env.PDFPLUMBER_WEBKIT : undefined;
    browser = await type.launch(executablePath ? { executablePath } : {});
    const p = await browser.newPage();
    await p.goto(page);
    await p.waitForFunction(() => window.__result, null, { timeout: 30 * 60_000, polling: 1000 });
    const r = await p.evaluate(() => window.__result);
    results.push({ browser: `${name} ${browser.version()}`, ...r });
  } catch (e) {
    // a browser that cannot run here (WebKit without its system libraries)
    results.push({ browser: name, error: String(e.message ?? e).split("\n")[0] });
  } finally {
    await browser?.close();
  }
  process.stderr.write(`${JSON.stringify(results.at(-1)).slice(0, 300)}\n`);
}

if (only.includes("system-firefox") && existsSync(systemFirefox)) {
  process.stderr.write(`system firefox (${systemFirefox})...\n`);
  const profile = mkdtempSync(join(tmpdir(), "pp-ff-"));
  const version = await new Promise((ok) => {
    const v = spawn(systemFirefox, ["--version"]);
    let s = "";
    v.stdout.on("data", (d) => (s += d));
    v.on("close", () => ok(s.trim()));
  });
  const done = new Promise((ok) => (pending = ok));
  const ff = spawn(systemFirefox, ["--headless", "--no-remote", "--profile", profile, `${page}&post`], {
    stdio: "ignore",
  });
  const r = await Promise.race([done, new Promise((_, ko) => setTimeout(() => ko(new Error("timeout")), 30 * 60_000))]);
  ff.kill();
  rmSync(profile, { recursive: true, force: true });
  results.push({ browser: version, ...r });
}

server.close();
console.log(JSON.stringify(results, null, 1));

