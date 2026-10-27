# pdfplumber.js

[pdfplumber](https://github.com/jsvine/pdfplumber) for JavaScript, in Node and
in the browser: chars, words, text, vector objects, annotations and tables,
with the same results as Python pdfplumber.

The core is a faithful Rust port of pdfplumber and pdfminer.six, compiled to
WebAssembly behind a coarse-grained JavaScript API. Every stage of the pipeline
is checked against Python (pdfplumber 0.11.9, pdfminer.six 20260107,
Python ≥ 3.12) by comparing canonical traces on real and generated PDFs.

## Quick start

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
geometry (`page_number`, `bbox`, `width`, `height`, the boxes) is known
synchronously. Options and object keys are pdfplumber's (`x_tolerance`,
`vertical_strategy`, `object_type`, `x0`, `top`...); methods have both a
camelCase and a snake_case name (`extractText` and `extract_text` are the
same member). Results are those of Python pdfplumber, verified against it
through this API in Node, Chromium, Firefox and WebKit, in the page and in a
worker. The whole contract — every member, option, value, error, and what is
not ported — is [`docs/public-api.md`](docs/public-api.md).

In a browser, the package loads `dist/pdfplumber.wasm` and the CMaps of
`dist/cmaps/` next to itself; `configure({ wasm, cmaps, worker: true })` puts
them elsewhere or runs the engine in a dedicated worker.

## Status

The engine (pdfminer.six and pdfplumber's page layer) and the public API are
ported and verified (docs/architecture.md, "Vague 5"). Not ported:
`Page.to_image` (`display.py`), `repair=True` (Ghostscript), the command line.

- Why and how: [`docs/port-strategy.md`](https://github.com/AxENSRennes/pdfplumber.js/blob/HEAD/docs/port-strategy.md)
- Layout and porting rules: [`docs/architecture.md`](https://github.com/AxENSRennes/pdfplumber.js/blob/HEAD/docs/architecture.md)
- Verification method: [`docs/verification.md`](https://github.com/AxENSRennes/pdfplumber.js/blob/HEAD/docs/verification.md)
- Public JavaScript API: [`docs/public-api.md`](docs/public-api.md)
- Runtime (WebAssembly, CMaps, workers): [`docs/js-runtime.md`](docs/js-runtime.md)

## Performance

Milliseconds per page on the `quick` set (150 files, their first 20
pages: 302 pages), each phase as pdfplumber runs it, on a shared 8-core
Linux VPS (one benchmark at a time; median of 3 runs, Python 1 run;
measured at the integration of wave 5 on `rewrite/rust-core`, the
Chromium row at the wave's join, a few commits earlier):

| ms per page | open | objects (layout included) | `extract_text` | `extract_words` | `find_tables` | total | `extract_tables` |
|---|---|---|---|---|---|---|---|
| Python pdfplumber 0.11.9 (CPython 3.12) | 5.02 | 118.27 | 7.31 | 4.48 | 4.69 | 139.8 | 5.20 |
| pdfplumber-js in Node 26, public API | 1.20 | 15.10 | 1.83 | 1.64 | 1.46 | 21.2 | 1.27 |
| the Rust core, native (no JavaScript) | 0.32 | 10.68 | 1.72 | 1.36 | 1.71 | 15.8 | 1.12 |
| pdfplumber-js in Chromium 148, public API (join) | 1.11 | 15.64 | 2.02 | 1.79 | 1.68 | 22.2 | 1.37 |

The public API in Node is about 6.6 times as fast as Python on these
pages, native Rust 8.9 times. Chromium was measured at the join, against
a Python run of 134.3 ms per page there (Python's time varies by a few
percent from run to run on this machine): 6.0 times as fast as that run,
5.3 with the engine in a worker; WebKit is on par with Chromium,
Playwright's Firefox Nightly about five times slower. `objects` includes
moving every object into JavaScript values (about 3.2 ms of its 15.1 ms:
a second read, the objects already computed). The WebAssembly module is
2.58 MB (662 kB brotli); the CJK CMaps are loaded only when a document
needs them. Method, per-phase history and memory:
[`docs/architecture.md`](https://github.com/AxENSRennes/pdfplumber.js/blob/HEAD/docs/architecture.md),
"Intégration (branche `rewrite/rust-core`)" and "Jonction (branche
`wave5/join`)".

## Migrating from the pdf.js-based port (`legacy-pdfjs`)

The previous port (tag `legacy-pdfjs`) reimplemented pdfplumber over pdf.js.
This one runs pdfminer.six and pdfplumber themselves, ported to Rust, so
results change wherever the old port differed from Python. The API keeps
most of its names, with these changes:

- **Everything that extracts is a promise.** `pdf.pages` is
  `await pdf.pages`; object lists are getters returning promises
  (`await page.chars`); `extractText()`, `extractWords()`, `search()`,
  tables and derived pages (`crop`, `filter`...) resolve asynchronously.
- **`close()` is Python's `PDF.close()`**: it flushes caches; the document
  stays usable, except, for a document opened by path in Node, what was
  not read before (the file is closed: Python's `ValueError: seek of
  closed file`, wrapped in `PdfminerException`). Free a document with
  `release()`, or with `await using pdf = await open(...)` (close then
  release). Unreachable documents, derived pages and tables are freed by
  the garbage collector.
- **Errors are Python's**: a `PDFPlumberError` whose `name` is the Python
  exception (`PdfminerException`, `ValueError`...), `cause` the exception it
  wraps; `isInstance("PDFSyntaxError")` answers like `isinstance`. Removed
  features reject with `NotImplemented`.
- **Values are pdfplumber's**: keys in pdfplumber's order, `PSLiteral`,
  `PDFObjRef`, `PDFStream` (a reference whose `rawdata`/`data` are read on
  demand) and `PageRef` (`await pdf.page(ref)`) for pdfminer objects;
  colours and other tuples are arrays.
- **New members**: `extractTextSimple`, `debugTablefinder`, `toDict`,
  `toJson`, `toCsv`, `structure_tree` and `pdf.structTree(page?)`
  (`findAll`, `find`, `elementBbox`, `allMcids`), `annots`/`hyperlinks` with
  pdfplumber's `data`, every `open()` keyword of `pdfplumber.open`.
- **Removed or renamed**:
  - the pdf.js dependency and its worker (`dist/browser/pdf.worker.js`;
    worker mode is `configure({ worker: true })`), the old compatibility
    scripts (`test:compat`, `test:parity`, `contract:dashboard`);
  - `page.label` and `page.figures` (pdfplumber has neither; with
    `laparams`, layout figures are in `page.objects`);
  - the options `limit` and `slice` of the text methods: pdfplumber's own
    keyword arguments only (`extractWords({ limit: 5 })` is Python's
    `TypeError: ... got an unexpected keyword argument 'limit'`; an
    unknown option of `extractText` is ignored as pdfplumber ignores it);
  - the types `BBox` (now `Bbox`), `Dir` (`TextDirection`), `TextOptions`
    and `LimitSliceOptions` (gone: each method has its options type);
    `TableAxisGroup` is a type only (rows and columns are plain objects);
  - `PDFPlumberWarning.source` is the Python logger or warning category
    (`pdfminer.pdfinterp`, `UserWarning`...), not `"pdfjs" | "native"`,
    and a warning has a `level`.
- **`search()`** keeps JavaScript `RegExp` semantics (Python's `re` is not
  ported); the text it runs over and its results are pdfplumber's.

## Requirements and bundlers

- Node 22.12 or later (the package is ESM; `require()` loads it through
  Node's `require(esm)`), or a browser with WebAssembly and ES2022
  (Chromium, Firefox and WebKit are tested). The tests run on Node 26.
- `await using` (in the quick start) needs a runtime with explicit
  resource management (Node 24+, recent browsers) or a compiler that
  lowers it; without it, call `await pdf.close()` and `await
  pdf.release()`. The type declarations need TypeScript 5.2 or later;
  they bring `esnext.disposable` themselves and do not need the DOM
  library.
- In a browser, the package finds `pdfplumber.wasm`, `cmaps/` and
  `worker.js` next to its own files (`new URL(..., import.meta.url)`).
  An application bundle moves the code, not these files:
  - **Vite** (and other bundlers that follow `new URL(..., import.meta.url)`
    and `new Worker(new URL(...))`) copies the module and bundles the
    worker; copy `node_modules/pdfplumber-js/dist/cmaps` to the public
    directory and call `configure({ cmaps: { baseUrl: "/cmaps/" } })`. In
    the dev server, add `optimizeDeps: { exclude: ["pdfplumber-js"] }`
    (pre-bundling rewrites the URLs).
  - **esbuild** (and any bundler that leaves those URLs alone): copy
    `dist/pdfplumber.wasm`, `dist/cmaps/` and `dist/browser/worker.js`
    (one file, also exported as `pdfplumber-js/worker`) where the
    application serves them, and call `configure({ wasm:
    "/assets/pdfplumber.wasm", cmaps: { baseUrl: "/assets/cmaps/" },
    worker: "/assets/worker.js" })` (`worker` only for worker mode).
  - A worker script that cannot load rejects `open()` with an error naming
    its URL. `scripts/check-browsers.sh` builds these two recipes from the
    packed package and runs them in Chromium (page and worker, a CJK
    document).

## Development

```sh
scripts/setup-dev.sh   # submodules, Python oracle (.venv), Node, Rust + wasm32, external PDFs
scripts/check.sh       # everything below, as CI would run it
```

Build (Rust workspace in `crates/`, toolchain pinned by `rust-toolchain.toml`):

```sh
cargo build --release -p pdftrace                                         # trace binary, target/release/pdftrace
cargo build --target wasm32-unknown-unknown --profile dist -p pdfplumber-wasm
```

Test: the Rust tests replay cases computed by Python (`oracle/unit/`), which
live in `.cache/unit/` and are regenerated when missing or stale:

```sh
PYTHONPATH=. .venv/bin/python -m oracle.unit       # unit oracle data (the real psparser corpus takes a few minutes)
cargo test --release
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings
PYTHONPATH=. .venv/bin/python -m unittest discover -s oracle/tests -t .   # oracle self-tests
```

Verify a pipeline stage against Python on a PDF set (`quick`, `all`, ...):

```sh
PYTHONPATH=. .venv/bin/python -m oracle.verify --stage psparser --set quick
```

In a git worktree, set `PDFPLUMBER_ROOT` to the main checkout (downloaded
PDFs, submodules, `.venv`, `.cache`). Details: [`oracle/README.md`](https://github.com/AxENSRennes/pdfplumber.js/blob/HEAD/oracle/README.md)
and [`docs/verification.md`](https://github.com/AxENSRennes/pdfplumber.js/blob/HEAD/docs/verification.md).
