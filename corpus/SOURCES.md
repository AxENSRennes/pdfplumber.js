# Corpus

The PDFs the port is checked on, besides the test suites of the upstream
submodules (`upstream/pdfplumber-python/tests/pdfs`,
`upstream/pdfminer-six/samples`, `upstream/pdfjs/test/pdfs`) and the
adversarial corpora generated under the cache (`oracle/adversarial/`). How
the oracle groups them into sets (`micro`, `quick`, `all`...) is in
[`docs/verification.md`](../docs/verification.md#corpus-sets).

## Directories

| Directory | Content | In git |
|---|---|---|
| `micro/` | 17 small PDFs, one feature each, written by `oracle/generate_micro_pdfs.py` (deterministic, with `manifest.json`) | yes |
| `upstream-contract/` | 66 regression PDFs of pdf.js's test suite that its repository only links to, with source URL, license and SHA-256 in `manifest.json` | yes |
| `parity-cycles/cycle-01` to `cycle-06` | small generated PDFs, one feature each, in `working/` and `holdout/` batches with their manifests | yes |
| `parity-cycles/cycle-10` to `cycle-14` | page selections of the external corpus (manifests only: their `localPath` points into `external/`) | manifests |
| `external/` | 53 public PDFs, about 129 MB: tagged and accessible PDFs, conformance files, forms, government and legal documents, scientific articles, financial reports, CJK and right-to-left text, scanned books | `manifest.json` only |
| `external-holdout/` | 20 public PDFs, about 67 MB, kept apart from `external/` (companies, government forms, patents, regulators, arXiv) | `manifest.json` only |
| `passwords.json` | passwords of upstream test files, each citing the upstream test that opens it | yes |

## Downloading

```sh
node scripts/download-corpus.mjs        # or npm run corpus:download; scripts/setup-dev.sh runs it
```

downloads every entry of `corpus/external/manifest.json` and
`corpus/external-holdout/manifest.json` to its `localPath` and checks its
SHA-256 (`--manifest PATH` and `--id ID` restrict it, `--force` downloads
again, `--dry-run` only prints). A file whose source no longer answers can
be copied from a machine that has it: the SHA-256 identifies it. In a git
worktree, the downloaded files live in the main checkout
(`PDFPLUMBER_ROOT`).

## Manifests

Each entry records where a file comes from and why it is there:

```json
{
  "id": "pdfua-reference-complex-table",
  "sourceUrl": "https://example.invalid/file.pdf",
  "license": "Source-specific terms",
  "sha256": "...",
  "size": 123456,
  "categories": ["tagged-pdf", "table", "form"],
  "localPath": "corpus/external/pdfua-reference-complex-table.pdf",
  "selectedPages": [1, 2]
}
```

A `password` (or `openOptions.password`) field gives the password of an
encrypted file; `oracle/corpus.py` reads it, keyed by the file's SHA-256.

## Selection Criteria

- Public, stable URLs with clear reuse terms; large files are downloaded
  from their manifest, never committed.
- Behavior that matters to pdfplumber: character geometry, reading order,
  tables, annotations, forms, page boxes, tagged structure, images,
  malformed files, encryption, edges and paths.
- Provenance with each file: source URL, license or terms, SHA-256, size,
  categories and the pages of interest.
- A difference found on a file is fixed at its cause, never by its name: a
  failure is classified by its technical family, and a focused case that
  shows the family is added to `micro/` or to an adversarial corpus. The
  holdout files are not used to tune anything.

## Sources

| Source | What it brings |
| --- | --- |
| PDF Association pdf-corpora index: https://github.com/pdf-association/pdf-corpora | an index of PDF corpora (pdf.js, pdfium, iText, PDF/UA, PDF/X, stress corpora) |
| PDF/UA Reference Suite: https://pdfa.org/resource/pdfua-reference-suite/ | tagged, accessible PDFs: structure trees, MCIDs, links to structure elements, forms, complex tables |
| veraPDF corpus: https://github.com/veraPDF/veraPDF-corpus | atomic PDF/A, PDF/UA and ISO 32000 conformance files (CC BY 4.0): metadata, embedded files, color spaces, page boxes |
| Mozilla pdf.js regression PDFs: https://github.com/mozilla/pdf.js/tree/master/test/pdfs | real and synthetic files that stress fonts, CMaps, annotations and malformed structures |
| PubLayNet / PMC OA PDFs: https://github.com/ibm-aur-nlp/PubLayNet | born-digital scientific articles: two-column text, figures, captions, tables |
| PubTables-1M: https://huggingface.co/datasets/bsmock/pubtables-1m/tree/main | table pages from PubMed Central (small pinned samples only) |
| TableBank: https://github.com/doc-analysis/TableBank | bordered, borderless and LaTeX-style tables |
| DocLayNet v1.2: https://huggingface.co/datasets/docling-project/DocLayNet-v1.2 | legal, financial, scientific, manual and magazine-like pages |
| IRS forms and publications: https://www.irs.gov/forms-instructions-and-publications | fillable forms, dense instructions, multilingual and accessible variants |
| Federal Register: https://www.archives.gov/open/dataset-fedreg.html | dense multi-column legal text, tables, long documents |
| CourtListener bulk legal data: https://www.courtlistener.com/help/api/bulk-data/ | court opinions: legal layout, headers and footers, scanned and older PDFs |

The external corpus also holds long annual reports and filings (accounting
tables, notes, cover pages, mixed portrait and table layouts), public
medical and government forms, patent drawings, multilingual UN documents,
CJK fonts with missing CMaps, Arabic and right-to-left text, file
attachments, button widgets, annotations without appearances, bad page
labels, bitmap and CCITT images, and a scanned public-domain book with
variable page boxes.
