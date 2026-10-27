# Contributing

pdfplumber.js is a port: its specification is Python pdfplumber 0.11.9 on
pdfminer.six 20260107, run by CPython 3.12. A change is correct when the
port behaves like Python, which the repository checks by comparing
canonical traces of every pipeline stage on real and generated PDFs. Read
[`docs/architecture.md`](docs/architecture.md) (layout, porting rules,
value model, Python semantics) and
[`docs/verification.md`](docs/verification.md) (traces, corpora, mutation
testing, definition of done) before changing the engine.

## Prerequisites

- Linux or macOS, with `git`, [`uv`](https://docs.astral.sh/uv/) and
  `rustup`.
- Rust from `rust-toolchain.toml` (1.99.0, components `clippy` and
  `rustfmt`, targets `wasm32-unknown-unknown` and `wasm32-wasip1`), which
  `rustup toolchain install` installs from the checkout.
- Node 26 (`.node-version`). The published package supports Node 22.12 or
  later; the tests run on Node 26.
- CPython 3.12 for the oracle (`.python-version`), installed by `uv sync`
  into `.venv` with pdfminer.six 20260107 and pdfplumber from
  `upstream/pdfplumber-python`. pdfplumber's output depends on the Python
  minor version (3.12 to 3.14 give the same results, 3.11 does not), so the
  oracle is pinned to 3.12.
- The git submodules in `upstream/`: `pdfplumber-python` and `pdfminer-six`
  (the exact sources the port follows and the oracle runs) and `pdfjs`
  (only its test PDFs, part of the corpus).
- For the browser tests, Playwright's Chromium, Firefox and WebKit
  (`npx playwright install`). On a Debian or Ubuntu machine without root,
  `scripts/webkit-local-libs.sh` fetches the system libraries WebKit needs.
- For mutation testing, `cargo-mutants` (27.1).
- `systemd-run --user` (a systemd user session) for the memory caps of
  `scripts/heavy.sh` and `scripts/check.sh`.

## Setup

```sh
scripts/setup-dev.sh
```

It initializes the submodules, creates `.venv` (`uv sync`) and checks that
it is CPython 3.12, runs `npm ci`, installs the Rust toolchain and the
`wasm32-unknown-unknown` target, and downloads the external PDFs (see
[The Corpus](#the-corpus)). It is safe to re-run.

In a git worktree, point `PDFPLUMBER_ROOT` at the main checkout: the
downloaded PDFs, the initialized submodules, `.venv` and the shared
`.cache/` live there.

```sh
export PDFPLUMBER_ROOT=/path/to/main/checkout
export PYTHONPATH=.            # this checkout's oracle/ package
PY=$PDFPLUMBER_ROOT/.venv/bin/python
```

## Building

```sh
node scripts/build.mjs                    # the npm package in dist/
node scripts/build-wasm.mjs [--no-opt]    # dist/pdfplumber.wasm only
node scripts/build-wasm.mjs --oracle      # dist/pdfplumber-oracle.wasm, for the trace harness (never shipped)
cargo build --release -p pdftrace         # the trace binary, target/release/pdftrace
```

`scripts/build.mjs` runs `build-wasm.mjs` (`crates/wasm` for
`wasm32-unknown-unknown` with the `dist` profile, then `wasm-opt -O3` from
the `binaryen` devDependency; it fails without `wasm-opt` unless `--no-opt`
is given), `build-cmaps.mjs` (the predefined CMaps as
`dist/cmaps/*.pdfcmap.zlib`) and `build-js.mjs` (`tsc` for Node, esbuild
for browsers). The package layout and runtime are described in
[`docs/js-runtime.md`](docs/js-runtime.md).

## Testing

```sh
HEAVY_SLOTS=1 scripts/heavy.sh scripts/check.sh            # about 40 minutes
HEAVY_SLOTS=1 scripts/heavy.sh scripts/check-browsers.sh   # about 30 minutes
```

`scripts/check.sh` is the local CI: `rustfmt`, `clippy`, the upstream
`file:line` references, the generated tables, the unit oracle data
(regenerated when stale), `cargo test --release` natively and compiled to
`wasm32-wasip1` (run by Node's WASI), the deepest recursions on the
production Wasm profile, the JavaScript package (build, types, Node tests,
size budget, the package as a consumer installs it), the four `plumber_*`
stages through the public API on `micro` and `adv-plumber`, and the oracle
self-tests. `scripts/check-browsers.sh` builds the package with `wasm-opt`
and runs the runtime tests and the API sessions in Chromium, Firefox and
WebKit, in the page and in a worker, plus the bundler recipes in Chromium.
Both must pass before a change is merged.

The pieces, for a faster loop:

```sh
$PY -m oracle.unit                       # unit oracle data that is missing or stale (minutes when the real corpus is involved)
cargo test --release                     # Rust replays of the unit data
cargo test --release -p pdftrace --test layout_unit   # one replay
node node_modules/vitest/vitest.mjs run --config js/vitest.config.ts   # Node tests (after node scripts/build.mjs)
$PY -m unittest discover -s oracle/tests -t .           # oracle self-tests
$PY -m oracle.verify --stage layout --set quick         # one stage against Python on a PDF set
```

`oracle.verify` and the unit generators are described in
[`docs/verification.md`](docs/verification.md) and
[`oracle/README.md`](oracle/README.md). A direct `pdftrace` run of a stage
that interprets pages needs the predefined CMaps:
`--cmap-dir <cache>/cmap/<fingerprint>` or `PDFPLUMBER_CMAP_DIR`
(`oracle.verify` passes it itself).

## The Corpus

| Directory | Content | Source |
|---|---|---|
| `corpus/micro` | small generated PDFs, one feature each | `oracle/generate_micro_pdfs.py` (deterministic, checked in) |
| `corpus/upstream-contract` | upstream regression PDFs | checked in, provenance in `manifest.json` |
| `corpus/parity-cycles` | real-world PDFs sampled in batches | checked in, or selected pages of `corpus/external` |
| `corpus/external`, `corpus/external-holdout` | larger public PDFs | downloaded, SHA-256 checked |
| `upstream/*` | pdfplumber's and pdfminer's test PDFs, pdf.js's test suite | git submodules |
| `<cache>/corpus/adv-*` | adversarial corpora | generated by `oracle/adversarial/` |

```sh
node scripts/download-corpus.mjs          # or npm run corpus:download (also run by setup-dev.sh)
```

downloads every PDF of `corpus/external/manifest.json` and
`corpus/external-holdout/manifest.json` and checks its SHA-256 (`--id`,
`--manifest`, `--force`, `--dry-run`). A file whose URL no longer answers
can be copied from a machine that has it. Provenance and selection
criteria: [`corpus/SOURCES.md`](corpus/SOURCES.md). The sets the oracle
runs on (`micro`, `quick`, `all`, `adv-<stage>`...) are listed in
[`docs/verification.md`](docs/verification.md#corpus-sets).

## Investigating A Difference With Python

1. Reproduce it on the smallest stage that shows it:
   `$PY -m oracle.verify --stage S --files the.pdf` prints the first
   differing line of the trace with its context. Stages go from the lowest
   (`psparser`, `objects`, `streams`, `pages`) through the interpreter
   (`fonts`, `interp`, `layout`) to pdfplumber (`plumber_objects`,
   `plumber_text`, `plumber_tables`, `plumber_doc`): if a higher stage
   differs, run the lower ones on the same file until the first one that
   differs. `$PY -m oracle.trace --stage S the.pdf` and
   `target/release/pdftrace --stage S the.pdf` print both traces.
2. Read the upstream code at the `file:line` reference of the Rust function
   that wrote the line, and find where the port departs from it. The cause
   is a missing branch, another evaluation order, a Python semantic not
   followed (int against float, `sum`, `repr`, sort stability, a recursion
   frame or C unit), never "a font" or "a file".
3. Fix it there, the way upstream does it. No font name, file fingerprint,
   page-specific condition or threshold without an upstream counterpart.
4. Pin it: add the smallest case that shows it to the generator that owns
   the module (`oracle/unit/<module>.py`) or to an adversarial corpus
   (`oracle/adversarial/<stage>.py`), with Python computing the expected
   result. Never write an expected value by hand.
5. Run the module's replays and its mutation pass
   ([`docs/verification.md`](docs/verification.md#mutation-testing)), then
   the stage on `all` and the adversarial sets, then `scripts/check.sh`.

A known, documented difference (memory addresses, `RegExp` semantics...) is
listed in [`docs/public-api.md`](docs/public-api.md#differences-from-python)
and in [`docs/verification.md`](docs/verification.md#parity-status).

## Heavy Jobs And Resource Limits

The development machine may be shared, and some jobs can take all of it: a
mutant that loops while allocating, a probe that records too much, a pass
over the whole corpus.

- Run heavy jobs through `scripts/heavy.sh COMMAND...`: `scripts/check.sh`,
  `scripts/check-browsers.sh`, `oracle.verify` or `oracle.api_parity` on a
  large set, `oracle.unit` regeneration, adversarial corpus generation,
  `cargo mutants`. It waits for one of `HEAVY_SLOTS` machine-wide slots
  (lock files under `$PDFPLUMBER_ROOT/.cache/locks`, shared by every
  worktree) and runs the command in a systemd scope capped at `HEAVY_MEM`
  (default 5G) without swap. Export `HEAVY_SLOTS=1` on a shared machine:
  heavy jobs then run one at a time.
- Run exploratory and measurement scripts (profilers, depth probes,
  one-off scans) under explicit limits:
  `timeout 15m prlimit --as=4294967296 -- $PY probe.py`. `prlimit` takes
  bytes: `--as=4G` sets a 4-byte limit and every command fails at start.
- A probe that follows the Python stack keeps the current and maximum depth
  only, and copies the stack only at a new maximum (`oracle/depth_probe.py`).
- `CARGO_BUILD_JOBS=2` keeps a build from taking every core.
- Temporary files of `oracle.verify` and `oracle.api_parity` go to
  `<cache>/tmp`, which is empty after a run that finished.

## Commit Conventions

- One logical change per commit. The subject is one line saying what
  changed, prefixed by the area when it helps (`layout: ...`,
  `js_api: ...`, `docs: ...`); the body says why, with the upstream
  `file:line` when the change follows upstream code.
- A commit that changes behavior includes the case that shows it (unit
  case, adversarial PDF or API session) and passes `scripts/check.sh`.
- Never commit generated outputs: `dist/`, `target/`, `.cache/`, the
  downloaded PDFs. Generated sources that are checked in (the tables of
  `oracle/codegen/`, `js/src/generated/`) are regenerated by their script,
  never edited by hand.
- Editing a module of `oracle/` changes the fingerprint of the caches built
  from it (oracle traces, unit data): say so in the commit body, since the
  next `scripts/check.sh` regenerates the unit data.
