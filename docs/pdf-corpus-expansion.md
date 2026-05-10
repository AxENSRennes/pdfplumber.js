# PDF Corpus Expansion

The current compatibility/parity fixtures already cover the upstream
`pdfplumber-python/tests/pdfs` corpus. The next useful expansion is a small,
curated external corpus that stresses document classes not well represented by
upstream regression PDFs.

## Selection Criteria

- Prefer public, stable URLs with clear reuse terms.
- Keep checked-in PDFs small and limited; use a manifest plus downloader for
  anything large.
- Cover behavior that matters to pdfplumber parity: character geometry,
  reading order, tables, annotations, forms, page boxes, tagged structure,
  images, malformed files, encryption, and edge/path extraction.
- Store provenance with each fixture: source URL, license/terms note, SHA-256,
  file size, category, selected pages, and intended checks.

## Recommended Sources

| Source | Why it helps | Suggested use |
| --- | --- | --- |
| PDF Association pdf-corpora index: https://github.com/pdf-association/pdf-corpora | Curated index of PDF-centric corpora, including PDF.js, pdfium, iText, PDF/UA, PDF/X, 3D, and stress corpora. | Use as the primary discovery index for low-level PDF feature fixtures. |
| PDF/UA Reference Suite: https://pdfa.org/resource/pdfua-reference-suite/ | Ten tagged, accessible PDFs covering multi-page content, complex tables, interactive forms, links to structure elements, and scanned documents. | Add targeted parity tests for structure/MCID/tagged content, links, forms, and table extraction. |
| veraPDF corpus: https://github.com/veraPDF/veraPDF-corpus | Atomic PDF/A, PDF/UA, ISO 32000-1, and ISO 32000-2 conformance files under CC BY 4.0. | Add a small subset for metadata, embedded files, color spaces, structure trees, page boxes, and parser robustness. |
| Mozilla PDF.js regression PDFs: https://github.com/mozilla/pdf.js/tree/master/test/pdfs | Real and synthetic files used by the same rendering/parsing engine that `pdfplumber.js` depends on. | Add smoke/parity cases for features PDF.js parses differently from pdfminer, especially fonts, CMaps, annotations, and malformed files. |
| PubLayNet / PMC OA PDFs: https://github.com/ibm-aur-nlp/PubLayNet | Scientific articles from PubMed Central OA with layout annotations from PDF/XML matching. | Select a few born-digital article pages with two-column text, figures, captions, and tables. |
| PubTables-1M: https://huggingface.co/datasets/bsmock/pubtables-1m/tree/main | Large table-focused corpus from PubMed Central OA; includes PDF annotations and page/table word data. | Use only tiny pinned samples for table detection/extraction parity; do not vendor the full dataset. |
| TableBank: https://github.com/doc-analysis/TableBank | 417K labeled tables from Word and LaTeX documents, with official train/val/test splits. | Use representative generated PDFs/pages for bordered, borderless, and LaTeX-style tables if original PDFs are accessible under acceptable terms. |
| DocLayNet v1.2: https://huggingface.co/datasets/docling-project/DocLayNet-v1.2 | Diverse human-annotated layout dataset with embedded PDFs in the dataset rows. | Sample legal, financial, scientific, manual, and magazine-like pages for reading order and object geometry. |
| IRS forms/publications: https://www.irs.gov/forms-instructions-and-publications | Stable public PDFs with AcroForm-style fields, dense instructions, tables, multilingual variants, and accessibility variants. | Add fillable form and instruction PDF cases for annotations/forms/text extraction. |
| Federal Register dataset: https://www.archives.gov/open/dataset-fedreg.html | Official daily legal newspaper issues, roughly 250 issues per yearly dataset. | Select a few pages for dense multi-column legal text, tables, headers/footers, and long documents. |
| CourtListener bulk legal data: https://www.courtlistener.com/help/api/bulk-data/ | Legal opinions and metadata; bulk exports include paths to Harvard Caselaw PDF files. | Sample court opinions for legal layout, citations, page headers, footers, and scanned/older PDFs. |

## First Seed Set

Start with 20-30 files rather than a huge corpus:

- 5 tagged/accessibility PDFs from the PDF/UA Reference Suite.
- 5 atomic conformance PDFs from veraPDF, chosen for page boxes, structure,
  metadata, embedded files, and color/path behavior.
- 5 PDF.js regression PDFs that exercise fonts, CMaps, annotations, and
  malformed-but-loadable files.
- 3 scientific article/table PDFs from PubLayNet or PubTables-1M.
- 3 IRS PDFs: one fillable form, one long instruction document, one
  multilingual/publication variant.
- 2 Federal Register pages/issues with dense multi-column text and tables.
- 2 legal opinion PDFs from CourtListener/Harvard Caselaw.

## Integration Shape

The external corpus lives in `test/fixtures/external-pdfs/` and is described
by `test/fixtures/external-pdfs/manifest.json`. It currently contains 53 PDFs,
about 124 MB total, from PDF.js, veraPDF, IRS, DOL, NASA, USPTO, UN ODS, arXiv,
SCOTUS, GovInfo Federal Register, Wikimedia Commons / Internet Archive,
company investor-relations sites, U.S. financial regulators, Treasury/Fiscal
Data, and the World Bank Open Knowledge Repository.

The external parity goldens intentionally compare object-level page snapshots.
For short PDFs, every page is snapshotted. For long PDFs, the generator selects
seven representative pages: the first two pages, three pages around the middle,
and the last two pages. Each selected page records geometry, object counts,
object samples for `char`, `line`, `rect`, `curve`, `image`, annotations,
edge counts, extracted text summaries, and extracted word summaries.

The second expansion adds coverage for CJK fonts, Arabic/RTL shaping, missing
CMaps, file attachments, button widgets, annotations without appearances, bad
page labels, bitmap/CCITT images, public medical/government forms, tagged NASA
reports, USPTO patent drawings, multilingual UN documents, and a public-domain
scanned book with variable page boxes.

The financial expansion adds long annual reports and filings from Berkshire
Hathaway, JPMorgan Chase, Apple, Walmart, Target, Morgan Stanley, Citigroup, the
Federal Reserve, Treasury Financial Report of the U.S. Government, FSOC, FDIC,
OCC, and the World Bank. These fixtures stress dense accounting tables,
consolidated financial statements, notes, SEC cover pages, mixed portrait/table
layouts, image-heavy government report pages, and long-document page sampling.

## Holdout Corpus

The anti-overfit holdout lives in
`test/fixtures/external-holdout-pdfs/manifest.json`. It contains 20 separate
PDFs, about 65 MB total, from public companies, government forms, USPTO, FDA,
CMS, arXiv, Federal Register/GovInfo, the European Banking Authority, the Bank
of Japan, PDF.js, and SCOTUS.

The holdout is intentionally separate from the stabilization corpus. Generate
its Python reference once with:

```bash
npm run fixtures:generate:external-holdout-parity
```

Run it only at validation checkpoints:

```bash
npm run test:external-holdout-parity
```

Do not move holdout PDFs into the current corpus, tune fixes by file name,
regenerate holdout goldens from JavaScript, or add allowed failures. If a final
holdout run fails, classify failures by technical family and add any missing
family coverage to focused stabilization tests rather than repeatedly targeting
the holdout document itself.

The manifest shape is:

```json
[
  {
    "id": "pdfua-reference-complex-table",
    "sourceUrl": "https://example.invalid/file.pdf",
    "license": "Source-specific terms",
    "sha256": "...",
    "category": ["tagged-pdf", "table", "form"],
    "localPath": "test/fixtures/external-pdfs/pdfua-reference-complex-table.pdf",
    "checks": ["document.snapshot", "page.textSummary", "page.findTablesSummary"]
  }
]
```

Then teach the golden generator to read both the vendored upstream directory
and the external manifest. This keeps current tests deterministic while making
licensing, provenance, and updates explicit.
