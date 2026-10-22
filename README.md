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

- Why and how: [`docs/port-strategy.md`](docs/port-strategy.md)
- Layout and porting rules: [`docs/architecture.md`](docs/architecture.md)
- Verification method: [`docs/verification.md`](docs/verification.md)
- Public JavaScript API: [`docs/public-api.md`](docs/public-api.md)
- Runtime (WebAssembly, CMaps, workers): [`docs/js-runtime.md`](docs/js-runtime.md)

## Migrating from the pdf.js-based port (`legacy-pdfjs`)

The previous port (tag `legacy-pdfjs`) reimplemented pdfplumber over pdf.js.
This one runs pdfminer.six and pdfplumber themselves, ported to Rust, so
results change wherever the old port differed from Python. The API keeps
its names, with these changes:

- **Everything that extracts is a promise.** `pdf.pages` is
  `await pdf.pages`; object lists are getters returning promises
  (`await page.chars`); `extractText()`, `extractWords()`, `search()`,
  tables and derived pages (`crop`, `filter`...) resolve asynchronously.
- **`close()` is Python's `PDF.close()`**: it flushes caches and the
  document stays usable. Free a document with `release()`, or with
  `await using pdf = await open(...)` (close then release). Unreachable
  documents, derived pages and tables are freed by the garbage collector.
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
- **Removed**: the pdf.js dependency and its worker (`dist/browser/pdf.worker.js`;
  worker mode is `configure({ worker: true })`), `page.label`, the old
  compatibility scripts (`test:compat`, `test:parity`, `contract:dashboard`).
- **`search()`** keeps JavaScript `RegExp` semantics (Python's `re` is not
  ported); the text it runs over and its results are pdfplumber's.

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
PDFs, submodules, `.venv`, `.cache`). Details: [`oracle/README.md`](oracle/README.md)
and [`docs/verification.md`](docs/verification.md).
