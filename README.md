# pdfplumber.js

Browser-capable JavaScript extraction API modeled on Python `pdfplumber`, with
Python `pdfplumber` and `pdfminer.six` used as the behavior oracles.

The implementation target is native pdfminer-style parsing and extraction.
PDF.js remains a named runtime dependency for PDF loading/operator capabilities
that are covered by tests.

The supported public surface is documented in
[`docs/public-api.md`](docs/public-api.md).

## Installation

Install from npm once published, or build locally while parity work continues:

```sh
npm install
npm run build
```

The Node ESM entrypoint is `dist/src/index.js`. The browser ESM bundle is
`dist/browser/index.js` and loads its worker from `dist/browser/pdf.worker.js`.

## Opening PDFs

```ts
import { open } from "pdfplumber-js";

const pdf = await open(arrayBufferOrPathOrUrl);
try {
  const page = pdf.pages[0];
  console.log(page.extractText());
} finally {
  await pdf.close();
}
```

Supported inputs:

- Node: filesystem path, `file:` URL, HTTP(S) `URL`, `ArrayBuffer`,
  `Uint8Array`, `Blob`, and HTTP(S) URL string.
- Browser: `ArrayBuffer`, `Uint8Array`, `Blob`, `URL`, and URL string.

Supported options:

- `pages`: one-based page numbers to load.
- `password`: password for encrypted PDFs.
- `laparams`: pdfplumber-style layout object generation options.
- `unicode_norm`: one of `NFC`, `NFKC`, `NFD`, or `NFKD`.
- `raise_unicode_errors`: reserved for Python-compatible unicode error handling.

The ESM package exports TypeScript types for public inputs, documents, pages,
objects, search results, text/word/search/crop/dedupe options, and table
settings.

## Document API

`open()` resolves to a `PDFPlumberDocument`:

- `metadata`: Info metadata using pdfplumber-compatible keys.
- `pages`: loaded `PDFPlumberPage` objects.
- `objects`: document-level objects grouped by type.
- `annots`: all page annotations.
- `hyperlinks`: annotations with a URI.
- `edges`, `horizontal_edges`, `vertical_edges`: document-level vector edges.
- `close()`: releases runtime resources.

## Page API

Each page exposes pdfplumber-shaped geometry and objects:

- Page geometry and boxes: `page_number`, `pageNumber`, `label`, `width`,
  `height`, `bbox`, `mediabox`, `cropbox`, `artbox`, `bleedbox`, and
  `trimbox`.
- Object arrays: `chars`, `rects`, `lines`, `curves`, `images`, `annots`,
  `hyperlinks`, `rect_edges`, `curve_edges`, and `edges`.
- Vector objects include pdfplumber-style `path` command arrays and `pts`.
- Edge helpers: `horizontal_edges` and `vertical_edges`.
- `objects`: object arrays grouped by `object_type`.

Extraction and transformation methods:

- `extractText(options)` / `extract_text(options)`
- `extractWords(options)` / `extract_words(options)`
- `extractTextLines(options)` / `extract_text_lines(options)`
- `search(pattern, options)`
- `filter(testFunction)`
- `crop(bbox, options)`
- `withinBbox(bbox, options)` / `within_bbox(bbox, options)`
- `outsideBbox(bbox, options)` / `outside_bbox(bbox, options)`
- `dedupeChars(options)` / `dedupe_chars(options)`
- `findTable(options)` / `find_table(options)`
- `findTables(options)` / `find_tables(options)`
- `extractTable(options)` / `extract_table(options)`
- `extractTables(options)` / `extract_tables(options)`

Object dictionaries include pdfplumber-style fields where available, including
coordinates, colors, vector state, marked-content `mcid`/`tag`, annotation
fields including parsed annotation `data`, and image metadata such as `name`,
`srcsize`, `imagemask`, `colorspace`, and `bits`.

Table detection accepts pdfplumber-style table settings, including `lines`,
`lines_strict`, `text`, and `explicit` `vertical_strategy` /
`horizontal_strategy` values, explicit edge inputs, snap/join/intersection
tolerances, minimum word thresholds, and `text_*` options forwarded to cell text
extraction.
`Table.rows` and `Table.columns` contain `TableAxisGroup` objects with `bbox`
and `cells`.

## Compatibility Gates

Useful local checks:

```sh
npm run typecheck
npm test
npm run test:compat
npm run test:parity
npm run test:stability:smoke
npm run test:stability:real-cycles
npm run test:stability
npm run test:browser
npm run package:check
npm run audit:mupdf:check
npm run contract:dashboard
npm run contract:dashboard:check
```

The upstream contract dashboard is generated at
`docs/upstream-contract-dashboard/dashboard.tsv`.
