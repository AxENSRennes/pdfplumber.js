# PDF Engine Comparison: PDF.js vs MuPDF.js

This compares the two realistic browser-capable substrates for pdfplumber.js.

## Summary

MuPDF.js is technically the stronger fit for reproducing pdfplumber's object model. It exposes per-character structured text and page execution callbacks for text, paths, images, colors, stroke state, and transforms. That maps naturally to pdfplumber's `char`, `line`, `rect`, `curve`, `image`, and edge extraction.

PDF.js is the safer open-source dependency. It is Apache-2.0, widely deployed, and browser-first. It exposes page metadata, text content, annotations, structure trees, and operator lists. The downside is that the pdfplumber adapter would need to reconstruct more low-level drawing and glyph state manually from PDF.js operator lists.

Recommended direction: keep the internal engine interface abstract, but use PDF.js as the default dependency unless AGPL/commercial licensing for MuPDF.js is acceptable. If exact table/geometry parity becomes more important than permissive licensing, MuPDF.js is the better engine.

## Licensing

- PDF.js / `pdfjs-dist`: Apache-2.0.
- MuPDF.js / `mupdf`: AGPL-3.0-or-later, with commercial licensing available from Artifex.

The AGPL point is decisive for an npm library intended for broad browser use. Depending on `mupdf` directly would make the distribution/licensing story much harder for downstream applications.

## API Shape

PDF.js provides:

- `getDocument()`
- `PDFDocumentProxy.numPages`
- `PDFPageProxy.view`, `rotate`, `userUnit`
- `PDFPageProxy.getTextContent()`
- `PDFPageProxy.getOperatorList()`
- `PDFPageProxy.getAnnotations()`
- `PDFPageProxy.getStructTree()`

MuPDF.js provides:

- `Document.openDocument()`
- `Document.countPages()`, `loadPage()`, metadata, password APIs
- `Page.getBounds(box)`
- `Page.toStructuredText("preserve-spans")`
- `StructuredText.walk({ onChar, beginLine, ... })`
- `Page.run(new Device({ fillPath, strokePath, fillText, fillImage, ... }))`
- PDF-specific `PDFPage.getAnnotations()`

## Local Spike Results

Tested against representative upstream pdfplumber PDFs.

### `nics-background-checks-2015-11.pdf`

PDF.js:

- 1 page, correct `view`: `[0, 0, 1008, 612]`
- 2,603 text items, approximately 5,584 text characters
- 8,316 operators
- 703 `constructPath`, 1,098 `showText`, 254-ish filled paths visible through operator analysis

MuPDF.js:

- 1 page, correct `MediaBox`: `[0, 0, 1008, 612]`
- 4,327 structured text chars
- First char includes real font name, size, quad, origin, RGB color
- `Page.run(Device)` produced 254 `fillPath`, 192 `strokePath`, 1,098 `fillText`
- First filled path included rectangle-like path ops and fill color `[0.8, 1, 1]`, matching the kind of object pdfplumber tests inspect

### `pdffill-demo.pdf`

PDF.js:

- 7 pages
- 26 text items, approximately 500 text characters on page 1
- 13 display annotations
- Operator list includes paths, images, colors, text

MuPDF.js:

- 7 pages
- 497 structured text chars on page 1
- 11 page links from `getLinks()`
- `PDFPage.getAnnotations()` is also available
- Device callbacks produced 8 filled paths, 9 stroked paths, 5 images, 17 text draws

### `table-curves-example.pdf`

PDF.js:

- 218 text items, approximately 1,929 chars
- 247 `constructPath` operators
- Curves/path details require decoding operator args and current graphics state

MuPDF.js:

- 1,992 structured text chars
- 241 filled paths through `fillPath`
- Path walker exposes `moveTo`, `lineTo`, `curveTo`, and `closePath`
- This is much closer to pdfplumber's `curve_to_edges` / table extraction needs

### `issue-1181.pdf`

This is important because pdfplumber has specific regressions around non-zero MediaBox/page coordinate behavior.

PDF.js:

- Page 1 `view`: `[0, 200, 420.9449, 585.2756]`
- This preserves the original offset information that pdfplumber needs to handle carefully

MuPDF.js:

- `getBounds("MediaBox")`: `[0, 0, 420.9449, 385.2756]`
- Page run emitted CTM offsets such as `[1, 0, 0, 1, 0, -10]`
- MuPDF repaired the PDF automatically and warned about xref repair

This means MuPDF may normalize some box information differently from pdfminer/PDF.js, so parity still needs careful coordinate tests.

## Fit For pdfplumber.js

### PDF.js Strengths

- Permissive Apache-2.0 license.
- Best fit for a browser-first open-source npm package.
- Text content, annotations, operator lists, structure tree APIs are available.
- Already broadly used and maintained.
- Smaller legal risk for downstream users.

### PDF.js Weaknesses

- Text items are runs, not pdfplumber-style chars.
- Font names can be internal IDs unless resolved through common/font objects.
- Vector graphics require interpreting operator lists and graphics state manually.
- Color, CTM, path, stroke, clipping, and fill behavior need more adapter code.
- Some Node usage needs extra setup for standard fonts/worker configuration.

### MuPDF.js Strengths

- Best technical match for pdfplumber internals.
- `StructuredText.walk` gives per-char callbacks with `origin`, `font`, `size`, `quad`, and color.
- `Device` callbacks expose `fillPath`, `strokePath`, `fillText`, images, stroke state, colors, alpha, CTM.
- Path objects can be walked into move/line/curve operations.
- Robust parser; auto-repaired a malformed xref in the spike.
- Likely faster for large documents because most heavy parsing is native/WASM.

### MuPDF.js Weaknesses

- AGPL-3.0-or-later unless a commercial license is used.
- npm package unpacked size is about 13.8 MB before app bundling decisions.
- WASM integration adds loading and deployment complexity.
- Some page-box/coordinate normalization differs from pdfminer/PDF.js and needs explicit handling.
- Direct dependency would make pdfplumber.js much less frictionless for many apps.

## Decision

For an open-source browser library, start with PDF.js and an internal engine abstraction:

```ts
interface PdfEngine {
  open(input: PDFInput, options: OpenOptions): Promise<EngineDocument>;
}

interface EnginePage {
  getBoxes(): PageBoxes;
  getTextRuns(): Promise<EngineTextRun[]>;
  getOperators(): Promise<EngineOperatorList>;
  getAnnotations(): Promise<EngineAnnotation[]>;
}
```

Then build the pdfplumber object adapter on top of that.

Keep MuPDF.js as an optional/prototype engine. If PDF.js path reconstruction proves too slow or inaccurate for tables, revisit MuPDF.js with an explicit licensing decision.

