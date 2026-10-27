# Verification

How the repository shows that the Rust port behaves like pdfminer.six and
pdfplumber. Commands are in [`oracle/README.md`](../oracle/README.md) and
[`CONTRIBUTING.md`](../CONTRIBUTING.md).

## Principle

For each stage of the pipeline, Python (the oracle) and Rust write **the
same canonical text trace** from the same input, and the two traces are
compared byte for byte. The first differing line locates the difference.
Nothing is ever compared with a numeric tolerance. Code without a PDF input
(filters, inflate, crypto, CMap parsing, layout analysis, tables from
edges, the WebAssembly boundary) is checked by **unit cases** that Python
generates and Rust replays. The JavaScript API is checked by **sessions**:
calls that Python runs as statements and the API replays. Expected values
always come from Python, never from a hand-written file.

## Canonical Trace Format

A trace is a sequence of `\n`-terminated ASCII lines (anything else goes
through JSON escapes). Each stage defines its lines (a label, then values);
values are written by `canon(v)` (`oracle/canon.py`,
`crates/pdftrace/src/canon.rs`, `js/trace/canon.mjs`):

| Python value | Written as | Examples |
|---|---|---|
| `None`, `True`, `False` | as is | `None` |
| `int` (any size) | decimal | `-12`, `0` |
| `float` | Python's `repr()` | `1.0`, `0.1`, `1e-05`, `1e+16`, `-0.0`, `inf`, `nan` |
| `str` | `json.dumps(s, ensure_ascii=True)` | `"caf\u00e9"` |
| `bytes` (≤ 256 bytes) | `x"` lowercase hex `"` | `x"0d0a"` |
| `bytes` (> 256 bytes) | `x#` length `:` first 16 hex digits of the SHA-256 | `x#5120:9f86d081884c7d65` |
| `tuple` | `(a, b)`; one element `(a)` | `(1.0, 2)`; empty `()` |
| `list` | `[a, b]` | `[]` |
| `dict` | `{k: v, ...}` in insertion order | `{"Type": /"Page"}` |
| `PSLiteral` | `/` + canon(name) (`str` or `bytes`) | `/"F1"`, `/x"ff"` |
| `PSKeyword` | `K` + canon(bytes name) | `Kx"4254"` |
| `PDFObjRef` | `R` + canon(objid) | `R12` |
| `PDFStream` | `Stream(objid, genno, attrs, raw)`, `raw` = canon(`rawdata`) | `Stream(5, 0, {"Length": 300}, x#300:...)` |
| exception | `!` + class name + `(` + json(str(e)) + `)` | `!PSEOF("Unexpected EOF")` |
| stage object | name + canon(dict of fields) | `LTChar{"x0": 1.0}` |

- The separator is always `", "`; `dict` keys go through `canon` too.
  `bool` is tested before `int`. Every NaN is `nan` (sign and payload
  ignored, as `repr` does). An `int` past the 4,300-digit limit of `str()`
  is still written in decimal; the global limit is never changed, so
  pdfminer meets it as a user would.
- A `str` character outside the BMP is a pair of escapes (`"\ud83d\ude00"`),
  a lone surrogate one escape (`"\ud800"`); a Python string made of the two surrogates
  of a pair is therefore written as the character they encode.
- `PDFStream`: `rawdata` is `None` once the stream is decoded (`decode()`
  empties it), and `raw` is then `None`: the trace shows the state of the
  decoding cache, which the port reproduces, so each stage fixes the order
  of its accesses. For an encrypted document, `raw` is the stored,
  encrypted content.
- An exception is named `type(e).__name__` for the classes of `builtins`,
  pdfminer, pdfplumber and `oracle`, `module.name` otherwise
  (`zlib.error`, `struct.error`, `binascii.Error`). A default repr's
  address in a message (` at 0x7f3a...>`) becomes ` at 0x?>`. An exception
  of pdfplumber that wraps another is followed by ` cause ` and that one.
- `canon` is iterative (any depth). A cycle or an unknown type raises
  `CanonError`, a `BaseException`: a harness error is never caught by a
  stage's `except Exception` nor written as if pdfminer had raised it.
- A trace ends with `!end` when the stage finished, or with the line of the
  exception that stopped it. An exception caught in a loop (one unreadable
  object among others) is written in its place.
- No memory address appears in a trace: inline image names are written
  `inline{"n": k}`, and the `layout` and `plumber_objects` stages write the
  rank of their analysis's `id()`s (`ids` lines,
  [Layout Tie Order](architecture.md#layout-tie-order)).

`canon` is tested between Python and Rust on about 6,300 generated values
(`oracle/unit/canon.py`, `crates/pdftrace/tests/canon_unit.rs`): edge cases
of every type and random values, each also round-tripped through typed
JSON.

## Typed JSON

Unit cases are JSON Lines, `{"in": [args...], "out": result}`, with Python
values encoded without loss, keeping int/float and tuple/list apart
(`oracle/typed_json.py`, `crates/pdftrace/src/typed_json.rs`,
`js/trace/typed-json.mjs`):

| Python value | JSON |
|---|---|
| `None`, `True`, `False`, `str` | `null`, `true`, `false`, a JSON string |
| `str` holding a surrogate | `{"s": [<code point>, ...]}` (UTF-8 cannot carry it) |
| `int` | `{"i": "<decimal>"}` |
| `float` | `{"f": "<16 hex digits of the IEEE-754 bits, big-endian>"}` |
| `bytes` | `{"b": "<hex>"}` |
| `list` | JSON array |
| `tuple` | `{"t": [...]}` |
| `dict` | `{"d": [[key, value], ...]}` (insertion order) |
| `PSLiteral`, `PSKeyword` | `{"lit": <str or {"b": ...}>}`, `{"kw": "<hex>"}` |
| `PDFObjRef` | `{"ref": objid}` (a JSON integer when it fits an i64, else a typed value) |
| `PDFStream` | `{"stream": {"objid": v, "genno": v, "attrs": {"d": ...}, "rawdata": v, "data": v}}` |
| exception | `{"exc": "<class>", "msg": "<str(e)>"}` (class named as in `canon`) |
| stage object | `{"named": "<name>", "d": [[key, value], ...]}` |

Every object has one key, except `exc` (+ `msg`) and `named` (+ `d`).
`dumps` writes compact ASCII JSON with keys in that order, so a re-encoding
can be compared with the original text. A NaN of the `text_tables*` cases
carries in its payload the number of its Python object in the case
(`oracle/unit/_nan_objects.py`), so the replay knows which values are the
same object ([NaN Identity](architecture.md#nan-identity)).

## Stages

Each stage has a stable name, a Python implementation
(`oracle/stages/<name>.py`) and a Rust one
(`crates/pdftrace/src/stages/<name>.rs`). The head of each Python module is
the specification: the upstream code exercised, the exact walk (the order
of `getobj` and `get_data` calls, which have side effects through the
caches) and the format of every line.

| Stage | Traced, per PDF |
|---|---|
| `psparser` | the `(pos, obj)` of `PDFContentParser` on the content streams of each page and Form XObject |
| `objects` | the xref (type, trailer, `get_pos` of each entry), catalog, info, encryption, then every indirect object resolved by `getobj`, errors included |
| `streams` | every stream: filters, parameters, decoded data or exception |
| `pages` | the page tree: inherited attributes, boxes, rotation, resources, contents, labels; pdfplumber's `Page` boxes and `doctop` |
| `fonts` | the fonts the interpreter gets (the page's `fontmap`, the resource manager's cache): class and attributes, then for each string shown `decode` and, per code, `to_unichr`, `char_width`, `char_disp` |
| `interp` | every call of the interpreter to pdfplumber's device (`set_ctm`, pages, figures, `paint_path`, `render_image`, `render_string`/`render_char` and the advance returned, tags), with the graphic and text states, and where exceptions surface |
| `fonts_text`, `interp_text` | `fonts` and `interp` on pdfminer's `PDFTextDevice` (`render_char` returns 0): the interpreter without the converter |
| `layout` | the raw `LTPage` (`laparams=None`, pdfplumber's default), then analyzed (`LAParams()`), every attribute, and the `ids` lines |
| `plumber_objects` | `pdfplumber.open(path, pages=[1..N], ...)`: boxes and `doctop` of each page, `page.objects` of every kind, annotations, links, `pdf.objects`/`annots`/`hyperlinks`; then the same with `laparams={}` (text boxes and lines, chars by reference, `ids`) |
| `plumber_text` | `extract_text` (simple, `layout=True`, option sets), `extract_text_simple`, `extract_words`, `extract_text_lines`, `search`, `dedupe_chars`, on the page and on cropped, filtered and deduplicated pages |
| `plumber_tables` | `debug_tablefinder`, `find_table(s)`, `extract_table(s)` for `lines`, `lines_strict`, `text`, `explicit` and setting variants, on the page and derived pages |
| `plumber_doc` | metadata (`strict_metadata` both ways), structure trees (`find_all`, `find`, `all_mcids`, `element_bbox`, `repr`), `crop`/`within_bbox`/`outside_bbox`, `filter`, `dedupe_chars`, `to_dict`/`to_json`/`to_csv`, `close`, `unicode_norm`, `raise_unicode_errors`, `pages`, passwords |

A stage's cache depends on the stages listed in its module's `DEPENDS`
(`plumber_text`, `plumber_tables` and `plumber_doc` build on
`plumber_objects`); changing a Python stage rebuilds its cache and theirs.

### Common Rules

- Documents are opened as pdfplumber opens them:
  `PDFDocument(PDFParser(open(path, "rb")), password=password or "")`
  (`caching=True`, `fallback=True`, `STRICT=False`). Nothing in pdfminer or
  pdfplumber is patched or hooked.
- A stage runs in a fresh thread, so its body always starts at the same
  Python stack depth (`STAGE_ENTRY_DEPTH`), under the default recursion
  limit: a `RecursionError` happens at the same place whatever the caller.
  The thread has a 256 MiB C stack so that a deep C recursion raises
  `RecursionError` instead of crashing (the limits count frames, not bytes).
  `oracle.stages.entry_depth()` measures that depth (frames and remaining C
  units, `oracle/depth.py`); `pdftrace::stages::STAGE_ENTRY` holds it and a
  unit test compares the two.
- Each trace is computed in its own process with its own random
  `PYTHONHASHSEED`, so the starting process state is a new process's.
- Paged stages take the first `--max-pages` pages (20 by default, 0 for
  all), created lazily: `create_pages` is not resumed after the last page
  asked for.
- `fonts`, `interp` and `layout` drive the pages as pdfplumber does
  ([Driving Pages](architecture.md#driving-pages)). `interp` and `fonts`
  observe the calls through a subclass of the device (an extension point,
  not a patch) that writes nothing during interpretation; `layout` adds no
  override.

### Logging Disabled

`logging` is disabled during a stage (`logging.disable(logging.CRITICAL)`),
and the port counts the frames of that configuration
(`pycompat::logging`). pdfminer never reads its log level, so results do not
change, but a logging call costs Python frames: `log.debug(...)` runs
`Logger.debug` then `Logger.isEnabledFor` (two frames under the caller,
three at the first call of a level on a logger, which fills a cache), and
that decides where a `RecursionError` falls in a deep recursion
(`create_pages` logs at every node). Disabled logging is also, frame for
frame, the usual configuration that silences pdfminer
(`logging.getLogger("pdfminer").setLevel(logging.ERROR)`), and what the
JavaScript API does without `onWarning`.

It is not Python's default configuration (no handler, level `WARNING`),
where a `log.warning` runs `_log`, `findCaller`, `makeRecord` and the
`lastResort` handler, up to 11 frames deeper, and formats the `%r` of its
arguments: on an operand nested near the C limit that `repr` raises
`RecursionError` and the page fails, and a chain of Form XObjects whose
deepest Form warns fails one level earlier. That configuration is not
modelled: the reference stays one fixed configuration that the oracle runs
and the port reproduces.

### The plumber Stages

The four `plumber_*` stages drive pdfplumber as a user does
(`pdfplumber.open`, `open_pdf` in `oracle/stages/plumber_objects.py`);
`--max-pages` becomes the `pages` option. Their option sets are tables
(`oracle/codegen/plumber_ops.py` writes them for Rust and for `js/trace`),
of which each page takes a deterministic slice (`select`, from the file's
SHA-256 and the page's rank). A result longer than 65,536 characters is
written as its length and digest. `re` is not ported: `plumber_text` writes
Python's match spans before each `search` result, and `pdftrace` builds the
results from them. The stages model their own property loads whose getter
can interpret a page (`pdf.pages`, `page.objects`, `page.chars`,
`page.edges`...) and their bound-method calls with `**kwargs`
([Call-Site Specialization](architecture.md#call-site-specialization)).

## Corpus Sets

`oracle/corpus.py` lists the sets, relative to the corpus root
(`PDFPLUMBER_ROOT`):

| Name | Content |
|---|---|
| `micro` | `corpus/micro`: small generated PDFs, one feature each |
| `upstream` | `upstream/pdfplumber-python/tests/pdfs`, `upstream/pdfminer-six/samples`, `corpus/upstream-contract` |
| `real` | `corpus/external`, `corpus/external-holdout`, `corpus/parity-cycles` |
| `pdfjs` | `upstream/pdfjs/test/pdfs` (937 PDFs, many malformed) |
| `quick` | `micro`, plus the files of `all` whose SHA-256 starts with a byte below 18 (about 7 %, 150 PDFs): membership depends only on a file's content |
| `all` | everything above (1,853 distinct PDFs) |
| `adv-<name>` | the adversarial corpus written by `oracle/adversarial/<name>.py`: `adv-objects` (333), `adv-pages` (285), `adv-interp` (353), `adv-plumber` (199), `adv-structure` (441) |

A file is any `*.pdf` (case ignored) under the folders, recursively, in
relative path order; identical files are deduplicated by SHA-256 (the first
occurrence wins, in table order). Passwords are found by the file's
SHA-256: `corpus/passwords.json` (upstream test files, each entry citing the
upstream test that opens it), then the `password`/`openOptions.password`
fields of the manifests, then `upstream/pdfjs/test/test_manifest.json`;
`python -m oracle.corpus --check` reports conflicts.

## Adversarial Corpora

The real corpus does not reach the boundaries: 4,096-byte blocks, exact
tolerances, escapes, int against float, missing or mistyped objects, the
recursion limits. Each generator of `oracle/adversarial/` writes PDFs aimed
at them, with a fixed seed, into `<cache>/corpus/adv-<name>/` (with a
`manifest.json` of passwords); the unit generators that use a corpus
rewrite it first, byte for byte the same.

- `objects.py` → `adv-objects`: broken or missing xref tables (the
  `PDFXRefFallback` path), wrong offsets, xref streams with odd `/W`,
  `/Index`, predictors and fields, object streams with odd `/N`, `/First`
  and ids, `/Prev`/`/XRefStm` chains and loops, incremental updates,
  missing trailer or `/Root`, garbage around the file, `\r` and `\r\n` line
  ends, indirect, negative, huge or self-referencing `/Length`s, block
  boundaries, and every standard encryption (RC4 40/56/128, AES-128,
  Identity and V2 filters, AES-256 revisions 5 and 6) with the user, owner,
  wrong or empty password.
- `pages.py` → `adv-pages`: inheritance, loops and sharing in the page tree,
  odd `/Kids`, no `/Pages`, deep trees, every kind of `/Rotate`, box and
  `/Contents`, reference cycles, page labels of every style, outlines, name
  trees and destinations.
- `interp.py` → `adv-interp` (checked by `interp_text`, `fonts_text`,
  `interp`, `fonts`, `layout`, and the `plumber_*` stages): every operator
  with missing, extra or mistyped operands, keywords spelled like methods
  (`T_a`, `_q`), extreme text state, `TJ` on any object, Type 3 and vertical
  `Type0` fonts, predefined and unknown CMaps, every color space and `scn`
  arity, paths and paints, unbalanced `q`/`Q` and marked content, inline
  images on block boundaries, Form XObjects with odd `/BBox`, `/Matrix`,
  `/Resources` and cycles.
- `plumber.py` → `adv-plumber` (checked by the four `plumber_*` stages):
  every object kind with atypical attributes, `bytes` font names, page
  boxes and rotations, nested Forms, `pages=` and `laparams` of any type,
  layout ties, degenerate text and tables. No NaN in the keys
  `cluster_objects` puts in a `set` (their order follows their address in
  CPython).
- `structure.py` → `adv-structure` (checked by `plumber_doc` and
  `plumber_objects`): structure trees of every shape (`/K` of any kind,
  MCRs and OBJRs, DAGs, cycles at the document scope, `/RoleMap`,
  `/ClassMap`, attributes with revisions and references), parent trees,
  metadata of any type (shared, nested, self-referencing), annotations with
  undecodable strings. No cycle through a page's parent tree: Python loops
  forever there.

The stages also run on the adversarial sets of other stages: each set is
checked by every stage.

## Tools

### Traces

- `python -m oracle.trace --stage S [--password P] [--max-pages N] FILE.pdf`
  prints the Python trace; `pdftrace --stage S ...` the Rust one, with the
  same defaults. `pdftrace ... --out DIR F1.pdf F2.pdf...` writes
  `DIR/<sha256>.txt` per file and appends `<sha256>\t<seconds>` to
  `DIR/_times.tsv`; exit status 2 means "stage not implemented";
  `pdftrace --list-stages` lists the stages. Both accept `--option=value`;
  a password is always passed as `--password=P` (it may start with `-`).
- `pdftrace --oracle-dir DIR`: for a stage whose Python module declares
  `ORACLE_INPUT` (`layout` and `plumber_objects`: the `id()` ranks;
  `plumber_text`: the `re` spans), the oracle's trace of each file, which
  Rust reads (`StageInput::oracle`); other stages refuse it. Without it,
  `layout` analyzes with `CreationOrder`.
- `pdftrace --cmap-dir DIR` (default `$PDFPLUMBER_CMAP_DIR`): the provider
  of the predefined CMaps (`<cache>/cmap/<fingerprint>/`, written by
  `oracle/codegen/cmaps.py`). Every stage that interprets pages needs it;
  without it a font with a predefined CMap gives `RuntimeError("no CMap
  provider installed ...")`.

### Comparing With Python

`python -m oracle.verify --stage S --set NAME [-j N] [--max-pages N]
[--pdftrace PATH] [--cmap-dir DIR] [--limit K] [--files ...] [--json]
[--engine js-api]` builds the missing oracle traces (`-j` processes, 3 by
default), runs `pdftrace` in batches of files that share a password, always
with the CMap provider (`--cmap-dir`, by default the directory of
`oracle/codegen/cmaps.py` under the cache; a directory without `names.txt`
stops `verify` with status 2) and with `--oracle-dir` for the stages that
take it, compares byte for byte, and prints the counts (identical,
different, Rust crashes, Rust timeouts, oracle timeouts, oracle errors) and,
for each differing file, the first differing line with three lines of
context on each side.

`--timeout` (120 s by default) applies per file on both sides: a batch is
killed when no file completes for that long, and a file whose time
(`_times.tsv`) exceeds it is a Rust timeout even if its trace is right. A
failing batch keeps the traces it completed and runs each remaining file
alone, so that a crash is attributed to the right PDF. `--oracle-only` only
fills the cache (`--retry-timeouts` rebuilds the timeouts). Exit status: 0
only if at least one file was compared and all are identical; 1 on any
difference, crash, timeout (unless `--allow-timeouts`, which lists them),
oracle error, or when nothing was compared; 2 when `pdftrace` is missing or
does not implement the stage (asked before the oracle cache is built), or
when the set cannot be listed (unknown name, `adv-<name>` never generated).

### Oracle Cache

`<cache>/oracle/<stage>/<fingerprint>/<sha256>.txt` (`.pw-<12 hex>.txt` with
a password; `<cache>` is `PDFPLUMBER_CACHE` or `$PDFPLUMBER_ROOT/.cache`).
The fingerprint hashes the stage module and those of its `DEPENDS`,
`stages/__init__.py`, `stages/_common.py`, `canon.py`, `typed_json.py`,
`trace.py`, `environment.py`, the oracle environment (interpreter, every
installed package version, the installed pdfminer and pdfplumber files,
zlib, the C library and the CPU flags that select its `pow`) and, for paged
stages, `--max-pages`: any change rebuilds the cache. Writes are atomic.
Each trace is computed in its own process; a file that does not finish
within the timeout gets the trace `!Timeout <limit>`, never counted as a
success, and rebuilt when a later run allows more time. Building every
stage on `all` takes hours of CPU; use `--timeout 600` for it.

### Wasm Parity

The WebAssembly build is checked by the same unit replays compiled to
`wasm32-wasip1` and run by Node's WASI (`scripts/check.sh`), by
`scripts/wasm-depth-gate.sh` (the `dist` profile on the depth windows of
the adversarial sets, compared with native), and by `python -m
oracle.wasm_parity --set quick [--stages ...]`: `pdftrace` compiled to
WebAssembly (`dist`, under Node's WASI) against the native one, file by
file, both given what `oracle.verify` gives the native one (the CMap
provider explicitly, and for `layout` the oracle cache), so that an
identical trace is Python parity wherever `verify` finds the native one
identical. Build both binaries first (`cargo build --release -p pdftrace
--bin pdftrace` and `cargo build --profile dist --target wasm32-wasip1 -p
pdftrace --bin pdftrace`). `wasm-depth-gate.sh` runs `layout` without the
oracle cache: its traces are those `verify` checks only because none of
its windows has a page whose analysis depends on the `id()` ranks (the 188
`depth-*` and `climit-*` files of `adv-interp` give the same native
`layout` trace with and without `--oracle-dir`); for a new window with
ties, the gate checks the stack, not the parity.

## Parity Through The JavaScript API

The public API is checked against Python through the API itself, in Node,
Chromium, Firefox and WebKit, in the page and in a worker.

- **Sessions.** `oracle/unit/js_api.py` writes sessions: calls on the API
  in a neutral form (steps of calls issued without waiting, then awaited in
  a given order; inputs, arguments in their JavaScript forms, aliases), the
  statements of `oracle/unit/js_pages_run.py` the API runs for them (the
  plan of [API Operations](js-runtime.md#api-operations)), and Python's
  results, each session run in a fresh process.
  `js/test/shared/api-sessions.mjs` replays them through the public API only
  (`js/test/node/api.test.ts` on the sources, `api-dist.test.ts` on the
  `dist` build, `js/test/browser/api.spec.ts` in the browsers). The
  replayer fails if an expectation is not compared, if the API does not
  make exactly the calls Python made, or if no call was made.
  `api-lifetime.test.ts` and `api-runtime.test.ts` check what has no Python
  value (identities, the garbage collector, crashes, the module cache).
- **Traces.** `python -m oracle.verify --engine js-api --stage
  plumber_{objects,text,tables,doc} --set NAME` runs `js/trace/pdftrace-js`
  (`pdftrace`'s command line over the built package, `dist/` with
  `dist/pdfplumber-oracle.wasm`: `node scripts/build.mjs`, `node
  scripts/build-wasm.mjs --oracle`) in place of the Rust binary: the four
  stages written through the public API (`js/trace/plumber_*.mjs`), each
  value read in the typed format (`core.lastTyped`, which keeps int/float
  and tuple/list apart) and written by `js/trace/canon.mjs`.
- **Oracle hooks.** The module `dist/pdfplumber-oracle.wasm` is the
  package's module built with the `oracle-hooks` feature
  (`crates/wasm/src/oracle.rs`, `js/src/hooks.ts`, reached through
  `pdfplumber-js/internal`), never shipped. It takes Python's `id()` ranks
  (`ids` lines), Python's `re` spans (in code points), and gives the
  identity of objects shared between a page and its derived pages. Nothing
  else differs from the shipped module.
- **Classification.** `python -m oracle.api_parity --stage S --set NAME`
  runs the oracle (cached), `pdftrace` and `pdftrace-js`, and classifies
  every API trace that is not Python's by its content: `core`, the API
  trace is `pdftrace`'s byte for byte (a known difference of the core,
  which the API does not add to); `filter`, the eager `filter()` of a page
  whose objects raise (Python writes the derived page's `dbox` and
  `dobjects <exc>`, the API rejects `filter()` with the same exception:
  [Cropping, Filtering, And Dedupe](public-api.md#cropping-filtering-and-dedupe));
  `window`, a `depth-*`/`climit-*` window identical up to a line where one
  side is a `RecursionError` (or has one as cause), or that Python
  reproduces with more recursion limit; `defect`, anything else (exit
  status 1), including a missing API trace. The rules have their tests
  (`oracle/tests/test_api_parity.py`).
- **Depth windows of the API.** The stages call pdfplumber from nested
  generators, so on a depth window a trace written through the API fails
  earlier or later than the stage by construction. Parity there is the
  sessions': for each family of windows of the adversarial sets,
  `js_api.py` bisects the first nesting at which a statement of a depth
  session raises `RecursionError` (`ApiDepth`, probes cached under
  `<cache>/corpus/js-api-depth/`), writes that PDF and its two neighbours
  and makes sessions of them, checking that one passes and the other
  raises. A family with no `RecursionError` up to the cap (6,000 levels,
  12,000 for `climit-*`) keeps only the stages' windows (listed in
  `meta.json`, `depth_api_first_failing`). `depth.jsonl` opens the 593
  windows by path (Node, `js/test/node/api-depth.test.ts`),
  `depth-bytes.jsonl` from their bytes (Node and the three browsers, page
  and worker, in batches of 25 sessions), without the members that copy the
  document through `dataclasses.asdict` (`structure_tree`, an element's
  `toDict`), whose deep copy of a document read from bytes is not modelled.

## Unit Generators And Replays

`oracle/unit/<module>.py` generates cases (fixed seed) with Python's
outputs; Rust replays them (`crates/pdftrace/tests/<replay>_unit.rs`,
through `pdftrace::unit::UnitData`). Each generator runs as a module of the
checkout whose Rust replays it (`python -m oracle.unit.<module>`) and writes
`<cache>/unit/<module>/<fingerprint>/`: `<name>.jsonl` files of typed JSON
lines and, last, `meta.json`, whose `sources` holds the SHA-256 of every
`oracle/` module the generator loaded and `environment` the oracle
environment. The fingerprint hashes both, so checkouts sharing the cache
never mix their data. A replay takes the most recent directory whose
sources are those of its checkout and fails without one, printing the
command that regenerates it. `python -m oracle.unit` regenerates whatever
is missing or stale (`--check` only reports, `--force` regenerates all).
Editing a module of `oracle/` therefore regenerates the data of every
generator that loads it; editing `canon.py`, `typed_json.py`, `corpus.py`,
`depth.py`, `unit/_common.py` or `stages/__init__.py` regenerates nearly
all of it (hours).

The same replays run compiled to `wasm32-wasip1` under Node's WASI
(`scripts/wasi-runner.mjs`, in `scripts/check.sh`): 32-bit `usize`, no host
libm, a browser's arithmetic. Tests that leave state in the process (the
void-call tests of `js_runtime_unit`) run there one per process.

| Generator | Checks | Replay |
|---|---|---|
| `canon` | `canon` and typed JSON, values nested 2·10^5 to 10^6 levels | `canon_unit` |
| `pycompat` | CPython semantics: `PyStr`, Unicode normalization, `repr`, `pow`, `atan2`, numeric `hash()`, recursion limits | `pycompat_unit` |
| `exceptions` | pdfminer's and pdfplumber's exception classes | `exceptions_unit` |
| `pdftypes` | the object model, references, streams, decryption, nesting at the recursion limits, `literal_name` as the first `str(x)` of its site | `pdftypes_unit` |
| `psparser_base`, `psparser` | `PSStackParser` on a file; `PDFContentParser` on every content stream of `all` and on 4,000 adversarial jobs, each replayed from the depth pdfminer ran it at | `psparser_base_unit`, `psparser_unit` |
| `utils` | pdfminer's `utils`, `casting`, `data_structures`, `pdfcolor`; pdfplumber's `ctm` | `utils_unit` |
| `layout` | `LT*` objects and the analysis under several `LAParams`, real and adversarial raw pages, Python's `id()` order | `layout_unit` |
| `converter` | the device calls of the interpreter replayed on `PDFPageAggregatorWithMarkedContent` without the interpreter (generated, depth boundaries; with `--corpus`, the first 20 pages of `all`) | `converter_unit` |
| `zlib`, `filters`, `ccitt`, `ccitt_corpus` | CPython's `zlib` on zlib 1.3; every filter, predictor and `PDFStream.get_data`, on generated chains and every stream of `all`; the CCITT decoder, and every `ccittfaxdecode` call pdfminer makes on `all` | `zlib_unit`, `filters_unit`, `ccitt_unit` |
| `crypto` | RC4, `unpad_aes`, `saslprep` (NFKC of Unicode 3.2.0), every standard security handler | `crypto_unit` |
| `cmaps`, `fonts` | the font tables, `encodingdb`, `cmapdb`, every predefined CMap, `CMapParser`; fonts from generated dictionaries and from `all`, with their depth boundaries | `cmaps_unit`, `fonts_unit` |
| `text_tables`, `text_tables_real` | pdfplumber's object model, clustering, geometry, `utils/text.py`, `table.py`, CPython's `list.sort`, NaN pages, user arguments of any type; the same operations on real pages | `text_tables_unit` |
| `document` | the `objects`, `streams`, `pages`, `psparser` traces on `adv-objects`, `adv-pages`, `micro` and small `quick` files, each in a forked process; `bytes % int` | `document_unit` |
| `interp`, `interp_deep` | the `interp_text`, `fonts_text`, `interp`, `fonts`, `layout` traces on `adv-interp`, `micro` and small `quick` files; each page interpreted from every depth near its deepest frame | `interp_unit` |
| `plumber`, `plumber_sweep`, `plumber_frames` | the four `plumber_*` traces on `adv-plumber`, `micro` and small `quick` files; `pdfplumber.open` arguments of any type; each page-layer operation run from every depth through the limit; the deepest depth of `process_object`, `decode_text`, `resolve_all` along both counters | `plumber_unit`, `plumber_sweep`, `plumber_frames` |
| `structure` | the `plumber_doc` traces on `adv-structure` and on the `quick` files with structure trees, metadata or annotations; codecs | `structure_unit` |
| `js_runtime` | what crosses the WebAssembly boundary: exception MROs, lazy CMaps and void calls, diagnostics, sessions of boundary operations (`pages.jsonl`) with their depth windows | `crates/wasm/tests/js_runtime_unit.rs`, `js/test/shared/tests.mjs` |
| `js_api` | the sessions of the public API and its depth windows | `js/test/shared/api-sessions.mjs` |

A Python result that depends on memory addresses (the order of a `set` of
NaNs: `hash(nan)` is the object's address) is never compared as is: the
port fixes an order CPython can give, the generator makes objects whose
hashes give that order and checks, by replaying `setobject.c` on every
`set` built, that CPython gave it; otherwise the case is dropped,
deterministically (`oracle/unit/_nan_objects.py`).

## Depth Windows

A recursion limit is checked where it is reached. For each recursive path
(page trees, `/Prev` and `/Length` chains, number and name trees,
outlines, `resolve_all`, Form XObject chains, nested operands, structure
elements, serializers), the adversarial generators bisect, in child
processes, the first nesting at which Python raises `RecursionError` for
each stage that checks it, and write the PDFs of a window around it
(`cases_depth`, files `depth-*`). The deepest point of each window is a
different path: `adv-interp` has 28 (a font, a predefined CMap, a char, an
inline image, a path, a color space, a tag, a warning, the first compile of
a pattern...). `cases_climit` (`climit-*`) does the same along the C-unit
counter (operands nested about 10,000 levels under `Tf`, `Do`, `CS`, `cs`,
a `/ColorSpace` resource, an inline image, at the bottom of Form chains of
0 to 300 levels, two-page variants where the second page runs the
specialized site).

The unit generators replay windows from one and two frames lower, sweep
operations from every depth through the limit (`interp_deep`,
`plumber_sweep`), and bisect the boundary operations' windows on the
statements of `js_pages_run.py` (`js_runtime`). A window bisected on an
operation that follows others on the same page makes sure the earlier ones
cached nothing at its depth (otherwise the bisection measures the earlier
one).

## Measuring A Frame Depth

The port models with constants the frames that standard-library calls take
when they do not depend on the input (`CMapDB._load_data`, `lru_cache`,
`re`, `json`, `copy.deepcopy`, `typing`). `oracle/depth_probe.py` measures
them: a profile hook (`sys.setprofile`) keeps two integers, the current and
the maximum depth, and copies the stack's names only at a new maximum.
Never copy the stack at every call: such a probe grows without bound. Run
every probe under explicit limits (`prlimit` takes bytes):

```sh
timeout 15m prlimit --as=4294967296 -- $PDFPLUMBER_ROOT/.venv/bin/python -m oracle.depth_probe cmaps
```

The measured constants are also checked at their thresholds by the unit
cases (`fonts`, `plumber_frames`).

## Mutation Testing

`cargo mutants` mutates the Rust code of a module (`<` into `<=`, a
constant, a branch removed...) and runs its replays. **Every surviving
mutant** is either killed by a new case or justified in writing (an
equivalent mutant, with the reason) next to the code or in the script that
lists it.

### Resource Guards

A mutant can loop without consuming its input, often while allocating.

- Run every `cargo mutants`, adversarial corpus generation or pass over a
  whole set through `scripts/heavy.sh` (one of `HEAVY_SLOTS` machine-wide
  slots, `HEAVY_SLOTS=1` on a shared machine, a systemd scope capped at
  5 GB without swap, `OOMPolicy=continue` so that the OOM killer kills the
  mutant's test binary, a caught mutant, and the run goes on). Use `-j 1
  --timeout 20`. Correct tests stay under 250 MB.
- The `mutants` profile (`--profile mutants`) is `release` with
  `debug_assertions`, which enable `psparser::ProgressGuard`: between two
  `seek`s a parser only reads forward (`read_position`), so a loop that
  stops progressing panics in a few steps. The guards are excluded from
  mutation (`-E "ProgressGuard|check_progress|read_position"`, `-E
  OutputGuard` for `pycompat::zlib`, `-E check_growth` for `cmapdb.rs`; the
  predictors' output ratio in `utils.rs`; the "progress guard"
  `debug_assert!`s of `utils/text.rs` and `table.rs`).
- Replays bound what a job may produce (no more objects than pdfminer, no
  more tokens than input bytes).

### Choosing The Tests

The tests of a module are its replays, given after `--`: `--
--package=pdftrace --test=<replay>` (cargo-mutants 27.1 runs `cargo test
--package=<mutated crate> -- ...`; check in `mutants.out/log/baseline.log`
that the command holds `--package=pdftrace --test=<replay>`). Shared files
take every replay that exercises them: `utils.rs`: `utils_unit`,
`layout_unit`, `filters_unit`, `crypto_unit`; `pdftypes.rs`:
`pdftypes_unit`, `filters_unit`, `document_unit`; `pdfparser.rs`,
`pdfdocument/*.rs`, `pdfpage.rs`, `pdfinternals.rs`, `bytes_format.rs`:
`document_unit`; `pdfinterp.rs`: `interp_unit`, `psparser_unit`,
`document_unit`; `converter.rs`, `pdfplumber/src/page.rs`:
`converter_unit`, `interp_unit`; `specialize.rs`: `pdftypes_unit`,
`psparser_unit`, `cmaps_unit`, `interp_unit`; `process_state.rs`,
`wasm/src/ops.rs`: `lazy_cmap`, `js_runtime_unit`; `crates/wasm/src`: the
`pdfplumber-wasm` tests (`--test=js_runtime_unit --test=boundary`).

```sh
HEAVY_SLOTS=1 scripts/heavy.sh cargo mutants -f crates/pdfminer/src/psparser.rs \
  -E "ProgressGuard|check_progress|read_position" \
  --profile mutants -j 1 --timeout 20 -- --package=pdftrace --test=psparser_unit
```

For a file of which only a part changed, `--in-diff <diff>` limits the run
to the changed lines.

### The Ok Pass

cargo-mutants recognizes a `Result` only by its name: for the alias
`pycompat::PyResult<T>` it generates `PyResult::new(..)` bodies, which never
compile ("unviable"), and never `Ok(..)`, the strongest mutation of a
function. `scripts/mutants-ok.sh` copies the checkout, rewrites
`PyResult<T>` as `Result<T, pycompat::PyErr>` in the files given with `-f`
(`crate::PyErr` inside `pycompat`) and runs only the `Ok(` mutants; with
`--in-diff`, the diff must come from the rewritten files. Where
`Ok(Default::default())` is unviable (a type without `Default`, such as
`PDFObject`), the body is replaced by hand, on a copy, by a concrete value
of each relevant variant, and the test must fail (not fail to compile).

```sh
scripts/heavy.sh scripts/mutants-ok.sh -f crates/pdfminer/src/arcfour.rs \
  --profile mutants -j 1 --timeout 60 -- --package=pdftrace --test=crypto_unit
```

### Hand Mutants

Some code cannot be mutated by cargo-mutants (a `let _g = ...?;` statement,
the wrapper of a property load, JavaScript). Scripts replace one exact text
at a time on a copy of the checkout, run the tests that can see it, and
restore the source; a survivor is killed by a new case or justified.

- `scripts/unit-site-mutants.py`: removes each C-unit site in turn (an
  `AttrSite` load becomes its getter, an `eval_entry`, `method_call` or
  `builtin_call` statement nothing), rebuilds, and runs `oracle.verify` of
  the `plumber_*` stages on the `climit-*` windows of `adv-interp` and
  `adv-plumber` and `js_runtime_unit`. A surviving site is killed by a
  window whose limit goes through it, or justified: a cheap or already
  cached getter (nothing deep runs while its units are held), a site
  dominated by another that always fails first, or a path the boundary
  cannot reach.
- `scripts/page-layer-mutants.py`: the C units of the page layer
  (`isinstance` with a tuple, `decode_text`'s `"".join` and `ord`,
  `resolve_all`, `process_object`'s `re.sub` and `.lower()`), the `typing`
  staircases, the structure matcher's mask argument and pdfinterp's
  `path_segment`, against `plumber_frames`, `plumber_unit`, `interp_unit`,
  `js_runtime_unit` and `boundary`. Accepted survivors: the transient units
  of `isinstance(o, (LTChar, LTTextContainer))`, of `(LTCurve,)` and of
  `.lower()` in `process_object` each need one unit, after the
  `isinstance` of `re._compile`, which needs two in the same call: none of
  them is ever the first to fail (Python takes them, the port too).
- `scripts/api-runtime-mutants.mjs`: the API's argument encoding (`null`
  and `undefined`, `Map`s, objects with no Python value), `search()` and
  `filter()` of what is not a pattern or a function, the finalizers' turns
  of the event loop, page and stream references owned by a document and a
  core, unsigned wasm32 addresses, CMaps the loader cannot provide, the
  worker client, structure elements, and in Rust the core's history bound
  and CMap availability. Accepted survivors: when the finalizers get their
  turn (every job, or a `MessageChannel` instead of `setImmediate`) and the
  sweep of dead stream entries change timing or memory only; a signed
  response length needs a response over 2 GiB; the frames of
  `search_pattern_error`'s `re.escape`/`re.compile` are never at the limit
  in a top-level statement.
- `scripts/js-api-mutants.mjs`: the API layer (`js/src/api.ts`,
  `api/structure.ts`, `search.ts`, `errors.ts`). Accepted survivors: the
  `unregister` of a pinned page or of a released document (the document
  holds the page; the entry finds its document released), the `close()`
  before `release()` of `await using` (nothing of a released document is
  observable), `named: null` for a pattern without named groups (the core
  reads `null` and an empty dict alike), and a matcher value sent along
  with a mask (the core never reads it then).
- `scripts/js-boundary-mutants.mjs`: the JavaScript boundary layer (stream
  references, lazy CMaps, history, page operations, diagnostics).

Equivalent survivors of the Rust boundary and of the performance paths are
noted at their sites (bit operations on disjoint flags, a capacity hint,
`int_hash` of 0, the shared-text threshold, a check that is never the
first past the limit).

## Check Scripts

- `scripts/check.sh` (about 40 minutes; through `scripts/heavy.sh`):
  `rustfmt`, `clippy -D warnings`, `oracle.upstream_refs`, the `--check` of
  the generated tables, `oracle.unit` (regenerates stale data), `cargo test
  --release` natively and on `wasm32-wasip1` (the tests that need their own
  process run one per WASI process), `scripts/wasm-depth-gate.sh`, the
  JavaScript package (CMap names, `dist/` without `wasm-opt`, `tsc`, vitest:
  runtime tests, the API sessions on the sources and on `dist`, the depth
  windows, lifetimes; the size budget), the package as a consumer gets it
  (`scripts/check-package.mjs`: files, notices, links, declarations,
  `import` and `require`), `oracle.api_parity` for the four `plumber_*`
  stages on `micro` and `adv-plumber` with the `wasm-opt`'d oracle module,
  and the oracle self-tests.
- `scripts/check-browsers.sh` (about 30 minutes): the package built as it
  ships, the bundler recipes (esbuild and Vite, in Chromium), then the
  runtime tests and the API sessions with the depth windows from bytes in
  Chromium, Firefox and WebKit, in the page and in a worker.
- `scripts/wasm-depth-gate.sh`: see [Wasm Parity](#wasm-parity).

## Parity Status

Traces through `pdftrace` (release), the 13 stages on `all` (1,853 files,
first 20 pages) and the five adversarial sets: 75 of 78 runs identical to
Python, no timeout, no crash. `oracle.wasm_parity` on `quick`: the
WebAssembly build gives the native traces for the 13 stages. Through the
public API (`oracle.api_parity`, the four `plumber_*` stages on the same six
sets): no `defect`; every other difference is a `core` difference of the
table below, a `filter` difference, or a depth `window`, and the 593 API
depth windows match Python through the sessions, in Node and the three
browsers.

| Known difference | Where | Why |
|---|---|---|
| the `ids` line of `pdfjs-issue19624-text.pdf` | `plumber_objects` on `all` (1,852/1,853) | two text boxes with the same content, which the key of the `id()` ranks cannot tell apart ([Layout Tie Order](architecture.md#layout-tie-order)) |
| NaN order in `cluster_objects` | `plumber_text` (16 files) and `plumber_tables` (12) on the `op-*`/`text-*` files of `adv-interp` | CPython orders a `set` of NaNs by address; Python itself changes from one process to another |
| the oracle of `096-text-TL.pdf` | `plumber_text` on `adv-interp` | Python needs more than the 5 GB cap of `heavy.sh` |
| `filter()` evaluated at the call | `plumber_doc` through the API (31 files of `all` and the adversarial sets) | the API runs the test in the `filter()` job: a page whose objects raise rejects `filter()` instead of raising at the first read ([Differences From Python](public-api.md#differences-from-python)) |
| depth windows through the API | 777 `depth-*`/`climit-*` traces of the adversarial sets | the stages call pdfplumber from nested generators: parity there is checked by the API's own depth sessions |

Not modelled, by design or out of reach: Python's default logging
configuration ([Logging Disabled](#logging-disabled)), the deep copy of a
document read from bytes by `structure_tree`, the first unspecialized call
of a builtin outside the modelled sites, the `typing` subscriptions of
nested functions in `table.py`, `utils/text.py` and `structure.py`. Parity
is checked on the first 20 pages of each file of `all` and on generated
corpora; there is no fuzzing.

## Definition Of Done

A change to a module is done when:

1. It is a faithful port: one Rust module per Python module, the upstream
   reference on every function, no special case
   ([Porting Rules](architecture.md#porting-rules)).
2. Its stages are identical to Python on `all` (first 20 pages) and on the
   adversarial sets, apart from oracle timeouts (listed with
   `--allow-timeouts`) and the known differences above.
3. Its unit replays pass natively and on `wasm32-wasip1`, with the cases
   that pin the change generated by Python.
4. Mutation testing leaves no unjustified survivor (`cargo mutants`, the
   `Ok(..)` pass, the hand mutants of the code they cover).
5. For the JavaScript API: every member and option of
   [`public-api.md`](public-api.md) checked against Python through the API
   (sessions and traces), in Node, Chromium, Firefox, WebKit and worker
   mode; performance measured before and after, with no trace changed.
6. `scripts/check.sh` and `scripts/check-browsers.sh` pass.
