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
