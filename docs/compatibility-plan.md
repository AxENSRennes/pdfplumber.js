# pdfplumber.js Compatibility Plan

This project should treat Python `pdfplumber` as the behavioral oracle until the TypeScript port is mature. The first implementation target is not "parse PDFs somehow"; it is "match `pdfplumber` object geometry, text grouping, table extraction, and edge cases on the upstream test corpus."

## Test Strategy

The compatibility harness has two layers:

1. `scripts/generate-goldens.py` runs the vendored Python reference implementation against upstream PDFs and writes `test/fixtures/goldens/pdfplumber-compat.json`.
2. `test/compat/pdfplumber.compat.test.ts` opens those same PDFs through the TypeScript API and compares outputs with numeric tolerance.

Regular `npm test` only checks the API scaffold. Run the parity suite explicitly:

```bash
npm run python:setup
wsl_venv/bin/python scripts/generate-goldens.py
npm run test:compat
```

`test:compat` is expected to fail until the TypeScript implementation is built.

## Reference Coverage

The first golden set covers:

- PDF opening, metadata, password-protected PDFs, page count, page boxes, rotation, and non-zero MediaBox origins.
- Low-level objects: chars, rects, lines, curves, images, annotations, hyperlinks, rect edges, curve edges, and merged edge lists.
- Text extraction: default layout, custom tolerances, `x_tolerance_ratio`, ligature expansion, unicode normalization, punctuation splitting, search results, and cropped text.
- Word extraction: direction handling, extra attributes, and representative word object geometry.
- Table extraction: lines, `lines_strict`, text strategy, text tolerances, text layout, curves, cropped tables, and MediaBox coordinate regressions.
- Regression fixtures: duplicate chars, MCIDs, issue PDFs from upstream, and selected annotation behavior.

Not covered in the first browser target:

- `display.py` image rendering helpers.
- CLI conversion tests.
- Ghostscript repair flow.
- Pandas-dependent conversion details.
- Full tagged-PDF structure tree support beyond MCID-bearing low-level objects.

Those can be added later, but they should not block the core browser library.

## Implementation Implications

`pdfplumber` is built on `pdfminer.six`, and much of its behavior comes from pdfminer layout objects. A browser port needs an equivalent extraction substrate before porting higher-level algorithms.

The implementation target is native pdfminer-style parsing and extraction.
Mozilla PDF.js (`pdfjs-dist`) remains a named runtime/operator dependency for
browser-capable opening, worker/asset wiring, page metadata, text content,
operator lists, annotations, and operator-code constants while native
subsystems continue to mature. It is not a public engine selector or oracle, and
it does not expose pdfminer-shaped layout objects directly; any PDF.js-derived
data must still be adapted into `char`, `line`, `rect`, `curve`, `image`, and
annotation dictionaries using pdfplumber's coordinate conventions.

MuPDF.js is technically attractive because its JavaScript/WASM API exposes per-character structured text and path/text/image device callbacks. However, its npm package is AGPL-3.0-or-later unless a commercial license is used. See `docs/pdf-engine-comparison.md`.

Important compatibility risks:

- Coordinate systems: PDF space is bottom-left; pdfplumber exposes top-left `top`/`bottom` and `doctop`. Rotation and non-zero MediaBox handling must match Python exactly.
- Character geometry: pdfplumber relies on per-character boxes, font names, sizes, advance widths, text matrices, upright flags, and colors. PDF.js text items are often grouped runs, so splitting into char dictionaries must preserve widths and transforms carefully.
- Graphics extraction: table finding depends on accurate lines, rectangle edges, and curve-derived edges. Native content-stream parsing is the target, with PDF.js operator-list parsing retained only for named, tested transitional roles.
- Text ordering: `use_text_flow`, rotated text directions, ligature expansion, punctuation splitting, extra attributes, and layout rendering are mostly pdfplumber algorithms and can be transcribed once char objects are faithful.
- Tables: `table.py` is pure geometry and should port well, but only after edge extraction is credible.
- Metadata and annotations: decoding behavior, URI fields, rotated annotation boxes, and unicode error policy need explicit parity tests.

## Recommended Build Order

1. Keep the public document/page loader stable across Node and browsers with page boxes, rotation, metadata, password handling, and close lifecycle.
2. Emit pdfplumber-compatible `char` objects from native/pdfminer-style text semantics, using PDF.js text content only as a named tested transitional input.
3. Port `utils.geometry`, `utils.clustering`, and `utils.text`; make text/word tests pass.
4. Parse drawing operators into `line`, `rect`, and `curve` objects through native content-stream semantics; make edge tests pass.
5. Port `table.py`; make table tests pass.
6. Add annotations and MCID/tag support with Python-oracle parity and explicit PDF.js capability coverage where the runtime supplies inputs.
7. Expand goldens toward more upstream tests once the core object model stabilizes.
