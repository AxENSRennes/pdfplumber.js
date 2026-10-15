# Python oracle

Python side of the parity contract described in
[`docs/verification.md`](../docs/verification.md): canonical traces of
pdfminer.six 20260107 / pdfplumber 0.11.9 under CPython 3.12, the PDF sets they
run on, a cache of those traces, and the comparison with the Rust `pdftrace`
binary.

Everything runs with the oracle interpreter and only the standard library plus
the installed pdfminer/pdfplumber. From a checkout (or a worktree):

```bash
PY=/path/to/main/checkout/.venv/bin/python     # CPython 3.12 + pdfminer + pdfplumber
export PYTHONPATH=.                            # this checkout's oracle/ package
export PDFPLUMBER_ROOT=/path/to/main/checkout  # corpus/, upstream/, .cache/ (default: this checkout)
```

In a git worktree, `PDFPLUMBER_ROOT` must point at the main checkout: that is
where the downloaded PDFs (`corpus/external*`), the initialized submodules
(`upstream/*`) and the shared cache (`.cache/`) are. `PDFPLUMBER_CACHE`
overrides the cache directory (default `$PDFPLUMBER_ROOT/.cache`).

## Layout

| Path | Role |
|---|---|
| `canon.py` | `canon(v)`: the canonical text of a value (trace format) |
| `typed_json.py` | lossless JSON encoding of Python values (unit cases) |
| `corpus.py` | PDF sets (`micro`, `upstream`, `real`, `pdfjs`, `quick`, `all`, `adv-<stage>`) and per-file passwords |
| `stages/<name>.py` | one stage each; the docstring lists the upstream code exercised, the exact traversal and every line format |
| `stages/__init__.py` | stage registry and runner (`trace_lines`) |
| `trace.py` | CLI: one trace on stdout |
| `cache.py` | trace cache under `.cache/oracle/` |
| `verify.py` | CLI: build the cache, run `pdftrace`, compare |
| `unit/<module>.py` | generators of unit cases replayed by `crates/pdftrace/tests/<module>_unit.rs` (`unit/_common.py`: output and `meta.json`) |
| `unit/__main__.py` | CLI: regenerate the unit data that is missing or stale |
| `upstream_refs.py` | CLI: check every upstream `file:line Name` reference of `crates/` and `oracle/` against `upstream/` |
| `generate_micro_pdfs.py` | deterministic generator of `corpus/micro` (+ its manifest) |
| `tests/` | `python -m unittest` self-tests (canon, typed JSON, runner, verify with a fake `pdftrace`) |

## One trace

```bash
$PY -m oracle.trace --stage psparser corpus/micro/form-xobject-matrix.pdf
$PY -m oracle.trace --stage objects --password user corpus/micro/encrypted-password.pdf
$PY -m oracle.trace --stage pages --max-pages 0 some.pdf    # 0 = every page
```

Stages today: `psparser`, `objects`, `streams`, `pages` (the list is
`oracle/stages/`). Paged stages stop after `--max-pages` pages (default 20).
The output is ASCII; it ends with `!end`, or with the exception that stopped
the stage (`!PDFPasswordIncorrect("")`). Exit status 0 whenever a trace was
written.

## Corpus sets

```bash
$PY -m oracle.corpus --set quick            # sha256, size, path, password
$PY -m oracle.corpus --set all --summary
$PY -m oracle.corpus --set all --check      # password conflicts between sources
```

Passwords come from `corpus/passwords.json` (upstream test files, each entry
citing the upstream test that opens it), then any `password` /
`openOptions.password` in the corpus manifests (`corpus/micro/manifest.json`
for the encrypted micro PDF), then `upstream/pdfjs/test/test_manifest.json`.
They are keyed by SHA-256.

## Comparing with Rust

```bash
cargo build --release -p pdftrace
$PY -m oracle.verify --stage psparser --set quick
$PY -m oracle.verify --stage objects --set all -j 3 --json > objects.json
$PY -m oracle.verify --stage pages --files a.pdf b.pdf --pdftrace target/debug/pdftrace
```

`verify` builds the missing oracle traces (`-j` processes, default 3;
`--timeout` 120 s per file, recorded as `!Timeout <limit>`), runs
`pdftrace --stage S --max-pages N [--password=P] --out DIR F1.pdf …` in
batches of files that share a password (`--batch-size`, default 64; a batch
is killed when no file completes for `--timeout` seconds, and a file that
took longer than that, per `DIR/_times.tsv`, is a `rust-timeout`), compares
byte for byte and prints, for each differing file, the first differing line
with 3 lines of context from both sides:

```
--- corpus/micro/vector-objects.pdf  (sha256 e38ebe1d57699c88, first difference at line 4, column 8)
  oracle:
        1 page 1 2
        2 streams [1]
        3 0 2
  >     4 2 Kx"77"
        5 4 10
        6 7 10
        7 10 Kx"6d"
  rust:
        1 page 1 2
        2 streams [1]
        3 0 2
  >     4 2 Kx"77" !perturbed
        5 4 10
        6 7 10
        7 10 Kx"6d"
psparser on micro: 15 identical / 1 different / 0 rust-crashed / 0 rust-timeout / 0 oracle-timeout / 0 oracle-error / 16 total
```

(that report comes from the fake binary of `tests/`, told to perturb line 4).
Lines longer than 240 characters are clipped around the first differing
column.

Exit status: 0 only if at least one file was compared and all are identical;
1 on any difference, Rust crash or timeout, oracle error, oracle timeout
(unless `--allow-timeouts`, which lists them) or when nothing was compared;
2 if `pdftrace` is missing or answers "stage not implemented" (its exit
status 2), or if the set cannot be listed (unknown name, `adv-<stage>` never
generated, missing directory). A failing batch keeps the traces it completed
and re-runs each remaining file alone, so that a crash is attributed to the
right PDF. `--keep` keeps the Rust outputs,
`--limit K` takes the first K files of the set.

Warm the cache without Rust (and see the slowest files and timeouts):

```bash
$PY -m oracle.verify --stage streams --set all --oracle-only
$PY -m oracle.verify --stage streams --set all --oracle-only --retry-timeouts --timeout 600
```

## Cache

`.cache/oracle/<stage>/<fingerprint>/<sha256>.txt` (`<sha256>.pw-<hash>.txt`
when a password is used). The fingerprint hashes the stage module (and the
stages in its `DEPENDS`), `stages/__init__.py`, `stages/_common.py`,
`canon.py`, `typed_json.py`, `trace.py`, `environment.py`, the oracle
environment (`environment.py`: interpreter, every installed package version,
the installed pdfminer/pdfplumber files, zlib, the C library and the CPU
flags that select its `pow`) and, for paged stages, `--max-pages`; each
directory has a `fingerprint.json` saying what was hashed and a
`_builds.jsonl` with one record (time, size, status) per built trace. Old
fingerprint directories are never read again and can be deleted.

## How the oracle runs pdfminer

- Documents are opened as pdfplumber opens them:
  `PDFDocument(PDFParser(open(path, "rb")), password=password or "")`
  (`caching=True`, `fallback=True`, `settings.STRICT` left at `False`). No
  monkeypatching.
- A stage runs in a fresh thread, so the stage always starts at the same Python
  stack depth (`stages.STAGE_ENTRY_DEPTH`) under the default recursion limit:
  a `RecursionError` happens at the same place for every caller. The thread has
  a 256 MiB C stack so that deep C recursion raises `RecursionError` instead of
  crashing (the limits count frames, not bytes, so this cannot change a trace).
- `logging` is disabled during a stage (pdfminer never reads its log level, so
  results are unchanged); warnings keep their default filters.
- Each cached trace is computed in its own process, with its own random
  `PYTHONHASHSEED`: a trace that depended on set or hash order would show up
  as a cache that differs between two builds.

## Unit cases

```bash
$PY -m oracle.unit                 # regenerate what is missing or stale (--check: only report, --force: all)
$PY -m oracle.unit.canon           # one generator -> $PDFPLUMBER_CACHE/unit/canon/<fingerprint>/
```

Generators: `canon` (the Rust `canon`/typed JSON), `pycompat` (CPython
semantics), `exceptions` (pdfminer/pdfplumber exception classes), `pdftypes`
(object model), `psparser_base` (`PSStackParser` on a file), `psparser`
(`PDFContentParser` on the content streams of the `all` set, and with
`--fuzz 4000` on an adversarial corpus), `zlib` (CPython's `zlib`
decompression on zlib 1.3) and `filters` (the stream filters, predictors
and `PDFStream.get_data`, on generated cases and every stream of the
`all` set). Each writes typed JSON lines
`{"in": [args...], "out": result}` (fixed seed) and a `meta.json` whose
`sources` holds the SHA-256 of every `oracle/` module the generator loaded
and `environment` the oracle environment (`environment.py`), in a directory
named by a hash of both: checkouts sharing the cache never mix their files.
The Rust replay takes the most recent directory whose sources are those of
its checkout, and refuses to run without one. `scripts/check.sh` replays the
cases natively and compiled to WebAssembly (`wasm32-wasip1` under Node's
WASI, `scripts/wasi-runner.mjs`).

The `canon` cases (about 6 300) are edge cases of every type (ints past the
4 300-digit `str()` limit, subnormals, NaN payloads, `repr` exponent
thresholds, lone surrogates, bytes of 256/257 bytes, decoded streams,
exceptions with memory addresses, `Named` objects, deep nesting) and random
values (seed 1), as `{"in": [<typed JSON value>], "out": "<canon>"}`. Each case
is checked to round-trip through typed JSON before it is written.

## Self-tests

```bash
$PY -m unittest                              # from the repository root, with PYTHONPATH=.
$PY -m unittest oracle.tests.test_verify     # one module
```

They need only this checkout (`corpus/micro`) and use a temporary cache.
`tests/test_verify.py` drives `verify.py` with `tests/fake_pdftrace.py`, a
stand-in for the Rust binary that copies the oracle's traces and can perturb a
line, truncate a trace, crash on a file or answer "not implemented" (also
through `--list-stages`, which `verify.py` asks before building the oracle
cache).

## Micro PDFs

```bash
$PY oracle/generate_micro_pdfs.py
```

rewrites `corpus/micro/*.pdf` and `corpus/micro/manifest.json`, byte for byte
the same on every run (the encrypted file uses a fixed `/ID` and no random
salt: RC4 40-bit, revision 2, encrypted by the script itself).
