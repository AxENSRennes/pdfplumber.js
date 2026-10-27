# pdfplumber.js

[![npm](https://img.shields.io/npm/v/pdfplumber-js)](https://www.npmjs.com/package/pdfplumber-js)
[![license: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
![WebAssembly: 642 kB brotli](https://img.shields.io/badge/wasm-642%20kB%20brotli-informational)

[pdfplumber](https://github.com/jsvine/pdfplumber) for JavaScript, in Node and
in the browser: chars, words, text, vector objects, annotations and tables,
with the same results as Python pdfplumber.

pdfplumber.js is a faithful port of pdfplumber 0.11.9 and pdfminer.six to
Rust, compiled to WebAssembly, with a TypeScript API that follows
pdfplumber's own. It is checked against Python pdfplumber on 1,853 real-world
PDFs and about 1,600 generated edge cases: same characters, same words, same
tables. It runs entirely on the user's machine, so documents never leave it,
and it is about 6 times as fast as Python pdfplumber.

- [Why pdfplumber.js](#why-pdfplumberjs)
- [What works](#what-works)
- [What doesn't work](#what-doesnt-work)
- [Install](#install)
- [Quick start](#quick-start)
- [Bundlers and CDN](#bundlers-and-cdn)
- [Size and loading](#size-and-loading)
- [Performance](#performance)
- [Coming from Python pdfplumber](#coming-from-python-pdfplumber)
- [Coming from pdf.js text extraction](#coming-from-pdfjs-text-extraction)
- [Alternatives](#alternatives)
- [FAQ](#faq)
- [Reporting a difference with Python](#reporting-a-difference-with-python)
- [Contributing](#contributing)
- [License](#license)

## Why pdfplumber.js

- **Python pdfplumber's results, not an approximation.** The extraction
  engine is pdfminer.six and pdfplumber themselves, ported line by line,
  and its output is compared with Python's stage by stage.
- **Runs where Python can't.** Browsers, Web Workers and Node, with no
  server and no native dependencies.
- **Private by default.** PDFs are parsed on the user's device; nothing is
  uploaded.
- **More than text runs.** Characters with their font, size, color and
  position; words; lines, rectangles and curves; table detection and
  extraction; layout-preserving text; search; cropping; the structure tree
  of tagged PDFs.
- **Small.** About 660 kB to download (brotli), a third of a PDFium build and
  a twentieth of Python in the browser; the CJK character maps are fetched
  only for documents that need them.
- **Typed.** TypeScript declarations are included.

## What works

Everything pdfplumber does to extract content from a PDF:

| | JavaScript | Python pdfplumber |
|---|---|---|
| Open a PDF (path, URL, bytes, `Blob`), with a password, a page selection or layout parameters | `open(input, options)` | `pdfplumber.open(...)` |
| Pages and their geometry | `await pdf.pages`, `page.width`, `page.bbox` | `pdf.pages`, `page.width`, `page.bbox` |
| Characters, lines, rectangles, curves, images, edges | `await page.chars`, `await page.rects`... | `page.chars`, `page.rects`... |
| Text, plain or with layout | `extractText({ layout: true })` | `extract_text(layout=True)` |
| Words and text lines | `extractWords()`, `extractTextLines()` | `extract_words()`, `extract_text_lines()` |
| Search | `search(/total:?\s*\d+/i)` | `search(r"total:?\s*\d+", case=False)` |
| Tables | `findTables()`, `extractTables()`, `debugTablefinder()` | `find_tables()`, `extract_tables()`, `debug_tablefinder()` |
| Cropping and filtering | `crop()`, `withinBbox()`, `outsideBbox()`, `filter()`, `dedupeChars()` | `crop()`, `within_bbox()`... |
| Annotations, hyperlinks, metadata | `annots`, `hyperlinks`, `metadata` | same names |
| Structure tree of tagged PDFs | `structure_tree`, `pdf.structTree()` | `structure_tree`, `PDFStructTree` |
| Export | `toDict()`, `toJson()`, `toCsv()` | `to_dict()`, `to_json()`, `to_csv()` |

Encrypted PDFs (RC4, AES-128, AES-256), CJK fonts with predefined CMaps,
broken cross-reference tables and malformed content streams are handled as
pdfminer.six handles them. Every option of these methods is pdfplumber's
(`x_tolerance`, `vertical_strategy`, `keep_blank_chars`...). The full
reference is [`docs/public-api.md`](docs/public-api.md).

## What doesn't work

- **Rendering and visual debugging.** `page.to_image()` is not ported. To
  draw pages, use [pdf.js](https://github.com/mozilla/pdf.js) next to
  pdfplumber.js; both can read the same bytes.
- **Scanned PDFs.** There is no OCR, as in pdfplumber: a scanned page has
  images and no characters. Run an OCR engine such as
  [tesseract.js](https://github.com/naptha/tesseract.js) on the rendered page.
- **Creating or editing PDFs.** Out of scope; see
  [pdf-lib](https://github.com/Hopding/pdf-lib).
- **`repair=True`** (which runs Ghostscript) and pdfplumber's command line.
- **A few documented differences with Python.** `filter()` runs its test
  function when it is called, `search()` uses JavaScript `RegExp` syntax, and
  a handful of tie-breaks that depend on CPython memory addresses: see
  [Differences From Python](docs/public-api.md#differences-from-python).
- **Very large documents.** The engine runs in 32-bit WebAssembly, so one
  document and everything extracted from it must fit in 4 GB of engine
  memory.
- **Runtimes.** Node 22.12 or later (tested on Node 26) and current Chromium,
  Firefox and Safari (WebKit). Deno, Bun and edge runtimes such as Cloudflare
  Workers are not tested yet.

## Install

```sh
npm install pdfplumber-js
```

> The package is ESM. In Node, `require()` loads it through Node's
> `require(esm)` (Node 22.12 or later). TypeScript 5.2 or later reads its
> type declarations.

## Quick start

### Node

```ts
import { open } from "pdfplumber-js";

await using pdf = await open("report.pdf"); // a path, URL, bytes or Blob
const [page] = await pdf.pages;

console.log(await page.extractText());
const words = await page.extractWords({ keep_blank_chars: true });
const tables = await page.extractTables();

const cropped = await page.crop([0, 0, page.width, page.height / 2]);
for (const table of await cropped.findTables({ vertical_strategy: "text" })) {
  console.log(table.bbox, await table.extract());
}

const hits = await page.search(/total:?\s*\d+/i);
```

Every member that runs the extraction engine returns a promise; a page's
geometry (`page_number`, `bbox`, `width`, `height`) is known synchronously.
Options and object keys are pdfplumber's (`x_tolerance`, `object_type`,
`x0`, `top`...), and every method has a camelCase and a snake_case name:
`extractText` and `extract_text` are the same method.

`await using` closes and frees the document at the end of the scope. Without
it (older runtimes), call `await pdf.close()` and `await pdf.release()`.

### Browser

```html
<input type="file" accept="application/pdf" id="file" />
<script type="module">
  import { open } from "pdfplumber-js"; // through your bundler, or a CDN URL (below)

  document.getElementById("file").addEventListener("change", async (event) => {
    const pdf = await open(event.target.files[0]); // a File is a Blob
    try {
      for (const page of await pdf.pages) {
        console.log(page.page_number, await page.extractTables());
      }
    } finally {
      await pdf.close();
      await pdf.release();
    }
  });
</script>
```

### Web Worker

To keep the page responsive on large documents, run the engine in a
dedicated worker. The API stays the same:

```ts
import { configure, open } from "pdfplumber-js";

configure({ worker: true }); // before the first open()
const pdf = await open("/report.pdf"); // a URL, resolved against the page
```

## Bundlers and CDN

In a browser, the package loads `pdfplumber.wasm`, the `cmaps/` directory and
`worker.js` from next to its own files (`new URL(..., import.meta.url)`). An
application bundle moves the code, so these files must be served too:

- **Vite** (and bundlers that follow `new URL(..., import.meta.url)` and
  `new Worker(new URL(...))`) copies the module and bundles the worker. Copy
  `node_modules/pdfplumber-js/dist/cmaps` to your public directory, call
  `configure({ cmaps: { baseUrl: "/cmaps/" } })`, and in the dev server add
  `optimizeDeps: { exclude: ["pdfplumber-js"] }`.
- **esbuild** (and bundlers that leave those URLs alone): copy
  `dist/pdfplumber.wasm`, `dist/cmaps/` and `dist/browser/worker.js` (also
  exported as `pdfplumber-js/worker`) to your assets, then call
  `configure({ wasm: "/assets/pdfplumber.wasm", cmaps: { baseUrl: "/assets/cmaps/" }, worker: "/assets/worker.js" })`
  (`worker` only for worker mode).
- **No build step**: import the browser build from a CDN, pinned to a
  version; the WebAssembly module and the CMaps load from the CDN too. For
  worker mode, serve the files yourself as in the esbuild recipe: browsers do
  not start a worker from another origin.

  ```html
  <script type="module">
    import { open } from "https://cdn.jsdelivr.net/npm/pdfplumber-js@0.1.0/dist/browser/index.js";
  </script>
  ```

Both bundler recipes are built and run in Chromium as part of the test suite.

## Size and loading

What a page downloads, compressed as a web server sends it:

| File | Raw | gzip | brotli | Loaded |
|---|---|---|---|---|
| `pdfplumber.wasm` (the engine) | 2.59 MB | 920 kB | 642 kB | at the first `open()` |
| JavaScript (`index.js` and its chunk) | 46 kB | 15 kB | 14 kB | with the import |
| `worker.js` | 25 kB | 9 kB | 8 kB | in worker mode only |
| CMaps of one CJK script (e.g. Japanese) | 156 kB | | 155 kB | only for documents that use them |

That is about 0.6 s on a 9 Mbit/s mobile connection and 3.4 s on a slow
one (1.6 Mbit/s), once: after that the browser's cache serves it. Compiling
the module takes about 40 ms in Chromium; the first document opens in about
a quarter of a second, later ones faster.

For comparison, in brotli: pdf.js 5.7 is 402 kB (text runs only, no words,
tables or layout), PDFium builds are 1.7 to 1.9 MB, MuPDF.js is 3.5 MB, and
Pyodide with pdfplumber and its dependencies is about 14.8 MB.

Serve `pdfplumber.wasm` compressed (brotli or gzip: four times fewer bytes)
and as `application/wasm` (needed for streaming compilation), with a
long-lived cache and a versioned URL.

The module is compiled once per page load (or once per worker) and reused
for every document. To keep it off your first paint, load the package when
the user picks a PDF (`await import("pdfplumber-js")`).

## Performance

Milliseconds per page on 150 real-world PDFs (their first 20 pages: 302
pages), each step as pdfplumber runs it, on an 8-core Linux server in
October 2026 (browsers: median of 3 runs, engine in the page):

| ms per page | open | objects (with layout) | `extract_text` | `extract_words` | `find_tables` | total |
|---|---|---|---|---|---|---|
| Python pdfplumber 0.11.9 (CPython 3.12) | 5.02 | 118.27 | 7.31 | 4.48 | 4.69 | 139.8 |
| pdfplumber.js in Node 26 | 1.20 | 15.10 | 1.83 | 1.64 | 1.46 | 21.2 |
| pdfplumber.js in Chromium 148 | 1.16 | 15.97 | 2.00 | 1.81 | 1.71 | 22.7 |
| pdfplumber.js in Safari (WebKit 26.4) | 1.16 | 16.89 | 2.08 | 1.91 | 1.73 | 23.8 |
| pdfplumber.js in Firefox 158 | 1.86 | 18.70 | 2.35 | 2.12 | 1.87 | 26.9 |

pdfplumber.js is about 6 times as fast as Python in Node and in browsers.
With the engine in a worker, add about 15 % (results cross a thread).

## Coming from Python pdfplumber

The API is pdfplumber's, with three systematic changes:

1. **Everything that extracts is a promise**: `await pdf.pages`,
   `await page.chars`, `await page.extractText()`.
2. **Methods have a camelCase name** (`extractWords`) next to the snake_case
   one (`extract_words`); options and object keys keep pdfplumber's names.
3. **Keyword arguments become one options object**:
   `extract_words(x_tolerance=2)` is `extractWords({ x_tolerance: 2 })`.

```python
import pdfplumber

with pdfplumber.open("report.pdf") as pdf:
    page = pdf.pages[0]
    table = page.crop((0, 100, page.width, 400)).extract_table(
        {"vertical_strategy": "lines", "horizontal_strategy": "text"}
    )
```

```ts
import { open } from "pdfplumber-js";

await using pdf = await open("report.pdf");
const [page] = await pdf.pages;
const table = await (await page.crop([0, 100, page.width, 400])).extractTable({
  vertical_strategy: "lines",
  horizontal_strategy: "text",
});
```

Also good to know:

- Errors are Python's: a rejected promise with a `PDFPlumberError` whose
  `name` is the Python exception (`PdfminerException`, `ValueError`...) and
  whose `cause` is the exception it wraps.
- pdfminer objects keep their types (`PSLiteral`, `PDFObjRef`, `PDFStream`);
  Python tuples are arrays.
- `pdf.close()` is Python's `PDF.close()` (it flushes caches). The
  document's engine memory is freed by `release()`, by `await using`, or by
  the garbage collector once the document is unreachable.

## Coming from pdf.js text extraction

pdf.js's `getTextContent()` returns text runs in drawing order with a
transform matrix. pdfplumber.js gives what you would otherwise rebuild on
top of them:

| You need | pdf.js | pdfplumber.js |
|---|---|---|
| Individual characters with font, size, color and box | runs only; colors through the operator list | `await page.chars` |
| Words, with tolerances for tight or loose spacing | rebuild from runs | `extractWords({ x_tolerance })` |
| Text in reading order, or laid out like the page | rebuild from runs | `extractText()`, `extractText({ layout: true })` |
| Ruling lines, rectangles, curves | operator list | `await page.lines`, `rects`, `curves`, `edges` |
| Tables | not available | `findTables()`, `extractTables()` |

pdf.js remains the right tool to render pages; the two work side by side on
the same bytes.

## Alternatives

| | pdfplumber.js | Python pdfplumber on a server | pdfplumber in Pyodide | pdf.js `getTextContent` |
|---|---|---|---|---|
| Runs in the browser | yes | no | yes | yes |
| Same output as pdfplumber | yes | yes | yes | no |
| Words, tables, layout text | yes | yes | yes | no |
| PDFs stay on the device | yes | no | yes | yes |
| Renders pages | no | `to_image()` (pypdfium2) | `to_image()` (pypdfium2) | yes |
| What the browser downloads (brotli) | 660 kB | nothing | about 14.8 MB | 402 kB |
| License | MIT | MIT | MIT (pdfplumber), MPL-2.0 (Pyodide) | Apache-2.0 |

## FAQ

**Does it need a server?** No. Everything runs in the browser or in Node.

**How big is it?** About 660 kB compressed, downloaded once and cached: see
[Size and loading](#size-and-loading).

**Is the output exactly Python's?** Yes, except the
[documented differences](docs/public-api.md#differences-from-python). Each
stage of the pipeline (parsing, fonts, layout, objects, text, tables) is
compared with Python's on 1,853 real PDFs and on generated edge cases; see
[`docs/verification.md`](docs/verification.md).

**Can it read scanned PDFs?** No: like pdfplumber, it reads the text a PDF
contains and does no OCR.

**Why are the results promises?** CJK fonts load their character maps on
demand, and worker mode runs the engine in another thread; both are
asynchronous in a browser.

**Does it work with React, Vue, Svelte...?** Yes: it is a plain ES module. Use
worker mode for large documents so the UI stays responsive.

**Does it run on Deno, Bun or Cloudflare Workers?** It is not tested there
yet. Reports are welcome.

**How do I free memory?** Use `await using`, or call `close()` then
`release()` when you are done with a document.

## Reporting a difference with Python

pdfplumber.js aims at Python's exact output, so any difference is a bug.
Please [open an issue](https://github.com/AxENSRennes/pdfplumber.js/issues)
with:

1. the PDF (or a smaller one that shows the problem),
2. the Python code and its output (pdfplumber version),
3. the JavaScript code and its output (pdfplumber.js version, Node or
   browser).

## Contributing

The engine is ported module by module from pdfminer.six and pdfplumber, and
every change is checked against Python. [`CONTRIBUTING.md`](CONTRIBUTING.md)
explains how to build, test and investigate a difference.

## License

MIT. pdfplumber.js is an independent port of
[pdfplumber](https://github.com/jsvine/pdfplumber) (MIT, Jeremy Singer-Vine)
and [pdfminer.six](https://github.com/pdfminer/pdfminer.six) (MIT); it is not
affiliated with either project. The predefined CMaps come from Adobe's CMap
resources. Their notices are in [`THIRD_PARTY_NOTICES`](THIRD_PARTY_NOTICES).
