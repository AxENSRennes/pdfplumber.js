# Agent Instructions

- The goal is a JavaScript pdfplumber that behaves exactly like Python
  pdfplumber 0.11.9 on pdfminer.six 20260107: a Rust core ported module by
  module, compiled to WebAssembly behind a coarse-grained JavaScript API.
  Passing the tests is not the goal; behaving like Python is.
- Read [`docs/architecture.md`](docs/architecture.md) (layout, porting rules,
  value model, Python semantics) and
  [`docs/verification.md`](docs/verification.md) (traces, stages, corpora,
  mutation testing, definition of done) before changing the port.
  [`CONTRIBUTING.md`](CONTRIBUTING.md) has the commands.
- Python is the oracle (`.venv`, CPython 3.12, upstream sources in
  `upstream/`). Never write an expected value by hand when Python can
  produce it; generate a test PDF or a unit case instead.
- Fix a difference with Python at its cause, found by following the stage
  traces down to the first differing line. Never add a font name, a file
  fingerprint or an unexplained threshold to make a case pass.
- The machine is shared. Run every exploratory or measurement script
  (profilers, depth probes, one-off corpus scans) under explicit limits,
  e.g. `timeout 15m prlimit --as=4294967296 -- .venv/bin/python probe.py`
  (`prlimit` takes bytes: `--as=4G` is a 4-byte limit and every command
  fails at start). Run heavy jobs (`scripts/check.sh`,
  `scripts/check-browsers.sh`, `oracle.verify` on large sets, unit data
  regeneration, `cargo mutants`) through `scripts/heavy.sh` with
  `HEAVY_SLOTS=1`, one at a time. Never record the whole call stack on every
  call in a probe: keep the current and maximum depth only.
