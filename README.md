# pdfplumber.js

[pdfplumber](https://github.com/jsvine/pdfplumber) for JavaScript, in Node and
in the browser: chars, words, text, vector objects, annotations and tables,
with the same results as Python pdfplumber.

The core is a faithful Rust port of pdfplumber and pdfminer.six, compiled to
WebAssembly behind a coarse-grained JavaScript API. Every stage of the pipeline
is checked against Python (pdfplumber 0.11.9, pdfminer.six 20260107,
Python ≥ 3.12) by comparing canonical traces on real and generated PDFs.

## Status

Rewrite in progress. The previous pdf.js-based port is tagged `legacy-pdfjs`.

- Why and how: [`docs/port-strategy.md`](docs/port-strategy.md)
- Layout and porting rules: [`docs/architecture.md`](docs/architecture.md)
- Verification method: [`docs/verification.md`](docs/verification.md)
- Public JavaScript API: [`docs/public-api.md`](docs/public-api.md)

## Development

```sh
scripts/setup-dev.sh   # submodules, Python oracle, Node, Rust + wasm32, external PDFs
```
