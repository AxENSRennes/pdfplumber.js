// pdfplumber.js demo: open a PDF in the browser, extract one page with
// pdfplumber.js (engine in a Web Worker), and draw what it found over the
// page. The page image comes from pdf.js (loaded from a CDN); every box,
// text and table comes from pdfplumber.js.
import { configure, open } from "./pdfplumber/index.js";

const PDFJS = "https://cdn.jsdelivr.net/npm/pdfjs-dist@4.10.38/";
const SAMPLES = [
  { file: "nics-background-checks-2015-11.pdf", label: "Ruled table", title: "FBI NICS firearm background checks, November 2015" },
  { file: "federal-register-2020-17221.pdf", label: "Two columns, 15 pages", title: "Federal Register, August 6, 2020" },
  { file: "scotus-transcript-p1.pdf", label: "Transcript", title: "U.S. Supreme Court oral argument transcript" },
  { file: "senate-expenditures.pdf", label: "Rotated page", title: "U.S. Senate expenditure report (page rotated 90°)" },
];
const LAYERS = [
  { id: "chars", color: "#2563eb", on: false },
  { id: "words", color: "#16a34a", on: true },
  { id: "lines", color: "#dc2626", on: false },
  { id: "rects", color: "#ea580c", on: false },
  { id: "curves", color: "#c026d3", on: false },
  { id: "tables", color: "#0891b2", on: true },
];

const params = new URLSearchParams(location.search);
const useWorker = params.get("worker") !== "0";
const lib = new URL("pdfplumber/", import.meta.url);
configure({
  wasm: new URL("pdfplumber.wasm", lib).href,
  cmaps: { baseUrl: new URL("cmaps/", lib).href },
  worker: useWorker ? new URL("worker.js", lib).href : false,
});

const $ = (id) => document.getElementById(id);
const nf = new Intl.NumberFormat("en-US");
const ms = (t) => (t < 10 ? t.toFixed(1) : nf.format(Math.round(t))) + " ms";
const plural = (n, word) => `${nf.format(n)} ${word}${n === 1 ? "" : "s"}`;
const num = (v) => (typeof v === "number" ? v.toFixed(2) : String(v));

const state = {
  name: "",
  size: 0,
  doc: null, // pdfplumber.js document
  pdfjsDoc: null, // Promise of the pdf.js document (page images)
  pdfjsSource: null, // what pdf.js reads: { buffer, password }
  pages: [],
  index: 0,
  openMs: 0,
  data: null, // what pdfplumber.js extracted from the current page
  layers: Object.fromEntries(LAYERS.map((l) => [l.id, l.on])),
  highlight: null, // [x0, top, x1, bottom] to emphasize
  hover: null,
  lang: "js",
  strategy: { v: "lines", h: "lines" },
};
let docToken = 0;
let pageToken = 0;

// ---------------------------------------------------------------- engine

/** A one-page PDF opened once at startup: loads, compiles and starts the
 * WebAssembly engine, so that the timings shown for a document are its own. */
function tinyPdf() {
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] >>",
  ];
  let out = "%PDF-1.4\n";
  const offsets = objects.map((o, i) => {
    const at = out.length;
    out += `${i + 1} 0 obj\n${o}\nendobj\n`;
    return at;
  });
  const xref = out.length;
  out += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  out += offsets.map((o) => `${String(o).padStart(10, "0")} 00000 n \n`).join("");
  out += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return new TextEncoder().encode(out);
}

const engine = (async () => {
  const t0 = performance.now();
  const pdf = await open(tinyPdf());
  const [page] = await pdf.pages;
  await page.extractText();
  await pdf.close();
  await pdf.release();
  return performance.now() - t0;
})();

const pdfjs = import(/* @vite-ignore */ PDFJS + "build/pdf.min.mjs").then(
  (lib) => {
    lib.GlobalWorkerOptions.workerSrc = PDFJS + "build/pdf.worker.min.mjs";
    return lib;
  },
  () => null,
);

// ---------------------------------------------------------------- documents

async function openBytes(buffer, name, password) {
  const token = ++docToken;
  setBusy(true);
  state.name = name;
  state.size = buffer.byteLength;
  const previous = state.doc;
  state.doc = null;
  if (previous) previous.close().then(() => previous.release()).catch(() => {});
  try {
    const engineMs = await engine;
    const options = password === undefined ? undefined : { password };
    const t0 = performance.now();
    const doc = await open(buffer.slice(0), options);
    const pages = await doc.pages;
    const openMs = performance.now() - t0;
    if (token !== docToken) {
      doc.close().then(() => doc.release());
      return;
    }
    // pdf.js reads the file once the first page is extracted (renderImage),
    // so that the timings shown are pdfplumber.js's alone
    state.pdfjsDoc = null;
    state.pdfjsSource = { buffer, password };
    Object.assign(state, { doc, pages, openMs, engineMs });
    const select = $("page");
    select.replaceChildren(...pages.map((p, i) => new Option(String(p.page_number), String(i))));
    $("pagecount").textContent = `of ${nf.format(pages.length)}`;
    const wanted = Number(params.get("page")) - 1;
    params.delete("page");
    await showPage(wanted > 0 && wanted < pages.length ? wanted : 0);
  } catch (err) {
    if (token !== docToken) return;
    if (password === undefined && /password|encrypt/i.test(describe(err))) {
      const pw = window.prompt(`"${name}" is protected by a password. Password:`);
      if (pw !== null) return openBytes(buffer, name, pw);
    }
    showError(err);
  }
}

async function showPage(index) {
  const token = ++pageToken;
  const page = state.pages[index];
  if (!page) return;
  state.index = index;
  state.highlight = state.hover = null;
  syncPager();
  setBusy(true);
  try {
    const t0 = performance.now();
    const [chars, lines, rects, curves, words, text, layout, tables] = await Promise.all([
      page.chars,
      page.lines,
      page.rects,
      page.curves,
      page.extractWords(),
      page.extractText(),
      page.extractText({ layout: true }),
      findTables(page),
    ]);
    const pageMs = performance.now() - t0;
    if (token !== pageToken) return;
    // tablesMs: the time of findTables alone (refreshTables); here it waited
    // for the calls queued before it
    state.data = { page, chars, lines, rects, curves, words, text, layout, tables: tables.list, tablesMs: null, pageMs };
    renderStatus();
    renderText();
    renderWords();
    renderTables();
    renderCode();
    image.hidden = true; // until pdf.js draws this page
    delete image.dataset.drawn;
    imageBox = null;
    layoutStage();
    drawOverlay();
    renderImage(token);
    setBusy(false);
  } catch (err) {
    if (token === pageToken) showError(err);
  }
}

function tableSettings() {
  const { v, h } = state.strategy;
  return v === "lines" && h === "lines" ? undefined : { vertical_strategy: v, horizontal_strategy: h };
}

async function findTables(page) {
  const t0 = performance.now();
  const found = await page.findTables(tableSettings());
  const list = await Promise.all(found.map(async (t) => ({ bbox: t.bbox, cells: t.cells, rows: await t.extract() })));
  return { list, ms: performance.now() - t0 };
}

async function refreshTables() {
  const data = state.data;
  if (!data) return;
  const token = pageToken;
  setBusy(true);
  try {
    const tables = await findTables(data.page);
    if (token !== pageToken) return;
    Object.assign(data, { tables: tables.list, tablesMs: tables.ms });
    renderTables();
    renderCode();
    renderCounts();
    drawOverlay();
    setBusy(false);
  } catch (err) {
    showError(err);
  }
}

// ---------------------------------------------------------------- status

function setBusy(busy) {
  $("busy").hidden = !busy;
  document.body.dataset.state = busy ? "busy" : "ready";
}

function showError(err) {
  console.warn(err);
  setBusy(false);
  document.body.dataset.state = "error";
  $("headline").textContent = `Could not read ${state.name || "this PDF"}.`;
  $("details").textContent = describe(err);
}

/** "Name: message", and the Python exception it wraps (pdfplumber's
 * PdfminerException wraps pdfminer's, e.g. PDFPasswordIncorrect). */
function describe(err) {
  const one = (e) => [e?.name, e?.message].filter(Boolean).join(": ") || String(e);
  return err?.cause ? `${one(err)} (${one(err.cause)})` : one(err);
}

function renderStatus() {
  const d = state.data;
  $("headline").innerHTML = "";
  $("headline").append(
    `${state.name}, page ${d.page.page_number}: parsed in `,
    Object.assign(document.createElement("strong"), { textContent: ms(d.pageMs) }),
    ".",
  );
  renderCounts();
}

function renderCounts() {
  const d = state.data;
  $("details").replaceChildren();
  $("count-words").textContent = nf.format(d.words.length);
  $("count-tables").textContent = nf.format(d.tables.length);
}

function syncPager() {
  const n = state.pages.length;
  $("page").value = String(state.index);
  $("page").disabled = n < 2;
  $("prev").disabled = state.index <= 0;
  $("next").disabled = state.index >= n - 1;
}

// ---------------------------------------------------------------- page view

const stage = $("stage");
const overlay = $("overlay");
const image = $("render");
let scale = 1;
let renderTask = null;
let imageBox = null; // [x, top, width, height] of the pdf.js image, in page points

function layoutStage() {
  const page = state.data?.page;
  if (!page) return;
  const width = stage.clientWidth;
  scale = width / page.width;
  stage.style.height = `${page.height * scale}px`;
  stage.style.minHeight = "0";
  placeImage();
}

function placeImage() {
  if (!imageBox || !state.data) return;
  const [bx, by] = state.data.page.bbox;
  const [x, y, w, h] = imageBox;
  Object.assign(image.style, {
    left: `${(x - bx) * scale}px`,
    top: `${(y - by) * scale}px`,
    width: `${w * scale}px`,
    height: `${h * scale}px`,
  });
}

function drawOverlay() {
  const d = state.data;
  if (!d) return;
  const dpr = window.devicePixelRatio || 1;
  const w = stage.clientWidth;
  const h = Math.round(d.page.height * scale);
  overlay.width = Math.round(w * dpr);
  overlay.height = Math.round(h * dpr);
  overlay.style.width = `${w}px`;
  overlay.style.height = `${h}px`;
  const ctx = overlay.getContext("2d");
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.clearRect(0, 0, overlay.width, overlay.height);
  const [bx, by] = d.page.bbox;
  const k = dpr * scale;
  ctx.setTransform(k, 0, 0, k, -bx * k, -by * k);
  const px = 1 / scale; // one CSS pixel, in PDF points
  const on = state.layers;
  const color = Object.fromEntries(LAYERS.map((l) => [l.id, l.color]));
  const box = (o) => ctx.rect(o.x0, o.top, o.x1 - o.x0, o.bottom - o.top);
  const path = (pts) => pts.forEach(([x, y], i) => (i ? ctx.lineTo(x, y) : ctx.moveTo(x, y)));

  if (on.tables) {
    for (const t of d.tables) {
      ctx.fillStyle = "rgba(8, 145, 178, 0.10)";
      ctx.fillRect(t.bbox[0], t.bbox[1], t.bbox[2] - t.bbox[0], t.bbox[3] - t.bbox[1]);
      ctx.beginPath();
      for (const c of t.cells) ctx.rect(c[0], c[1], c[2] - c[0], c[3] - c[1]);
      ctx.strokeStyle = "rgba(8, 145, 178, 0.75)";
      ctx.lineWidth = 1.5 * px;
      ctx.stroke();
      ctx.strokeStyle = color.tables;
      ctx.lineWidth = 2.5 * px;
      ctx.strokeRect(t.bbox[0], t.bbox[1], t.bbox[2] - t.bbox[0], t.bbox[3] - t.bbox[1]);
    }
  }
  const stroke = (id, width, draw) => {
    if (!on[id]) return;
    ctx.beginPath();
    draw();
    ctx.strokeStyle = color[id];
    ctx.lineWidth = width * px;
    ctx.stroke();
  };
  stroke("rects", 1, () => d.rects.forEach(box));
  stroke("curves", 1.5, () => d.curves.forEach((c) => path(c.pts)));
  stroke("lines", 1.5, () => d.lines.forEach((l) => path(l.pts)));
  ctx.globalAlpha = 0.85;
  stroke("words", 1, () => d.words.forEach(box));
  ctx.globalAlpha = 0.6;
  stroke("chars", 0.75, () => d.chars.forEach(box));
  ctx.globalAlpha = 1;

  for (const b of [state.highlight, state.hover]) {
    if (!b) continue;
    ctx.fillStyle = "rgba(250, 204, 21, 0.4)";
    ctx.fillRect(b[0], b[1], b[2] - b[0], b[3] - b[1]);
    ctx.strokeStyle = "#a16207";
    ctx.lineWidth = 1.5 * px;
    ctx.strokeRect(b[0], b[1], b[2] - b[0], b[3] - b[1]);
  }
}

/** The pdf.js document of `buffer`, or null when pdf.js cannot load or
 * read it. pdf.js gets its own copy: it transfers the bytes to its worker. */
async function loadPdfjs({ buffer, password }) {
  const lib = await pdfjs;
  if (!lib) return null;
  return lib
    .getDocument({
      data: new Uint8Array(buffer.slice(0)),
      password,
      cMapUrl: PDFJS + "cmaps/",
      cMapPacked: true,
      standardFontDataUrl: PDFJS + "standard_fonts/",
      isEvalSupported: false,
    })
    .promise.catch(() => null);
}

/** Draws the page with pdf.js under the overlay. pdf.js draws the crop box;
 * pdfplumber's coordinates start at the page's `bbox` (the media box), so
 * the image is placed at the crop box's offset. */
async function renderImage(token) {
  const d = state.data;
  const note = $("render-note");
  state.pdfjsDoc ??= loadPdfjs(state.pdfjsSource);
  const jsdoc = await state.pdfjsDoc;
  if (token !== pageToken) return;
  if (!jsdoc) {
    image.hidden = true;
    image.dataset.drawn = "none";
    note.textContent = "The page image is not available (pdf.js could not load or read this file): the overlays are drawn on a blank page.";
    return;
  }
  note.textContent = "";
  try {
    const p = await jsdoc.getPage(d.page.page_number);
    if (token !== pageToken) return;
    const dpr = Math.min(window.devicePixelRatio || 1, 3);
    const k = scale * dpr;
    const viewport = p.getViewport({ scale: k });
    const cb = d.page.cropbox ?? d.page.bbox;
    renderTask?.cancel();
    const canvas = document.createElement("canvas");
    canvas.width = Math.round(viewport.width);
    canvas.height = Math.round(viewport.height);
    renderTask = p.render({ canvasContext: canvas.getContext("2d"), viewport });
    await renderTask.promise;
    if (token !== pageToken) return;
    image.width = canvas.width;
    image.height = canvas.height;
    image.getContext("2d").drawImage(canvas, 0, 0);
    // where the image goes, in page points (layoutStage scales it)
    imageBox = [cb[0], cb[1], viewport.width / k, viewport.height / k];
    placeImage();
    image.hidden = false;
    image.dataset.drawn = String(d.page.page_number);
  } catch (err) {
    if (err?.name !== "RenderingCancelledException") console.warn(err);
  }
}

let resizeTimer = 0;
let lastWidth = 0;
new ResizeObserver(() => {
  if (stage.clientWidth === lastWidth) return;
  lastWidth = stage.clientWidth;
  if (!state.data) return;
  layoutStage(); // the image is stretched until it is drawn again at this size
  drawOverlay();
  clearTimeout(resizeTimer);
  if (!image.hidden) delete image.dataset.drawn;
  resizeTimer = setTimeout(() => renderImage(pageToken), 150);
}).observe(stage);

function pointAt(event) {
  const r = overlay.getBoundingClientRect();
  const [bx, by] = state.data.page.bbox;
  return { x: (event.clientX - r.left) / scale + bx, y: (event.clientY - r.top) / scale + by, cx: event.clientX - r.left, cy: event.clientY - r.top };
}

const inside = (o, p) => p.x >= o.x0 && p.x <= o.x1 && p.y >= o.top && p.y <= o.bottom;

function onPointer(event) {
  const d = state.data;
  if (!d) return;
  const p = pointAt(event);
  const word = d.words.find((w) => inside(w, p));
  const char = d.chars.find((c) => inside(c, p));
  const tip = $("tip");
  const target = word ?? char;
  state.hover = target ? [target.x0, target.top, target.x1, target.bottom] : null;
  drawOverlay();
  if (!target) {
    tip.hidden = true;
    return;
  }
  tip.replaceChildren();
  const strong = document.createElement("strong");
  strong.textContent = word ? word.text : char.text;
  const small = document.createElement("span");
  const font = char ? `${char.fontname} · ${num(char.size)} pt` : "";
  small.textContent = [font, `x0 ${num(target.x0)} · top ${num(target.top)}`].filter(Boolean).join("\n");
  tip.append(strong, small);
  tip.hidden = false;
  const left = Math.min(p.cx + 14, stage.clientWidth - tip.offsetWidth - 4);
  const top = p.cy + 18 + tip.offsetHeight > stage.clientHeight ? p.cy - tip.offsetHeight - 10 : p.cy + 18;
  tip.style.left = `${Math.max(4, left)}px`;
  tip.style.top = `${Math.max(4, top)}px`;
}

overlay.addEventListener("pointermove", onPointer);
overlay.addEventListener("pointerdown", onPointer);
overlay.addEventListener("pointerleave", () => {
  $("tip").hidden = true;
  state.hover = null;
  drawOverlay();
});

// ---------------------------------------------------------------- panels

function renderText() {
  const d = state.data;
  $("text").textContent = d.text || "(no text on this page)";
  $("layout").textContent = d.layout.trim() ? d.layout : "(no text on this page)";
}

const MAX_WORDS = 3000;

function renderWords() {
  const d = state.data;
  const table = $("words");
  const head = document.createElement("thead");
  head.innerHTML = "<tr><th>#</th><th>text</th><th>x0</th><th>top</th><th>x1</th><th>bottom</th></tr>";
  const body = document.createElement("tbody");
  d.words.slice(0, MAX_WORDS).forEach((w, i) => {
    const tr = body.insertRow();
    tr.dataset.i = String(i);
    for (const v of [i + 1, w.text, num(w.x0), num(w.top), num(w.x1), num(w.bottom)]) tr.insertCell().textContent = v;
  });
  table.replaceChildren(head, body);
  if (d.words.length > MAX_WORDS) {
    const tr = body.insertRow();
    const td = tr.insertCell();
    td.colSpan = 6;
    td.className = "more";
    td.textContent = `… and ${nf.format(d.words.length - MAX_WORDS)} more`;
  }
  if (!d.words.length) table.innerHTML = '<tbody><tr><td class="more">No words on this page.</td></tr></tbody>';
}

$("words").addEventListener("pointerover", (e) => {
  const tr = e.target.closest("tr[data-i]");
  const w = tr && state.data.words[Number(tr.dataset.i)];
  state.highlight = w ? [w.x0, w.top, w.x1, w.bottom] : null;
  drawOverlay();
});
$("words").addEventListener("pointerleave", () => {
  state.highlight = null;
  drawOverlay();
});

function renderTables() {
  const d = state.data;
  const root = $("tables");
  root.replaceChildren();
  if (!d.tables.length) {
    const p = document.createElement("p");
    p.className = "empty";
    p.textContent =
      state.strategy.v === "lines" || state.strategy.h === "lines"
        ? 'No table found. Try "text" for tables without ruling lines.'
        : "No table found.";
    root.append(p);
    return;
  }
  d.tables.forEach((t, i) => {
    const figure = document.createElement("figure");
    figure.className = "table";
    figure.dataset.i = String(i);
    const caption = document.createElement("figcaption");
    const cols = Math.max(0, ...t.rows.map((r) => r.length));
    caption.innerHTML = `<strong>Table ${i + 1}</strong> · ${t.rows.length} × ${cols}`;
    const wrap = document.createElement("div");
    wrap.className = "scroll";
    const table = document.createElement("table");
    const body = table.createTBody();
    for (const row of t.rows) {
      const tr = body.insertRow();
      for (const cell of row) {
        const td = tr.insertCell();
        if (cell === null) td.className = "null";
        else td.textContent = cell;
      }
    }
    wrap.append(table);
    figure.append(caption, wrap);
    root.append(figure);
  });
}

$("tables").addEventListener("pointerover", (e) => {
  const fig = e.target.closest("figure[data-i]");
  const t = fig && state.data.tables[Number(fig.dataset.i)];
  state.highlight = t ? t.bbox : null;
  drawOverlay();
});
$("tables").addEventListener("pointerleave", () => {
  state.highlight = null;
  drawOverlay();
});

for (const [id, key] of [
  ["vstrategy", "v"],
  ["hstrategy", "h"],
]) {
  $(id).addEventListener("change", (e) => {
    state.strategy[key] = e.target.value;
    refreshTables();
  });
}

// ---------------------------------------------------------------- code

function codeFor(lang) {
  const d = state.data;
  const n = d.page.page_number;
  const of = state.pages.length;
  const { v, h } = state.strategy;
  const custom = tableSettings();
  const c = (count, word) => plural(count, word);
  if (lang === "py") {
    const settings = custom ? `{"vertical_strategy": "${v}", "horizontal_strategy": "${h}"}` : "";
    return [
      "import pdfplumber",
      "",
      `with pdfplumber.open(${JSON.stringify(state.name)}) as pdf:`,
      `    page = pdf.pages[${n - 1}]  # page ${n} of ${of}`,
      "",
      `    chars = page.chars  # ${c(d.chars.length, "char")}`,
      `    words = page.extract_words()  # ${c(d.words.length, "word")}`,
      "    text = page.extract_text()",
      "    layout = page.extract_text(layout=True)",
      `    lines, rects = page.lines, page.rects  # ${d.lines.length} and ${d.rects.length}`,
      "",
      `    tables = page.find_tables(${settings})  # ${c(d.tables.length, "table")}`,
      "    for table in tables:",
      "        print(table.bbox, table.extract())",
    ].join("\n");
  }
  const settings = custom ? `{ vertical_strategy: "${v}", horizontal_strategy: "${h}" }` : "";
  return [
    'import { configure, open } from "pdfplumber-js";',
    "",
    `configure({ worker: ${useWorker} }); // ${useWorker ? "run the engine in a Web Worker" : "run the engine in the page"}`,
    "",
    "const pdf = await open(file); // a File, Blob, URL or bytes",
    `const page = (await pdf.pages)[${n - 1}]; // page ${n} of ${of}`,
    "",
    `const chars = await page.chars; // ${c(d.chars.length, "char")}`,
    `const words = await page.extractWords(); // ${c(d.words.length, "word")}`,
    "const text = await page.extractText();",
    "const layout = await page.extractText({ layout: true });",
    `const lines = await page.lines; // ${c(d.lines.length, "line")}`,
    `const rects = await page.rects; // ${c(d.rects.length, "rect")}`,
    "",
    `const tables = await page.findTables(${settings}); // ${c(d.tables.length, "table")}`,
    "for (const table of tables) {",
    "  console.log(table.bbox, await table.extract());",
    "}",
    "",
    "await pdf.close();",
    "await pdf.release();",
  ].join("\n");
}

function highlightCode(src, lang) {
  const comment = lang === "py" ? "#.*" : "//.*";
  const keywords =
    lang === "py"
      ? "import|from|with|as|for|in|print"
      : "import|from|const|await|for|of|true|false";
  const re = new RegExp(`(${comment})|("(?:[^"\\\\]|\\\\.)*")|\\b(${keywords})\\b`, "g");
  const esc = (s) => s.replace(/[&<>]/g, (ch) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[ch]);
  let out = "";
  let last = 0;
  for (const m of src.matchAll(re)) {
    out += esc(src.slice(last, m.index));
    const cls = m[1] ? "c" : m[2] ? "s" : "k";
    out += `<span class="${cls}">${esc(m[0])}</span>`;
    last = m.index + m[0].length;
  }
  return out + esc(src.slice(last));
}

function renderCode() {
  if (!state.data) return;
  $("code").innerHTML = highlightCode(codeFor(state.lang), state.lang);
}

for (const lang of ["js", "py"]) {
  $(`lang-${lang}`).addEventListener("click", () => {
    state.lang = lang;
    $("lang-js").setAttribute("aria-pressed", String(lang === "js"));
    $("lang-py").setAttribute("aria-pressed", String(lang === "py"));
    renderCode();
  });
}

$("copy").addEventListener("click", async () => {
  try {
    await navigator.clipboard.writeText(codeFor(state.lang));
    $("copy").textContent = "Copied";
  } catch {
    $("copy").textContent = "Select and copy";
  }
  setTimeout(() => ($("copy").textContent = "Copy"), 1500);
});

// ---------------------------------------------------------------- tabs

const tabs = [...document.querySelectorAll('[role="tab"]')];
function selectTab(tab) {
  for (const t of tabs) {
    const selected = t === tab;
    t.setAttribute("aria-selected", String(selected));
    t.tabIndex = selected ? 0 : -1;
    $(t.getAttribute("aria-controls")).hidden = !selected;
  }
}
tabs.forEach((tab, i) => {
  tab.addEventListener("click", () => selectTab(tab));
  tab.addEventListener("keydown", (e) => {
    const step = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
    if (!step) return;
    const next = tabs[(i + step + tabs.length) % tabs.length];
    selectTab(next);
    next.focus();
  });
});

// ---------------------------------------------------------------- inputs

function buildLayers() {
  const root = $("layers");
  for (const l of LAYERS) {
    const label = document.createElement("label");
    label.className = "layer";
    label.style.setProperty("--swatch", l.color);
    const input = document.createElement("input");
    input.type = "checkbox";
    input.checked = l.on;
    input.addEventListener("change", () => {
      state.layers[l.id] = input.checked;
      drawOverlay();
    });
    label.append(input, l.id);
    root.append(label);
  }
}

function buildSamples() {
  const root = $("samples");
  for (const s of SAMPLES) {
    const b = document.createElement("button");
    b.className = "chip";
    b.textContent = s.label;
    b.title = s.title;
    b.dataset.file = s.file;
    b.addEventListener("click", () => loadSample(s));
    root.append(b);
  }
}

function markSample(file) {
  for (const b of $("samples").children) b.setAttribute("aria-pressed", String(b.dataset.file === file));
}

async function loadSample(s) {
  markSample(s.file);
  setBusy(true);
  $("headline").textContent = `Opening ${s.file}…`;
  try {
    const res = await fetch(new URL(`samples/${s.file}`, import.meta.url));
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    await openBytes(await res.arrayBuffer(), s.file);
  } catch (err) {
    state.name = s.file;
    showError(err);
  }
}

async function loadFile(file) {
  if (!file) return;
  markSample(null);
  $("headline").textContent = `Opening ${file.name}…`;
  setBusy(true);
  await openBytes(await file.arrayBuffer(), file.name);
}

$("file").addEventListener("change", (e) => {
  loadFile(e.target.files[0]);
  e.target.value = "";
});
$("page").addEventListener("change", (e) => showPage(Number(e.target.value)));
$("prev").addEventListener("click", () => showPage(state.index - 1));
$("next").addEventListener("click", () => showPage(state.index + 1));
document.addEventListener("keydown", (e) => {
  if (e.target.closest("input, select, textarea, [role=tab]") || e.altKey || e.ctrlKey || e.metaKey) return;
  if (e.key === "ArrowLeft" && state.index > 0) showPage(state.index - 1);
  if (e.key === "ArrowRight" && state.index < state.pages.length - 1) showPage(state.index + 1);
});

const drop = $("drop");
let dragDepth = 0;
const hasFiles = (e) => [...(e.dataTransfer?.types ?? [])].includes("Files");
window.addEventListener("dragenter", (e) => {
  if (!hasFiles(e)) return;
  e.preventDefault();
  dragDepth++;
  drop.hidden = false;
});
window.addEventListener("dragover", (e) => {
  if (hasFiles(e)) e.preventDefault();
});
window.addEventListener("dragleave", () => {
  if (--dragDepth <= 0) {
    dragDepth = 0;
    drop.hidden = true;
  }
});
window.addEventListener("drop", (e) => {
  if (!hasFiles(e)) return;
  e.preventDefault();
  dragDepth = 0;
  drop.hidden = true;
  loadFile(e.dataTransfer.files[0]);
});

// ---------------------------------------------------------------- start

// for the console: demo.page is the current PDFPlumberPage
window.demo = {
  get pdf() {
    return state.doc;
  },
  get page() {
    return state.data?.page;
  },
  get data() {
    return state.data;
  },
};

buildLayers();
buildSamples();
engine.catch(showError);
const first = params.get("sample");
loadSample(SAMPLES.find((s) => s.file === first || s.file.startsWith(first ?? "\0")) ?? SAMPLES[0]);
