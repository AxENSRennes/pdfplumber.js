# Public Extraction API

`pdfplumber.js` exposes a browser-capable JavaScript API modeled on Python
`pdfplumber`. Callers use the same `open()` entrypoint in Node and browsers and
do not choose or configure the internal PDF engine.

## Entrypoint

```ts
import { open } from "pdfplumber-js";

const pdf = await open(input, options);
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

Supported `open()` options:

- `pages`: one-based page numbers to load.
- `password`: password for encrypted PDFs.
- `laparams`: pdfplumber-style layout object generation options.
- `unicode_norm`: one of `NFC`, `NFKC`, `NFD`, or `NFKD`.
- `raise_unicode_errors`: reserved for Python-compatible unicode error
  handling.

Backend names such as PDF.js, MuPDF, native, or engine are intentionally not
public options. The stable contract is extraction behavior, not implementation
selection.

The package also exports TypeScript types for the documented surface:
`PDFInput`, `OpenOptions`, `PDFPlumberDocument`, `PDFPlumberPage`, `PDFObject`,
`SearchResult`, `ExtractTextOptions`, `WordOptions`, `SearchOptions`,
`TextLineOptions`, `CropOptions`, `DedupeOptions`, `TableOptions`,
`TableStrategy`, `ExplicitTableLine`, `Table`, and `TableAxisGroup`.

## Document

`open()` resolves to a `PDFPlumberDocument` with:

- `metadata`: Info dictionary metadata using pdfplumber-compatible keys.
- `pages`: loaded `PDFPlumberPage` objects.
- `objects`: document-level objects grouped by `object_type`.
- `annots`: all page annotations.
- `hyperlinks`: annotations with a URI.
- `edges`, `horizontal_edges`, `vertical_edges`: document-level vector edges.
- `close()`: releases runtime resources.

## Page Geometry

Each page exposes pdfplumber-style geometry:

- `page_number` and `pageNumber`.
- `label`.
- `width`, `height`, and `bbox`.
- `mediabox`, `cropbox`, `artbox`, `bleedbox`, and `trimbox` when present.

All object coordinates use pdfplumber's top-left page coordinate convention:
`x0`, `x1`, `y0`, `y1`, `top`, `bottom`, `doctop`, `width`, and `height`.

## Page Objects

Object arrays are available both directly and through `page.objects`:

- Text: `chars`, `textboxhorizontals`, `textlinehorizontals`,
  `textboxverticals`, `textlineverticals`.
- Vectors: `rects`, `lines`, `curves`, `rect_edges`, `curve_edges`, `edges`,
  `horizontal_edges`, and `vertical_edges`.
- Vector objects include pdfplumber-style `path` command arrays and `pts`.
- Images: `images`.
- Annotations: `annots` and `hyperlinks`.
- Layout figures: `figures`.

Object dictionaries include pdfplumber-style fields where available:

- `object_type` and `page_number`.
- Geometry fields listed above.
- Text fields such as `text`, `fontname`, `size`, text matrix, and color
  fields.
- Vector fields such as `linewidth`, dash state, path points, stroking color,
  non-stroking color, and pattern names.
- Marked-content fields `mcid` and `tag`.
- Annotation fields such as geometry, `uri`, `contents`, `title`, `name`, and
  parsed annotation `data` when available.
- Image metadata such as `name`, `srcsize`, `imagemask`, `colorspace`, and
  `bits`.

## Text, Words, And Search

Pages expose camelCase and snake_case aliases:

- `extractText(options)` / `extract_text(options)`.
- `extractWords(options)` / `extract_words(options)`.
- `extractTextLines(options)` / `extract_text_lines(options)`.
- `search(pattern, options)`.

Text and word options follow the pdfplumber shape where implemented, including
layout options, tolerances, text direction, punctuation splitting, flow
ordering, `return_chars`, `extra_attrs`, `limit`, and `slice`.

`search()` accepts a string or `RegExp` and returns matches with text, geometry,
optional groups, and optional chars.

## Cropping, Filtering, And Dedupe

Pages can derive filtered page views without exposing engine internals:

- `filter(testFunction)`.
- `crop(bbox, options)`.
- `withinBbox(bbox, options)` / `within_bbox(bbox, options)`.
- `outsideBbox(bbox, options)` / `outside_bbox(bbox, options)`.
- `dedupeChars(options)` / `dedupe_chars(options)`.

Crop options support `relative` and `strict`. Dedupe options include
`tolerance` and `extra_attrs`.

## Tables

Table APIs expose pdfplumber-style table inputs and outputs:

- `findTable(options)` / `find_table(options)`.
- `findTables(options)` / `find_tables(options)`.
- `extractTable(options)` / `extract_table(options)`.
- `extractTables(options)` / `extract_tables(options)`.

Supported table settings include:

- `vertical_strategy` and `horizontal_strategy`: `lines`, `lines_strict`,
  `text`, and `explicit`.
- `explicit_vertical_lines` and `explicit_horizontal_lines`.
- Snap, join, intersection, edge-min-length, and text tolerances.
- Minimum word thresholds.
- `text_*` options forwarded to cell text extraction.

`findTable()` and `findTables()` return `Table` objects. `Table.rows` and
`Table.columns` contain `TableAxisGroup` objects with `bbox` and `cells`.
`extractTable()` and `extractTables()` return arrays of cell strings or `null`
values.

## Runtime Contract

The browser package is ESM and is built to `dist/browser/index.js` with a
packaged worker at `dist/browser/pdf.worker.js`. Browser compatibility is tested
in Chromium, Firefox, and WebKit using `ArrayBuffer`, `Uint8Array`, `Blob`, and
`URL` inputs against the same public extraction behavior used in Node.

PDF.js may appear internally only for named, tested runtime/operator
capabilities: opening PDF bytes, worker and asset wiring, page metadata,
text-content and operator-list retrieval, annotation retrieval, and operator-code
constants used by the adapter. PDF.js is not a public engine selector or oracle.
The public API stays extraction-shaped and backend-independent.

## Stable Open Errors

`open()` either returns a `PDFPlumberDocument` or rejects with a named `Error`.
Robustness-corpus inputs are expected to extract stable public page data or raise
one of these documented error names:

- `InvalidPDFException`: empty, invalid, or structurally unreadable PDFs.
- `PasswordException`: encrypted PDFs opened without a password or with an
  incorrect password.
- `FormatError`: PDF format errors surfaced by the runtime parser.
- `UnknownErrorException`: runtime parser failures without a more specific
  public error name.
- `Error`: non-PDF input adaptation failures such as failed HTTP fetches.

Callers should branch on `error.name` and treat the message as diagnostic text.
