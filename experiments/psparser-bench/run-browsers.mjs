// Runs the benchmark in Chromium, Firefox and WebKit (Playwright) and in Node,
// then writes out/results.json. Usage: node experiments/psparser-bench/run-browsers.mjs
import { build } from "esbuild";
import { createServer } from "node:http";
import { readFile, writeFile } from "node:fs/promises";
import { dirname, extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium, firefox, webkit } from "@playwright/test";

const here = dirname(fileURLToPath(import.meta.url));
await build({ entryPoints: [join(here, "ts/bench-browser.ts")], bundle: true, format: "esm", target: "es2022", outfile: join(here, "out/bench-browser.js"), logLevel: "warning" });
await writeFile(join(here, "out/bench.html"), '<!doctype html><meta charset="utf-8"><script type="module" src="bench-browser.js"></script>');

const types = { ".html": "text/html", ".js": "text/javascript", ".wasm": "application/wasm", ".bin": "application/octet-stream" };
let onPosted = () => {};
const server = createServer(async (req, res) => {
  if (req.method === "POST") {
    let body = "";
    for await (const chunk of req) body += chunk;
    res.writeHead(204).end();
    return onPosted(JSON.parse(body));
  }
  const path = normalize(join(here, decodeURIComponent(new URL(req.url, "http://x").pathname)));
  if (!path.startsWith(here)) return res.writeHead(403).end();
  try {
    const body = await readFile(path);
    res.writeHead(200, { "content-type": types[extname(path)] ?? "application/octet-stream" }).end(body);
  } catch {
    res.writeHead(404).end();
  }
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const base = `http://127.0.0.1:${server.address().port}`;

// BROWSERS=chromium,firefox,webkit,firefox-plain limits the run;
// FIREFOX_BIN=/Applications/Firefox.app/Contents/MacOS/firefox uses an installed Firefox for firefox-plain.
const only = process.env.BROWSERS?.split(",");
const results = [];

// Playwright drives Firefox through its debugging protocol, and SpiderMonkey then
// compiles wasm for debugging; so Firefox is also run as a plain headless process.
if (!only || only.includes("firefox-plain")) {
  const { spawn } = await import("node:child_process");
  const { mkdtemp } = await import("node:fs/promises");
  const { tmpdir } = await import("node:os");
  const profile = await mkdtemp(join(tmpdir(), "psbench-ff-"));
  const posted = new Promise((r) => (onPosted = r));
  const bin = process.env.FIREFOX_BIN ?? firefox.executablePath();
  const proc = spawn(bin, ["-headless", "-no-remote", "-profile", profile, `${base}/out/bench.html?post`], { stdio: "ignore" });
  const result = await posted;
  proc.kill();
  if (result.error) throw new Error("firefox-plain: " + result.error);
  result.env = process.env.FIREFOX_BIN ? "firefox (installe, sans Playwright)" : "firefox (build Playwright, sans automation)";
  results.push(result);
  console.log(result.env, Object.fromEntries(Object.entries(result.modes).map(([k, v]) => [k, Math.round(v.medianMs)])));
}

for (const [name, engine] of [["chromium", chromium], ["firefox", firefox], ["webkit", webkit]]) {
  if (only && !only.includes(name)) continue;
  const browser = await engine.launch();
  const page = await browser.newPage();
  await page.goto(`${base}/out/bench.html`);
  await page.waitForFunction(() => window.__result || window.__error, null, { timeout: 15 * 60 * 1000, polling: 1000 });
  const error = await page.evaluate(() => window.__error);
  if (error) throw new Error(`${name}: ${error}`);
  const result = await page.evaluate(() => window.__result);
  result.env = `${name} ${browser.version()}`;
  results.push(result);
  console.log(result.env, Object.fromEntries(Object.entries(result.modes).map(([k, v]) => [k, Math.round(v.medianMs)])));
  await browser.close();
}
server.close();

const nodeResult = JSON.parse(await readFile(join(here, "out/bench-node.json"), "utf8"));
if (!only) await writeFile(join(here, "out/results.json"), JSON.stringify({ node: nodeResult, browsers: results }, null, 2));
