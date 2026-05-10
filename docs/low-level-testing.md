# Low-Level PDF Testing

The low-level suite protects the PDF parsing primitives that sit below the public pdfplumber-shaped API. The parity suites remain the final user-visible oracle, but these tests should fail closer to the source of a regression.

## Layers

1. Unit tests over synthetic snippets in `test/lowlevel`.
   These cover byte streams, object extraction, content-stream tokenization, resources, fonts, metadata, and page-box normalization.

2. Micro-PDF fixtures.
   Add tiny generated PDFs when behavior depends on a real PDF engine interaction rather than a pure parser function. Keep each fixture focused on one feature.
   Generate them with `npm run fixtures:generate:micro`; sources live in `scripts/generate-micro-pdfs.py`.

3. Python pdfplumber parity.
   Use `wsl_venv` and the parity goldens to validate the public behavior expected from pdfminer/pdfplumber.
   The generated micro-PDFs are checked directly against Python pdfplumber in `test/lowlevel/micro-pdfs.test.ts`.

4. Diagnostic engine comparisons.
   Use `npm run audit:mupdf` to compare the current backend with MuPDF on real fixtures. MuPDF is a diagnostic oracle for robustness, not automatically the pdfplumber parity oracle.
   Use `npm run audit:mupdf:micro` for the generated micro-PDFs.

## Expansion Checklist

- Add one low-level regression for every bug fixed in `src/pdf.ts`, `src/font-decoding.ts`, `src/pdf-strings.ts`, or `src/pdfminer-compat.ts`.
- Prefer a small content stream or object snippet before adding a large PDF fixture.
- If a bug only reproduces through PDF.js operator lists, add a micro-PDF and a focused high-level assertion.
- Keep external corpus and holdout tests for breadth; keep low-level tests for localization.
- Keep fuzz tests deterministic: fixed seeds, small generated streams, and assertions against the exact operators that should survive tokenization.

## Current Micro-Fixture Coverage

`scripts/generate-micro-pdfs.py` currently generates focused fixtures for text operators, Type3 fonts, Type0/CIDFont with ToUnicode and Identity-H/V, ligature/missing glyph behavior, vertical and RTL text, inherited page resources, indirect `/Contents` arrays, nested Form XObjects with local resources and matrices, vector paths, clipping/dash/linewidth/fill/stroke/even-odd state, DeviceGray/RGB/CMYK/Pattern colors, image XObjects, masks, inline images, indirect colorspaces, link/highlight/widget and extended annotation families, page boxes/rotation, encrypted/password documents, and a recoverable broken-xref PDF.

Stream primitives also cover Flate, ASCII85, ASCIIHex, RunLength, LZW, chained filters, CR/LF variants, empty streams, and malformed streams.
