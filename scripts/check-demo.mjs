#!/usr/bin/env node
// Drives the built demo (scripts/build-demo.mjs) in real browsers: serves
// the folder under /pdfplumber.js/ as GitHub Pages does, then, in each
// browser, opens the samples, turns pages, picks a file, changes the table
// strategy, runs the engine in a worker and on the main thread, and checks
// that the boxes drawn from pdfplumber.js's coordinates sit on the ink of
// the page pdf.js draws. Any console error fails the run. The page image
// comes from pdf.js on a CDN: this check needs the network.
//
//   node scripts/check-demo.mjs [--dir demo/dist] [--browsers chromium,firefox,webkit]
//                               [--screenshots DIR] [--pdf FILE]...
//
// --pdf adds files to open through the file picker (besides a sample).
import { createReadStream, existsSync, statSync } from "node:fs";
import { createServer } from "node:http";
import { basename, extname, join, normalize, resolve } from "node:path";
import { chromium, firefox, webkit } from "playwright";

const args = process.argv.slice(2);
const opt = (name, dflt) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : dflt;
};
const ROOT = resolve(new URL("..", import.meta.url).pathname);
const DIR = resolve(opt("--dir", join(ROOT, "demo/dist")));
const BROWSERS = opt("--browsers", "chromium,firefox,webkit").split(",");
const SHOTS = opt("--screenshots");
const PDFS = args.flatMap((a, i) => (args[i - 1] === "--pdf" ? [resolve(a)] : []));
const BASE = "/pdfplumber.js/";

if (!existsSync(join(DIR, "index.html"))) {
  console.error(`check-demo: ${DIR}/index.html is missing: run \`node scripts/build-demo.mjs\` first`);
  process.exit(1);
}

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".wasm": "application/wasm",
  ".pdf": "application/pdf",
};
const server = createServer((req, res) => {
  const path = decodeURIComponent(new URL(req.url, "http://x").pathname);
  if (!path.startsWith(BASE)) {
    res.writeHead(404).end();
    return;
  }
  let file = normalize(join(DIR, path.slice(BASE.length)));
  if (!file.startsWith(DIR)) {
    res.writeHead(403).end();
    return;
  }
  if (existsSync(file) && statSync(file).isDirectory()) file = join(file, "index.html");
  if (!existsSync(file)) {
    res.writeHead(404).end();
    return;
  }
  res.writeHead(200, { "content-type": TYPES[extname(file)] ?? "application/octet-stream" });
  createReadStream(file).pipe(res);
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const ORIGIN = `http://127.0.0.1:${server.address().port}`;
const URL_ = (query = "") => `${ORIGIN}${BASE}${query}`;

let failures = 0;
function check(ok, what) {
  console.log(`  ${ok ? "ok  " : "FAIL"} ${what}`);
  if (!ok) failures++;
}

/** Waits for page `pageNumber` to be extracted and drawn by pdf.js at the
 * current size (`#render[data-drawn]`: the page number, "none" when pdf.js
 * could not draw it). Resolves to "ready", "no image", "error" or what the
 * page showed at the timeout. */
async function ready(page, pageNumber = 1) {
  const drawn = (n) => {
    const state = document.body.dataset.state;
    const image = document.getElementById("render").dataset.drawn;
    if (state === "error") return "error";
    if (state === "busy" || window.demo?.page?.page_number !== n) return false;
    return image === String(n) ? "ready" : image === "none" ? "no image" : false;
  };
  try {
    await page.waitForFunction(drawn, pageNumber, { timeout: 60_000 });
    // a resize (a scroll bar that appears) redraws the image: let it settle
    await page.waitForTimeout(400);
    await page.waitForFunction(drawn, pageNumber, { timeout: 60_000 });
    return await page.evaluate(drawn, pageNumber);
  } catch {
    const seen = await page.evaluate(() =>
      [document.body.dataset.state, ...["headline", "details", "render-note"].map((id) => document.getElementById(id).textContent)].join(" | "),
    );
    return `timeout: ${seen}`;
  }
}

/** Where the boxes of the words sit on the page pdf.js draws: the ink
 * (dark pixels) inside the union of the word boxes, with the boxes moved by
 * every offset up to 6 CSS pixels in each direction. Aligned boxes hold
 * about the most ink where they are (in a dense table, a box moved by a
 * pixel or two can catch a little more of a ruling line); boxes shifted by
 * a few pixels lose a third of it. Also the share of the words whose box
 * holds ink. */
function alignment(page) {
  return page.evaluate(() => {
    const d = window.demo.data;
    const stage = document.getElementById("stage");
    const img = document.getElementById("render");
    const W = img.width;
    const H = img.height;
    const pix = img.getContext("2d", { willReadFrequently: true }).getImageData(0, 0, W, H).data;
    // ink: dark pixels; raw: any mark, the faint antialiased glyphs of
    // 4-pixel text included (does each word hold some)
    const ink = new Uint8Array(W * H);
    const raw = new Uint8Array(W * H);
    for (let i = 0; i < W * H; i++) {
      const sum = pix[i * 4 + 3] > 0 ? pix[i * 4] + pix[i * 4 + 1] + pix[i * 4 + 2] : 765;
      ink[i] = sum < 450 ? 1 : 0;
      raw[i] = sum < 600 ? 1 : 0;
    }
    const k = W / parseFloat(img.style.width); // image pixels per CSS pixel
    const left = parseFloat(img.style.left);
    const top = parseFloat(img.style.top);
    const s = stage.clientWidth / d.page.width;
    const [bx, by] = d.page.bbox;
    // no ink on the ruling lines of tables (pdfplumber's lines and the
    // edges of its rects, two pixels around): next to right-aligned
    // numbers, a box moved onto a rule would catch its ink
    const unink = (x0, y0, x1, y1) => {
      const X0 = Math.max(0, Math.floor(((x0 - bx) * s - left - 2) * k));
      const X1 = Math.min(W, Math.ceil(((x1 - bx) * s - left + 2) * k));
      const Y0 = Math.max(0, Math.floor(((y0 - by) * s - top - 2) * k));
      const Y1 = Math.min(H, Math.ceil(((y1 - by) * s - top + 2) * k));
      if (Math.min(X1 - X0, Y1 - Y0) > 6 * k) return; // not a thin line
      for (let y = Y0; y < Y1; y++) ink.fill(0, y * W + X0, y * W + X1);
    };
    for (const l of d.lines) unink(l.x0, l.top, l.x1, l.bottom);
    for (const r of d.rects) {
      unink(r.x0, r.top, r.x1, r.top);
      unink(r.x0, r.bottom, r.x1, r.bottom);
      unink(r.x0, r.top, r.x0, r.bottom);
      unink(r.x1, r.top, r.x1, r.bottom);
    }
    const R = Math.round(6 * k);
    const mask = new Uint8Array(W * H);
    const words = d.words.filter((w) => w.text.trim());
    let hit = 0;
    for (const w of words) {
      const X0 = Math.max(R, Math.floor(((w.x0 - bx) * s - left) * k));
      const X1 = Math.min(W - R, Math.ceil(((w.x1 - bx) * s - left) * k));
      const Y0 = Math.max(R, Math.floor(((w.top - by) * s - top) * k));
      const Y1 = Math.min(H - R, Math.ceil(((w.bottom - by) * s - top) * k));
      let n = 0;
      for (let y = Y0; y < Y1; y++)
        for (let x = X0; x < X1; x++) {
          mask[y * W + x] = 1;
          n += raw[y * W + x];
        }
      if (n) hit++;
    }
    const cells = [];
    for (let i = 0; i < W * H; i++) if (mask[i]) cells.push(i);
    const at = new Int32Array(cells);
    let best = { dx: 0, dy: 0, n: -1 };
    let zero = 0;
    const step = Math.max(1, Math.round(k / 2));
    for (let dy = -R; dy <= R; dy += step)
      for (let dx = -R; dx <= R; dx += step) {
        const off = dy * W + dx;
        let n = 0;
        for (let j = 0; j < at.length; j++) n += ink[at[j] + off];
        if (dx === 0 && dy === 0) zero = n;
        if (n > best.n) best = { dx, dy, n };
      }
    return {
      words: words.length,
      hit: words.length ? hit / words.length : 1,
      dx: best.dx / k,
      dy: best.dy / k,
      ratio: best.n > 0 ? zero / best.n : 1,
    };
  });
}

async function shot(page, name) {
  if (!SHOTS) return;
  await page.screenshot({ path: join(SHOTS, `${name}.png`), fullPage: true });
  console.log(`  shot ${join(SHOTS, `${name}.png`)}`);
}

async function session(browser, name, options = {}) {
  const context = await browser.newContext({ viewport: { width: 1366, height: 900 }, ...options });
  // the WebKit of scripts/webkit-local-libs.sh has no TLS: Playwright
  // fetches the CDN's files (pdf.js) for it
  if (browser.browserType().name() === "webkit" && process.env.PDFPLUMBER_WEBKIT)
    await context.route(/^https:/, async (route) => route.fulfill({ response: await route.fetch() }));
  const page = await context.newPage();
  const errors = [];
  const workers = [];
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()));
  page.on("pageerror", (e) => errors.push(String(e)));
  page.on("worker", (w) => workers.push(w.url()));
  return { context, page, errors, workers, name };
}

async function alignedOk(page, label) {
  const a = await alignment(page);
  const aligned = a.hit > 0.9 && a.ratio > 0.95;
  check(
    aligned,
    `${label}: boxes on the ink (${a.words} words, ${(a.hit * 100).toFixed(1)} % hold ink; ` +
      `most ink at (${a.dx.toFixed(1)}, ${a.dy.toFixed(1)}) px, ${(a.ratio * 100).toFixed(1)} % of it at (0, 0))`,
  );
}

for (const browserName of BROWSERS) {
  const type = { chromium, firefox, webkit }[browserName];
  const launchOptions =
    browserName === "webkit" && process.env.PDFPLUMBER_WEBKIT ? { executablePath: process.env.PDFPLUMBER_WEBKIT } : {};
  console.log(`\n${browserName}`);
  const browser = await type.launch(launchOptions);
  try {
    // 1. the first sample, opened at load: a ruled table, in worker mode
    {
      const s = await session(browser, "default");
      await s.page.goto(URL_());
      check((await ready(s.page)) === "ready", "the default sample is extracted");
      const headline = await s.page.textContent("#headline");
      check(/^Page 1 parsed in [\d.,]+ ms, in your browser/.test(headline), `headline: "${headline}"`);
      check(s.workers.some((u) => u.endsWith("/pdfplumber/worker.js")), "the engine runs in pdfplumber/worker.js");
      await alignedOk(s.page, "NICS table");
      await s.page.click("#tab-tables");
      const cells = await s.page.locator("#tables figure.table td").count();
      check(cells > 300, `the table is shown as HTML (${cells} cells)`);
      const words = await s.page.evaluate(() => window.demo.data.words.length);
      const rows = await s.page.locator("#words tbody tr").count();
      check(rows === Math.min(words, 3000) && words > 0, `the words table has a row per word (${rows})`);
      await shot(s.page, `${browserName}-nics-tables`);
      await s.page.click("#tab-code");
      const code = await s.page.textContent("#code");
      check(code.includes("await page.findTables()") && code.includes("configure({ worker: true })"), "the code panel shows the calls");
      // hover a word: the tooltip names it
      const w = await s.page.evaluate(() => {
        const d = window.demo.data;
        const word = d.words[Math.floor(d.words.length / 2)];
        const s = document.getElementById("stage").clientWidth / d.page.width;
        return { text: word.text, x: ((word.x0 + word.x1) / 2 - d.page.bbox[0]) * s, y: ((word.top + word.bottom) / 2 - d.page.bbox[1]) * s };
      });
      await s.page.locator("#overlay").hover({ position: { x: w.x, y: w.y } });
      const tip = await s.page.textContent("#tip strong");
      check(tip === w.text, `hovering a word shows it ("${tip}")`);
      check(!s.errors.length, `no console error ${s.errors.join(" | ")}`);
      await s.context.close();
    }

    // 2. a 15-page document: next page, text
    {
      const s = await session(browser, "pages");
      await s.page.goto(URL_("?sample=federal-register"));
      check((await ready(s.page)) === "ready", "Federal Register, page 1");
      await s.page.click("#next");
      check((await ready(s.page, 2)) === "ready", "Federal Register, page 2 after Next");
      const text = await s.page.textContent("#text");
      check(text.length > 2000, `page 2 text (${text.length} characters)`);
      await alignedOk(s.page, "Federal Register p. 2");
      await s.page.click("#tab-layout");
      await shot(s.page, `${browserName}-fedreg-p2-layout`);
      check(!s.errors.length, `no console error ${s.errors.join(" | ")}`);
      await s.context.close();
    }

    // 3. files through the picker; the table strategy; a rotated page
    {
      const s = await session(browser, "picker");
      await s.page.goto(URL_());
      await ready(s.page);
      for (const file of [join(DIR, "samples/senate-expenditures.pdf"), ...PDFS]) {
        await s.page.setInputFiles("#file", file);
        await s.page.waitForFunction((n) => window.demo?.pdf && document.getElementById("headline").textContent.includes("parsed") && window.demo.data && document.getElementById("details").textContent.startsWith(n), basename(file), { timeout: 60_000 });
        const state = await ready(s.page);
        check(state === "ready", `${basename(file)} opened from the file picker`);
        if (state === "ready") await alignedOk(s.page, basename(file));
        await shot(s.page, `${browserName}-file-${basename(file, ".pdf")}`);
      }
      // a file that is not a PDF: an error, then a PDF dropped on the page
      await s.page.setInputFiles("#file", { name: "notes.pdf", mimeType: "application/pdf", buffer: Buffer.from("not a PDF\n") });
      await s.page.waitForFunction(() => document.body.dataset.state === "error", null, { timeout: 60_000 });
      const error = await s.page.textContent("#headline");
      check(error === "Could not read notes.pdf.", `a file that is not a PDF: "${error}" (${await s.page.textContent("#details")})`);
      await s.page.evaluate(async () => {
        const bytes = await (await fetch("samples/scotus-transcript-p1.pdf")).arrayBuffer();
        const data = new DataTransfer();
        data.items.add(new File([bytes], "dropped.pdf", { type: "application/pdf" }));
        for (const type of ["dragenter", "dragover", "drop"])
          window.dispatchEvent(new DragEvent(type, { dataTransfer: data, bubbles: true, cancelable: true }));
      });
      await s.page.waitForFunction(() => document.getElementById("details").textContent.startsWith("dropped.pdf"), null, { timeout: 60_000 });
      check((await ready(s.page)) === "ready", "a PDF dropped on the page is opened");
      check(await s.page.locator("#drop").isHidden(), "the drop hint is hidden after the drop");
      await s.page.click("#tab-tables");
      await s.page.selectOption("#vstrategy", "text");
      await s.page.selectOption("#hstrategy", "text");
      await s.page.waitForFunction(() => document.body.dataset.state === "ready");
      const code = await s.page.evaluate(() => (document.getElementById("lang-js").click(), document.getElementById("code").textContent));
      check(code.includes('vertical_strategy: "text"'), "the table strategy reaches the code panel");
      check(!s.errors.length, `no console error ${s.errors.join(" | ")}`);
      await s.context.close();
    }

    // 4. the engine on the main thread (?worker=0)
    {
      const s = await session(browser, "main-thread");
      await s.page.goto(URL_("?worker=0&sample=scotus"));
      check((await ready(s.page)) === "ready", "?worker=0: extracted on the main thread");
      check(!s.workers.some((u) => u.endsWith("/pdfplumber/worker.js")), "?worker=0: no pdfplumber worker");
      check(!s.errors.length, `no console error ${s.errors.join(" | ")}`);
      await s.context.close();
    }

    // 5. a phone, light and dark
    if (browserName === "chromium") {
      for (const colorScheme of ["light", "dark"]) {
        const s = await session(browser, "phone", {
          viewport: { width: 390, height: 844 },
          deviceScaleFactor: 3,
          isMobile: true,
          hasTouch: true,
          colorScheme,
        });
        await s.page.goto(URL_());
        check((await ready(s.page)) === "ready", `phone (${colorScheme})`);
        const overflow = await s.page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
        check(overflow <= 0, `phone (${colorScheme}): no horizontal scroll (${overflow})`);
        await alignedOk(s.page, `phone (${colorScheme})`);
        await shot(s.page, `${browserName}-phone-${colorScheme}`);
        check(!s.errors.length, `no console error ${s.errors.join(" | ")}`);
        await s.context.close();
      }
    }
  } catch (err) {
    check(false, `${browserName}: ${err.stack ?? err}`);
  } finally {
    await browser.close();
  }
}

server.close();
console.log(failures ? `\n${failures} check(s) failed` : "\nall checks passed");
process.exit(failures ? 1 : 0);
