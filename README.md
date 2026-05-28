# pdfplumber.js

Browser-capable JavaScript extraction API modeled on Python `pdfplumber`, with
Python `pdfplumber` and `pdfminer.six` used as the behavior oracles.

The implementation target is native pdfminer-style parsing and extraction.
PDF.js remains a named runtime dependency for PDF loading/operator capabilities
that are covered by tests.

## Installation

This package is still private while parity work is in progress. Build locally:

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

- Node: filesystem path, `URL`, `ArrayBuffer`, `Uint8Array`, `Blob`, and HTTP(S)
  URL string.
- Browser: `ArrayBuffer`, `Uint8Array`, `Blob`, `URL`, and URL string.

Supported options:

- `pages`: one-based page numbers to load.
- `password`: password for encrypted PDFs.
- `laparams`: pdfplumber-style layout object generation options.
- `unicode_norm`: one of `NFC`, `NFKC`, `NFD`, or `NFKD`.
- `raise_unicode_errors`: reserved for Python-compatible unicode error handling.

## Document API

`open()` resolves to a `PDFPlumberDocument`:

- `metadata`: Info metadata using pdfplumber-compatible keys.
- `pages`: loaded `PDFPlumberPage` objects.
- `objects`: document-level objects grouped by type.
- `annots`: all page annotations.
- `hyperlinks`: annotations with a URI.
- `close()`: releases PDF.js resources.

## Page API

Each page exposes pdfplumber-shaped geometry and objects:

- Page geometry: `page_number`, `pageNumber`, `width`, `height`, `bbox`,
  `mediabox`, `cropbox`, `artbox`, `bleedbox`, and `trimbox`.
- Object arrays: `chars`, `rects`, `lines`, `curves`, `images`, `annots`,
  `hyperlinks`, `rect_edges`, `curve_edges`, and `edges`.
- `objects`: object arrays grouped by `object_type`.

Extraction and transformation methods:

- `extractText(options)` / `extract_text(options)`
- `extractWords(options)` / `extract_words(options)`
- `search(pattern, options)`
- `crop(bbox, options)`
- `withinBbox(bbox, options)` / `within_bbox(bbox, options)`
- `outsideBbox(bbox, options)` / `outside_bbox(bbox, options)`
- `dedupeChars(options)` / `dedupe_chars(options)`
- `findTables(options)` / `find_tables(options)`
- `extractTable(options)` / `extract_table(options)`
- `extractTables(options)` / `extract_tables(options)`

Object dictionaries include pdfplumber-style fields where available, including
coordinates, colors, vector state, marked-content `mcid`/`tag`, annotation
fields, and image metadata such as `name`, `srcsize`, `colorspace`, and `bits`.

## Compatibility Gates

Useful local checks:

```sh
npm run typecheck
npm test
npm run test:browser
npm run contract:dashboard
```

The upstream contract dashboard is generated at
`docs/upstream-contract-dashboard/dashboard.tsv`.
