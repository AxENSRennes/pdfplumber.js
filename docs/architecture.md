# Architecture

pdfplumber.js is a port of Python pdfplumber and pdfminer.six to Rust,
compiled to WebAssembly behind a JavaScript API. This document describes how
the code is organised and the rules every module follows.
[`verification.md`](verification.md) describes how parity with Python is
checked, [`js-runtime.md`](js-runtime.md) the WebAssembly boundary and the
JavaScript package, [`public-api.md`](public-api.md) the public API.

## Upstream Sources And Python Version

- The specification is **pdfplumber 0.11.9** (`upstream/pdfplumber-python`,
  commit `9804153`) on **pdfminer.six 20260107** (`upstream/pdfminer-six`,
  tag `20260107`), run by **CPython 3.12** (`.venv`, `.python-version`). The
  port follows exactly the code the oracle runs; never port from another
  version.
- pdfplumber's output depends on the Python minor version. Python 3.12
  changed `sum()` of floats to a compensated (Neumaier) sum: with a naive
  sum, about half of the real pages with tables get table coordinates that
  differ in the last bits, and some a different table structure. On a
  sample of 316 real pages, Python 3.12, 3.13 and 3.14 give identical
  results and 3.11 does not. The public contract is "pdfplumber under
  Python ≥ 3.12"; the tooling is pinned to 3.12.
- The last bits of `float ** float` come from the C library. The reference
  is the oracle machine's glibc 2.39 on a CPU with FMA (its `__pow_fma`
  variant); the non-FMA variant of the same glibc differs on about 0.05 %
  of inputs, other libms (macOS, FreeBSD) more often. The port carries its
  own `pow` (`pycompat::libm`), so the result does not depend on the host.

## Repository Layout

```
Cargo.toml                 Rust workspace
crates/
  pycompat/                Python semantics shared by every crate (numbers, str, sort, recursion, zlib...)
  pdfminer/                the port of pdfminer.six, one Rust module per Python module
  pdfplumber/              the port of pdfplumber, same structure (utils/ included)
  pdftrace/                canonical traces: the `pdftrace` binary, stage writers, unit replays (tests/)
  wasm/                    the WebAssembly module: ABI, result encoding, documents and pages behind handles
js/                        the npm package: runtime, public API, types, Node and browser tests, benchmarks
  trace/                   the plumber_* stage traces written through the public API
oracle/                    the Python side: stages, traces, cache, corpus sets, unit generators, adversarial corpora
corpus/                    real PDFs and provenance manifests (large PDFs are downloaded, not committed)
upstream/                  pdfplumber and pdfminer sources (submodules), pdf.js test PDFs (submodule),
                           the glibc and Arm sources of the ported libm functions
scripts/                   build, checks, heavy-job wrapper, mutation scripts, profilers (perf/)
docs/                      this documentation
```

## Module Map

| Python | Rust |
|---|---|
| `pdfminer/psparser.py` | `crates/pdfminer/src/psparser.rs` (+ `psparser_symbols.rs`, the names interned at import) |
| `pdfminer/pdftypes.py`, `pdfparser.py`, `pdfpage.py` | `pdftypes.rs`, `pdfparser.rs`, `pdfpage.rs` |
| `pdfminer/pdfdocument.py` | `pdfdocument/` (`mod.rs`, `xref.rs`, `security.rs`, `pyval.rs`) |
| `pdfminer/pdffont.py` | `pdffont/` (`mod.rs`, `type1.rs`, `cff.rs`, `truetype.rs`, `unpack.rs`, `frames.rs`, `tables.rs`) |
| `pdfminer/cmapdb.py`, `encodingdb.py`, `glyphlist.py`, `latin_enc.py`, `fontmetrics.py` | modules of the same names (the tables generated, see [Upstream Data](#upstream-data)) |
| `pdfminer/pdfinterp.py`, `pdfdevice.py`, `converter.py`, `layout.py` | modules of the same names |
| `pdfminer/lzw.py`, `ascii85.py`, `runlength.py`, `ccitt.py`, `arcfour.py`, `_saslprep.py` | `lzw.rs`, `ascii85.rs`, `runlength.rs`, `ccitt.rs`, `arcfour.rs`, `saslprep.rs` |
| `pdfminer/utils.py`, `casting.py`, `data_structures.py`, `pdfcolor.py`, `settings.py`, `psexceptions.py`, `pdfexceptions.py` | modules of the same names |
| `pdfplumber/pdf.py`, `page.py`, `container.py`, `convert.py`, `structure.py`, `table.py`, `ctm.py` | `crates/pdfplumber/src/` modules of the same names |
| `pdfplumber/utils/*.py` | `crates/pdfplumber/src/utils/` (`clustering`, `geometry`, `generic`, `text`, `pdfinternals`, `exceptions`) |
| pdfplumber's dict objects and Python values | `crates/pdfplumber/src/obj.rs` |
| `zlib`, `json`, `csv`, `base64`, `codecs`, `unicodedata`, `logging`, `warnings`, `typing`'s cache, `re`'s cache, `list.sort`, numbers | `crates/pycompat` |

Modules with no Python counterpart: `pdfminer::process_state` (the modelled
state of a Python process, see [Process State](#process-state)),
`pdfminer::log_text` (the text of pdfminer's log records), and the
`pdftrace` and `wasm` crates.

## Porting Rules

1. **One Python module, one Rust module of the same name**
   (`pdfminer/psparser.py` → `crates/pdfminer/src/psparser.rs`,
   `pdfplumber/utils/text.py` → `crates/pdfplumber/src/utils/text.rs`). A
   module too large for one file becomes a folder (`pdfdocument/mod.rs` +
   `pdfdocument/security.rs`), said in its header.
2. **The same names** for types, functions and methods (in Rust case).
   Every ported function or method carries its upstream reference in a
   comment: the line of its `def` followed by its qualified name,
   `// pdfminer/pdfinterp.py:1282 PDFPageInterpreter.do_Tj` (a range names
   everything it covers: `:327-329 C.A, C.B, C.C`).
   `python -m oracle.upstream_refs`, run by `scripts/check.sh`, checks every
   such reference in `crates/` and `oracle/` against `upstream/`.
3. **The same algorithm, in the same evaluation order, with the same error
   cases.** An optimization is allowed when every trace stays identical; it
   is commented at its site and covered by mutation testing.
4. **No special cases.** No font name, file fingerprint, threshold or
   constant without an upstream counterpart cited next to it. A difference
   with Python is fixed at its cause.
5. **Out of scope**, not ported: the text, HTML, XML and hOCR converters of
   `converter.py` and `PDFConverter`, `TagExtractor` of `pdfdevice.py`,
   `PDFDevice.__repr__` and the context-manager methods, `high_level.py`,
   `image.py` (`ImageWriter`), pdfplumber's `display.py` (rendering),
   `repair.py` (Ghostscript) and `cli.py`. Everything else that
   `pdfplumber.open(...)` uses is ported, `laparams`, tagged structure,
   annotations and encryption included. A path that reaches something not
   ported raises `NotPorted` (`NotImplemented` in JavaScript).

The port's own dependencies are few. A proven crate is acceptable for a
standard algorithm with a single correct result (MD5, SHA-2, AES), checked
against CPython; it is not when the behavior to reproduce is CPython's or
zlib's (zlib's errors and partial output: `pycompat::zlib` ports zlib 1.3's
inflate; Unicode tables: generated from CPython 3.12's `unicodedata`). The
core does not depend on `wasm32`, JavaScript or an async runtime, so the
same crates can serve a native module.

## Value Model

Python is dynamic; the port reproduces that dynamism where it is
observable, and only there.

### PDF Objects

`pdftypes::PDFObject` is an enum: `Null`, `Bool`, `Int(PyInt)`,
`Float(f64)`, `Bytes`, `Str(PyStr)` (a Python `str`, made only by the
in-place decoding of metadata), `Literal` (a `PSLiteral`, its name a `str`
or `bytes` as in Python), `Keyword`, `List`, `Dict` (ordered, `str` keys),
`Stream(Rc<PDFStream>)` and `Ref` (`PDFObjRef`). `PSLiteral` and
`PSKeyword` are interned as in Python, so identity holds.

Object ids (`PDFXRef.offsets` keys, `_cached_objs`, `getobj`'s argument,
`PDFStream.objid`) are any hashable Python value: `PDFXRefFallback` stores
an object stream's header as it reads it (`5.5`, `True`, a literal...).
`pdftypes::ObjId` keeps the original object and Python's `hash()` and `==`
(`5 == 5.0 == True`; literals and keywords by identity; `bytes` by value; a
list or dict raises `TypeError("unhashable type: ...")`). `PDFObjRef.objid`
stays a `PyInt` (pdfminer makes references through `safe_int`). The
decryption function receives the `ObjId` as Python does: `struct.pack("<L",
objid)` (RC4, AES-128) raises `struct.error` for an id that is not an
integer.

### Numbers

Python distinguishes `int` and `float`, and the distinction changes results
(`sum()`, `int / int`, `repr`, table keys). `pycompat::PyInt` (an `i64` with
an arbitrary-precision fallback) and `pycompat::Num` (`Int` or `Float`)
follow Python's arithmetic. Code that upstream only ever sees floats may use
`f64`, with a comment saying why. Real PDFs rarely show the difference
(pdfplumber produces floats), generated inputs do.

### Strings And Surrogates

A Python `str` is a `pycompat::PyStr`: the bytes of
`s.encode("utf-8", "surrogatepass")`. A lone surrogate, which pdfminer
produces with `chr(cid)`, fits as in Python; ordering, equality,
concatenation and `in` are those of the bytes; `as_str()` gives the `&str`
of a text without surrogates. Object texts (`LTChar`, the values and keys
of pdfplumber's objects, `TextMap`, table cells) and exception messages
that quote a user's `str` use it; strings that cannot hold a surrogate
(class names, pdfminer keywords) stay `String`. Toward JavaScript,
`to_utf16`/`from_utf16`: a lone surrogate is one UTF-16 unit; a valid pair
reads back as one character.

### NaN Identity

Python compares container items and `set`/`dict` keys with an identity
shortcut (`x is y or x == y`). A NaN of the object model carries the
identity of its Python object in its payload: `PyValue::float` makes a new
object, a clone is the same object (`obj.rs`, "NaN identity").
`PyValue::Float` holds a `PyFloat` with a private field, so outside
`obj.rs` a float is made by `PyValue::float` or
`PyValue::float_with_identity` (test cases), never as a bare NaN that every
value would share.

The order of NaNs in a `set` (`cluster_objects`) follows their memory
address in CPython; the port puts NaN values last. That order is a known
difference ([Parity Status](verification.md#parity-status)).

### Sequence Sizes

`seq * n` (`str`, `bytes`, `tuple`, `list`) raises CPython's errors
(`OverflowError`, `MemoryError` beyond `PY_SSIZE_T_MAX`) and builds any
result up to 4 GiB (`obj::MAX_REPEAT_BYTES`). Beyond that the port raises
`MemoryError` where CPython would attempt the allocation (refused on the
oracle machine from about 26 GiB): `wasm32` cannot address more. The
layout-text loops (`[x] * n`, `range(n)` adding one line per turn) keep a
guard of 10^7 turns (`utils/text.rs`, `MAX_REPEAT`) where Python would loop
until memory runs out.

### pdfplumber Objects

`T_obj` values are ordered dicts with `str` keys, as in Python, keys in
Python's order (the assignment order of the `LT*` objects' `__dict__`, then
what `process_object` adds). Frequent attributes may have a fast path, but
the dict view stays exact (`extra_attrs`, `to_dict` and the JavaScript API
depend on it). An annotation's `/P` is the page itself (`PyValue::Page`,
`obj::PageRef`: `<Page:n>`, equal only to itself).

### Exceptions

`pycompat::PyErr { kind, message }`: `kind` is the Python class (`PSEOF`,
`PSSyntaxError`, `PDFSyntaxError`, `KeyError`, `TypeError`,
`AssertionError`, `ValueError`, `zlib.error`, `struct.error`,
`UnicodeDecodeError`, `RecursionError`...) with its hierarchy (`except
PSException:` catches `PSEOF`), and the message is `str(e)`. When the
message holds a lone surrogate, `message` is a copy with U+FFFD and only
`PyErr::py_message()` is exact: whatever shows `str(e)` (traces, the
JavaScript boundary) reads `py_message()`. An exception raised with
another as its only argument (`PdfminerException(e)`,
`MalformedPDFException(e)`) keeps it (`PyErr::wrapping`, `PyErr::cause`);
traces write it as `!PdfminerException("...") cause !PDFSyntaxError("...")`.
Upstream `try/except` blocks become `match`es on these kinds. Python
exceptions that escape (an `AssertionError`, an `IndexError`, a
`KeyError`...) are part of the behavior to reproduce.

### Warnings And Logging

Warnings never change a result. Their text goes to a diagnostics channel
(`pycompat::diagnostics`, `onWarning` in JavaScript, see
[`js-runtime.md`](js-runtime.md#diagnostics)); their Python frames count
for the recursion depth. The reference configuration is pdfminer's logging
off (`logging.disable`, or `setLevel(ERROR)` on the `pdfminer` logger,
which costs the same frames): in Python's default configuration a
`log.warning` goes through `logging.lastResort`, deeper, and formats the
`%r` of its arguments, which is not modelled
([Logging Disabled](verification.md#logging-disabled)).

What reaches the channel: pdfplumber's two `logger.warning` calls (metadata
in `PDF.__init__`, "Unknown attribute class" in `PDFStructTree`), its
`warnings.warn` of `Page.annots` (once per text, category and line, by
`pycompat::warnings`, which keeps the default filters' registry as process
state), and the text of every `log.warning` of pdfminer, formatted as
`logging` formats the record (`pycompat::logging::Logger::log`,
`pdfminer::log_text`), only when someone collects it. pdfminer's
`log.debug` calls count their frames and send nothing.

`pdfminer.settings.STRICT` is `False`, as in the oracle.

## Python Semantics

Everything that imitates Python lives once in `crates/pycompat`, tested
against CPython by the unit generators:

- `sum()` of 3.12 (exact ints; the first float added plainly, the next
  ones with Neumaier compensation; `+ c` at the end if finite and nonzero),
  `min`/`max` (the first on ties, so the sign of zero), stable sorts and
  CPython's timsort (`listsort`, NaN keys included).
- `float ** float` with glibc 2.39's `pow` (FMA variant, from Arm's
  optimized-routines) and `math.atan2` (CPython's `m_atan2`, then glibc's
  `atan2`), both in `pycompat::libm`: `f64::powf` and `f64::atan2` are other
  functions in Wasm.
- Numeric `hash()` and numbers as dict keys (`NumKey`: `3` finds `3.0`),
  `round()` (exact decimal banker's rounding), `float.__repr__` (exponent
  outside [1e-4, 1e16)), `int()` and `str()` of ints with the 4,300-digit
  limit, int to float with `OverflowError`, true and floor division,
  Python's `%`.
- `str.split()`/`strip()` and character classes (`\s` of a bytes pattern is
  `[\t\n\v\f\r ]`, `str.isspace()` is Unicode's), Unicode normalization
  with CPython 3.12's tables, JSON's `ensure_ascii`, `bytes % int` (with the
  `MemoryError` of a width the allocator refuses), strict `utf-8` and
  `utf-16` decoders (`codecs`), `json.dumps` (both the C encoder and the
  Python encoder used with `indent`), `csv.writer`, `base64`.
- zlib 1.3's inflate with CPython's `zlib.decompress` and `decompressobj`
  semantics (`pycompat::zlib`).
- The recursion counters (`recursion`), the frames of a `logging` call
  (`logging`), `re`'s pattern cache (`re_cache`), the `typing` subscription
  cache (`typing_cache`), call-site specialization (`specialize`) and
  CPython's argument binding (`call`: "got multiple values for argument",
  "got an unexpected keyword argument" formatted with `%S`, "argument after
  ** must be a mapping").
- pdfminer reads files by blocks of 4,096 bytes and some behaviors depend
  on it (a `\` escape followed by `\r\n` split across two blocks): the port
  keeps the same blocks.

## Recursion Depth

Python raises `RecursionError` from two counters, both reproduced by
`pycompat::recursion` (`Depth`, per thread): Python frames (at most
`sys.getrecursionlimit()`, 1,000) and C recursion units (`C_RECURSION_LIMIT`,
10,000). A PDF nested near these limits fails at the same level, on the same
path, in the port and in Python.

### Frames And C Units

- Structures that Python builds without recursion (a content stream of 10^6
  `[`) are freed, compared, written and converted without recursion in Rust
  too (`PDFObject`, `PyValue`, typed JSON, `canon`). Where Python recurses
  (Form XObjects: `execute` → `do_Do` → `render_contents` → `execute`), Rust
  recurses as well, up to Python's limit.
- A ported Python function takes its frame (`py_frame()`) for its duration;
  a function that returns at once is only checked (`leaf(n)`). A call from C
  (the `__init__` of a called class, a generator resumed by `list()`,
  `join()` or `extend()`, a `re.sub` callback, `map`, `filter`, `sorted(key=)`)
  also takes its units (`eval_entry()`, `c_called_leaf()`). A class call is
  `class_call`: the unit of `_PyObject_MakeTpCall` plus the evaluation-loop
  entry of `__init__`, three units. A generator takes its frame when created
  and at each resume. A comprehension takes no frame (PEP 709).
- `repr()` and `str()` of a PDF object take CPython's C units at each level;
  `resolve_all` and `decipher_all` one frame per level.
- `execute`'s `func(*args)` (`CALL_FUNCTION_EX`) holds two units for the
  whole nested Form; at the bottom of a chain of `d` Forms, the recursive
  `repr` of the content parser's error message fails `2d` levels earlier.
- `isinstance(x, (A, B))`, once specialized, holds one transient unit while
  it checks the tuple (`isinstance_tuple`: `resolve_all` of a non-reference,
  `resolve_and_decode`, `process_object`). `decode_text`'s `"".join` holds a
  unit around the generator, `ord(c)` takes one per character, and
  `str(s[2:], "utf-16be", "ignore")` holds one.
- Logging calls take their frames through `pycompat::logging`: two, three at
  the first call of a level on a logger since `logging.disable()` (which
  empties the caches).
- Standard-library costs that do not depend on the input are constants
  measured with `oracle/depth_probe.py` ([Measuring A Frame
  Depth](verification.md#measuring-a-frame-depth)): `CMapDB._load_data`
  (4 frames for an absent name, 8 at the first load of a file), the
  `lru_cache` of `Page.__init__`, `re.sub` (2 frames, 11 at the first
  compile: `EOL_COMPILE_DEPTH`), the first compile of `paint_path`'s
  `re.finditer` (`SUBPATH_COMPILE_DEPTH = 8`), `json.dumps`, `csv`,
  `dataclasses.asdict`, `copy.deepcopy`, `reprlib.recursive_repr`.
- A frame or unit that no input can make the first one past the limit is
  not modelled, and the source says so: the C units of class calls,
  `super()` calls and generator resumes are never decisive below 10,000
  (at most three units per Python frame), the comparisons of `obj.rs` never
  reach the C limit before `resolve_all`'s frames do, and a check dominated
  by a deeper one in the same frame changes nothing.

### Call-Site Specialization

CPython 3.12's adaptive interpreter runs an instruction in its generic form
the first time, then specializes it. Where the generic form holds C units
for the whole call, the first execution in a process fails one level
earlier than the next ones. `pycompat::specialize` models the sites whose
call can come near a limit; the state of each site is process state:

- `CallSite`: `return str(x)` of `literal_name` (psparser.py:121,
  `psparser::LITERAL_NAME_STR`; `str` has no `tp_vectorcall` in 3.12, so its
  generic `CALL` goes through `_PyObject_MakeTpCall`), `str(s)` in
  `decode_text` (`DECODE_TEXT_STR`), and the builtin calls of the page layer
  (`getattr(self, t + "s")`, `CallSite::builtin_call`).
- `AttrSite`: a property load (`LOAD_ATTR`). The first execution goes
  through `PyObject_GetAttr` → `property.__get__`, which calls the getter
  from C and holds two units for the whole getter, page interpretation
  included; once specialized, the getter frame is pushed inline. The
  specialization is keyed by the object's class (reading `Container.chars`
  on a `Page`, then on a `CroppedPage`, deoptimizes the site), with 52
  misses before a new attempt and an exponential backoff after a failed
  one. Every property load of the ported pdfplumber code that can
  interpret a page is a site.
- `typing_cache`: pdfplumber evaluates the annotations of nested functions
  at each call of the enclosing function, and each `typing` subscription
  goes through `typing`'s `lru_cache`: a staircase of frames and C units on
  a miss (the first time in a process) and on a hit (`CStaircase`,
  `Subscription::with_c`), measured by `oracle.depth_probe typing` and
  `typing-c`.

`oracle/depth.py` (`executed_call_sites`) reads the state of a site from the
adaptive counter of its instruction; `oracle/codegen/py_symbols.py` checks
that the oracle's own imports execute no modelled site.

### Process State

Some state belongs to the Python process, not to a document: the interned
names (`PSSymbolTable`: interning a new name runs `PSLiteral.__init__` from
C, one more frame than a known name), `re`'s compiled patterns, the caches
of the loggers, the call and attribute sites, the `typing` cache,
`CMapDB`'s caches and the warnings registry. At start, a process holds what
its imports interned (`import pdfplumber`: `psparser_symbols.rs`; `python
-m oracle.trace`: `pdftrace::stages::py_symbols`, generated by
`oracle/codegen/py_symbols.py`).

`pdfminer::process_state` captures and restores all of it at once. The
trace harness starts each trace from a new process's state, as the oracle
runs each trace in a new process (`pdftrace::stages::py_process`). The
WebAssembly boundary restores it before re-running a void call
([Void Calls And Replay](js-runtime.md#void-calls-and-replay)), so the
re-run starts from the state Python's single run would have.

### Where Depth Is Counted

Counting is in place wherever a document is read or a page interpreted:

- `psparser` (every parser method, `_add_token`, `literal_name`, interning),
  `pdftypes` (`resolve1`, `dict_value`..., `PDFStream.__getitem__`, `get`,
  `get_filters`, `decode`, `get_data`), the filters (`lzwdecode`,
  `ascii85decode`, `asciihexdecode`, `rldecode`, predictors,
  `ccittfaxdecode`, `decompress_corrupted`), `pdfparser`, `pdfdocument`
  (`getobj`, xref loading, `lookup`, outlines, page labels), `pdfpage`
  (`create_pages` and each generator of its `yield from` chain) and
  `PDFContentParser`. A loop (`/Prev`, object streams, a `/Length` equal to
  its stream, page or outline trees, reference cycles) ends in
  `RecursionError` as in Python, and a finite chain fails at the same level.
- `pdffont` and `cmapdb` (`CMapDB`, `CMapParser`); `pdffont::frames` takes
  the frames of the code it calls without counting (`casting`, `cmapdb`
  lookups, `encodingdb`).
- `pdfinterp` and `pdfdevice` (every method; the `casting` and `utils`
  helpers as leaves; `PDFStream` methods called from C; the compile of
  `get_inline_data`'s `re.sub`), `converter`, pdfplumber's device
  (`page.rs`), the constructors of the raw layout objects and
  `decode_text`. The layout analysis (`group_objects`, `group_textlines`,
  `group_textboxes`) takes one frame each, not its callees': `end_page` is
  never near the limit. `analyze`, `get_text` and `IndexAssigner.run`
  recurse and count.
- The pdfplumber page layer: every ported function and property takes its
  frame; calls from C take their units (`map`, `filter`, `dict(...)`,
  `list(genexpr)`, `"".join(genexpr)`, `sorted(key=...)`); class calls
  (`PDF`, `Page`, `CroppedPage`, `FilteredPage`, `LAParams`,
  `PDFResourceManager`, `PDFStructTree`, `PDFStructElement`, `Serializer`)
  go through `class_call`; generators (`iter_pages`, `iter_layout_objects`,
  `_find_all`, `all_mcids`) take a frame at each resume. `table.py`,
  `utils/text.py`, `utils/clustering.py` and `utils/geometry.py` count
  frames only on the paths that lead to a page property that interprets the
  page (`TableFinder.__init__`, `get_edges`, `Table.extract`,
  `test_proposed_bbox`); elsewhere they never recurse and run at user level.

The harness sets the starting depth (`with_depth`) to the one the oracle
measured at the same call point; ported code never sets it.

Not modelled, documented where they would apply: Python's default logging
configuration; the frames of `warnings.warn`; the extra unit of a builtin's
first, unspecialized call outside the modelled sites; the `typing`
subscriptions of nested functions in `table.py`, `utils/text.py` and
`structure.py` (no window reaches them at the deepest point); the
constructors of `PDFPage`, `PDFParser` and the xref classes, which count two
units per class call instead of three; the frames of `add_cid2unichr` and
`choplist` under `CMapParser.do_keyword`; the import of the UTF-16 codec at
a process's first `bytes.decode("UTF-16BE")`.

### Stack Size

Where Python recurses, Rust recurses too, and Rust frames are larger. The
longest Form chain Python accepts (about 330 levels) takes about 1.2 MiB of
stack in the `dist` WebAssembly profile (3.7 KiB per level), more than the
1 MiB `wasm-ld` reserves by default. The WebAssembly module and the
`wasm32` builds of `pdftrace` therefore get an 8 MiB stack
(`crates/wasm/build.rs`, `crates/pdftrace/build.rs`), and
`scripts/wasm-depth-gate.sh` (run by `scripts/check.sh`) runs the `dist`
profile on the depth windows of the adversarial corpora and requires the
native traces. Natively, the `release` profile needs a thread of about
1.5 to 2 MiB (the tests run on 2 MiB threads); a host calling the core from
a smaller thread must enlarge it.

## Reference Resolution And Parser Re-entrance

- `PDFParser.do_keyword(KEYWORD_STREAM)` reads `int_value(dic["Length"])`
  when `fallback` is false and `dict_value(dic)` in every case; a reference
  there goes through `PDFDocument.getobj` → `_getobj_parse`, which reuses
  **the same** parser (`self._parser.seek(pos)`, whose `reset()` clears the
  outer `nextobject`'s context and stack) while it is inside `do_keyword`.
  The document keeps its parser in a slot (`RefCell<Option<Box<PDFParser>>>`);
  an operation that reads the file takes it out
  (`PDFDocument::with_parser`), and a resolution started while the parser
  runs (`do_keyword`, a trailer's `dict_value`, the `get_data` of an xref
  or object stream read during `__init__`, `/Prev`) **lends** it back
  (`pdfparser::lend`): its state is swapped with a stand-in and put back in
  the slot for the resolution, where a nested `getobj` finds it, then taken
  again with what the nested call left, as in Python. Resolutions go through
  the ordinary interface (`PDFObjRef.resolve`, `PDFResolver`).
- The file: `PDFParser` seeks and reads with values from the file
  (`/Length`, `/Prev`, offsets). An `io.BufferedReader` (`open(path, "rb")`)
  and an `io.BytesIO` raise different errors for a negative or huge
  position or length: `pdfparser::FileKind` chooses (a document opened by
  path in Node is a file; one opened from bytes a `BytesIO`). A
  `BufferedReader.read(n)` beyond its buffer allocates `n` bytes first; the
  port makes the same allocation (virtual memory, never touched) and raises
  `MemoryError` when it fails. Not modelled: the `OSError` of a seek beyond
  the file system's maximum file size.
- `PDFDocument.__init__` resolves references (`Encrypt`, `ID`, `Info`,
  `Root`): the document is created in an `Rc` with interior state before its
  `__init__` logic runs, so that `PDFObjRef.resolve` (a `Weak<dyn
  PDFResolver>`) reaches it during initialization.
- `CMapParser.do_keyword` calls `CMapDB.get_cmap` (`usecmap`)
  synchronously: the predefined CMap provider is synchronous for the core;
  lazy loading in a browser happens around a call, not inside it
  ([CMaps](js-runtime.md#cmaps)).
- `Type1FontHeaderParser` puts `(cid, name)` pairs in `results`, `cid` any
  Python int: the port keeps them in the parser's state
  (`Type1Header::pairs`, in order) without changing `psparser`.
- `pdffont::get_font` is the part of `PDFResourceManager.get_font` that
  creates the font and takes its frame; the cache by `objid` stays in
  `PDFResourceManager` (`pdfinterp`).

## Interpreter, Devices And Converter

### Driving Pages

Pages are interpreted as pdfplumber interprets them (`Page.layout`,
pdfplumber/page.py:256; `pdfplumber::page::page_layout`): one
`PDFResourceManager` per document (`caching=True`, the font cache of the
document), and for each page a `PDFPageAggregatorWithMarkedContent` and a
`PDFPageInterpreter`, then `process_page`. A device exception, a
`RecursionError` included, stops that page only. The stages `fonts`,
`interp` and `layout` of `pdftrace` call the same `page_layout`.

The resource manager is document state. A missing predefined CMap does not
raise inside the interpreter: pdfminer replaces it with an empty `CMap()`,
which the document's font cache keeps, and the provider records the name;
the call is then void and re-run with the CMap (see
[CMaps](js-runtime.md#cmaps); `crates/pdftrace/tests/lazy_cmap.rs` checks
the re-run through the interpreter, at the recursion boundary).

### Classes, Overrides And super()

A Python class is a struct of attributes plus accessor traits
(`PDFDevice::attrs`, `LayoutAnalyzer::analyzer`,
`PageAggregator::aggregator`, `MarkedContent::marked`). A method is a trait
method whose body is a generic free function in its class's module
(`pdfdevice::device`, `pdfdevice::text_device`, `converter::layout_analyzer`,
`converter::page_aggregator`, `pdfplumber::page::marked_content`); a
subclass calls its base's function as Python calls `super()`, and calls on
`self` (`render_char` from `render_string_horizontal`, `paint_path` per
subpath, `receive_layout`) reach the most derived class, the tracing
devices of `pdftrace` included. Constructors that are a Python class call
(`PDFPageAggregator::create`, `PDFPageAggregatorWithMarkedContent::create`)
take the frames of the `__init__` chain; `new` takes none.

The interpreter's `execute` builds its operator table as Python's `"do_" +
name.replace(...)` does: same names and arities (`T_a`, `_q` and `q\x80`
included). Operands are popped as Python pops them for any operand count
or type. Two state differences after a device exception are not
observable (the interpreter catches nothing a device raises, and pdfplumber
drops the device): `begin_figure` pushes the current item after building
the figure, and `end_page` moves the page to `receive_layout`. For a device
other than pdfplumber's, a non-numeric Form `/Matrix` raises `TypeError` in
`do_Do` (pdfplumber's `begin_figure` raises it first).

### Inline Image Names

pdfminer names an inline image `str(id(obj))` (pdfminer/pdfinterp.py:1309
`PDFPageInterpreter.do_EI`), a memory address. The port names it
`inline-<k>` (`pdfinterp::inline_image_name`; `k` counts the inline images
of one page's interpretation, shared with the sub-interpreters of its
Forms), and the traces write `inline{"n": k}`. The name reaches
`LTImage.name`, the `LTFigure` around it and pdfplumber's image `name`. It
repeats across pages (and after a page's caches are flushed), and an image
XObject a PDF names `inline-0` gets the same name as a page's first inline
image: a documented difference
([Not Ported](public-api.md#not-ported)).

### Layout Tie Order

pdfminer's layout analysis breaks ties between box pairs at equal distance
by `id()`, a memory address. The library and the JavaScript API rank them
by creation order (`layout::CreationOrder`), which gives one of Python's
outcomes. Python does not agree with itself on those pages: the `id()`
order moves with the hash seed and every allocation of the process.

Parity is checked exactly anyway: the Python `layout` and `plumber_objects`
stages write the rank of the `id()`s of their analysis (`ids` lines), the
stage modules declare `ORACLE_INPUT`, and `oracle.verify` hands the
oracle's trace to `pdftrace --oracle-dir`, which analyzes with an injected
`IdSource` (`pdftrace::layout_dump::RecordedIds`; `ContentIds` and
`OpenOptions::ids` for `plumber_objects`, which identifies a box by the
sorted fingerprints of its chars). Ids beyond the recorded ranks are fresh
and increasing; only their order matters. The WebAssembly module built with
the `oracle-hooks` feature takes the same ranks for the traces written
through the API.

How often `CreationOrder` agrees with Python (`pdftrace --stage layout`
without `--oracle-dir`, page by page, `ids` lines left out, on the first 20
pages of the `all` set, four Python draws: the cache and `PYTHONHASHSEED` 1
to 3): Python is stable on 93.6 % of the pages; on those, `CreationOrder`
gives Python's result on 98.6 %; on the others, one of Python's draws on
about half. A statistic, not a parity criterion; it is why the library
keeps `CreationOrder` (`CONVERTER_CREATION_ORDER=1 cargo test --release -p
pdftrace --test converter_unit corpus` measures it on the replayed corpus).

From one oracle cache build to the next, the `ids` lines change, and so does
the analysis of the pages with ties (a rebuild of the `layout` cache on `all`
and the adversarial sets changed 87 of 3,463 traces beyond their `ids`
lines); nothing else does. Parity holds for whichever draw the cache holds,
since `pdftrace` replays it. One file can fall out of reach of the content
key, depending on the draw: `pdfjs-issue19624-text.pdf`, whose duplicated
text boxes (fake bold) have the same content.

## The pdfplumber Page Layer

### Opening, Pages And Caches

- `PDF::open` follows `PDF.open`/`__init__`: `LAParams(**laparams)` (its
  `TypeError`s), the password, pdfminer's errors wrapped in
  `PdfminerException`. Options: `pages`, `laparams`, `password`,
  `strict_metadata`, `unicode_norm`, `raise_unicode_errors`; `repair=True`
  raises `NotPorted`. pdfminer reads the password only for an encrypted
  document.
- `pdf::Source`: `Path`, a file the `PDF` opened itself (`close()` closes
  it; a later read raises CPython's `seek of closed file`), or `Stream`,
  the caller's file object (`close()` leaves it open).
- Laziness and caches are pdfplumber's, because they decide order,
  identity, recursion and stream state (decoding empties `rawdata`):
  `PDF.pages` (walks the whole page tree, builds only the chosen pages,
  `doctop` accumulated over them, the partial list kept after an
  exception), `Page.layout`, `Page.objects`, the edges, `get_textmap`'s
  `lru_cache` (128 entries, keys hashed as Python hashes them: `1` and `1.0`
  share a `TextMap`), `pdf.objects` built on the same dicts, `flush_cache`,
  `close`.
- One `Page` type with `PageKind::{Original, Cropped, Filtered}` stands
  for `Page`, `CroppedPage` and `FilteredPage`, fields in `__init__` order.
  A cropped page keeps its `bbox` as given (`list` or `tuple`: error
  messages print it). `Drop for Page` unwinds a chain of derived pages in
  a loop, like CPython's trashcan.
- `process_object` runs on every layout object, `LTAnno` included, in
  `__dict__` order; `iter_layout_objects` is a chain of generators.
- Annotations: `resolve_all` of `/Annots`, each `parse` called from C;
  `/URI`, `/T`, `/Contents` decoded with CPython's strict codecs
  (`pycompat::codecs`); `/P` replaced by the page.
- `search` runs in two halves (`Page::search_text`, the text the pattern
  runs over; `Page::search_with`, the results from match spans): `re` is
  not ported. The stages feed Python's matches, the JavaScript API runs a
  `RegExp` ([Search](public-api.md#search)).

### Serialization

`to_dict`, `to_json` and `to_csv` take their options as Python values
(`object_types` iterated, `t + "s"`, `set(exclude_attrs)`...).
`getattr(self, t + "s")` accepts the names `to_dict` and `to_csv` build;
any other (a method name, a name not ending in `s`) raises `NotPorted`.
`pycompat::json` reproduces both encoders of `json.dumps` (the C encoder,
one unit per container; the Python encoder used with `indent`, one frame
per container and per float), `pycompat::csv` the `_csv.c` writer (minimal
quoting, `\r\n`, a single empty field written `""`). `stream=None` returns
the text; another stream is `NotPorted`.

### Structure Tree, Metadata And Annotations

- `structure.rs` ports `structure.py` function by function and reads the
  document's objects with Python's operators whatever their type
  (`py_in`, `py_getitem`, `py_iter`...), so it raises where Python raises.
  The `seen` tables are keyed by `repr(ref)` (the units of the builtin
  `repr`). `find_all` is lazy, like Python's generator.
- `PDFStructElement::to_dict` reproduces `dataclasses.asdict` (frames and
  units per level) and `copy.deepcopy` of its leaves: a reference copies
  the document down to its file object, which for a document opened by
  path raises `TypeError("cannot pickle 'BufferedReader' instances")` after
  the same frames. For a document read from bytes, Python copies the whole
  document; the port keeps the reference (an equal value) and does not
  count that copy: a documented difference
  ([Not Ported](public-api.md#not-ported)).
- `repr(el)` reproduces the dataclass `__repr__` behind
  `dataclasses._recursive_repr`.
- A cycle among the elements a page's parent tree reaches makes Python
  loop until memory runs out; the port raises `MemoryError` once the walk
  only repeats itself. Cycles at the document scope end in `RecursionError`
  in `prune`, as in Python.
- Metadata: `read_metadata` merges the `/Info` dicts and decodes each value
  with `resolve_and_decode`, which decodes dicts **in place**, in the
  document's own objects (`PDFObject::Str`); later readers (pages,
  structure tree, annotations) see the decoded values, as in Python. With
  `strict_metadata` the error is raised, otherwise logged.

## JavaScript Boundary

The boundary is coarse-grained: JavaScript sends the PDF's bytes and
requests, the core returns complete results (a page's objects, text,
words, tables), never one round trip per intermediate object. There is no
`wasm-bindgen`: the module exports a minimal ABI (`pp_alloc`, `pp_call`,
the result buffer) and results travel in a binary format (`PJB1`: tagged
values, a string table, objects in columns, numbers without loss).

The core keeps pdfplumber's `PDF` and its pages behind handles. Each
operation is one statement at the top level of a Python script
(`oracle/unit/js_pages_run.py`): a `RecursionError` reaches JavaScript
where it happens in that script, and the API's tests compare each member
with that script run by Python. Derived pages live in the core;
`filter(fn)` runs `fn` in JavaScript on the parent's objects and sends the
core a mask; `search` runs a JavaScript `RegExp` on the text the core
returns, and the core converts UTF-16 offsets to code points.

Predefined CMaps are not in the module: the core asks a provider for them
by name, and a call that misses one is void, the CMap fetched, and the call
re-run from the same process state. A void call rebuilds every document it
reached by replaying that document's history.

The details (ABI, result encoding, values, errors, diagnostics, CMaps,
documents and pages, the API layer and its job queue, worker mode, package)
are in [`js-runtime.md`](js-runtime.md).

## Performance

The rule: no optimization changes a trace. The same computation runs, with
the same frame and C-unit checks in the same order; only copies,
allocations and lookups go away. Each such site is commented "Performance:"
and covered by mutation testing.

- `Page.process_object` (`page.rs`), the hottest path: the dict is created
  at its final size; keys known to be absent are added without lookup
  (`Obj::push_new`, checked in debug builds); keys, object types, path
  commands and literals are built once per thread (`AttrKeys`,
  `obj::static_str`); `resolve_all_owned` resolves in place a value only
  the caller holds, with the same frames; `point2coord_with` avoids the
  point tuple; `decode_text_pystr` returns the same `str` when
  PDFDocEncoding maps it to itself; consecutive chars share their color
  space name; `py_getitem_index` reads `mediabox[i]` without converting the
  list.
- Shared values: font names, marked-content tags and recent short char
  texts are shared `PyStr`s (`str` identity is not observable); `mcid` and
  `tag` are stored inline (`Attrs`).
- The interpreter: `keyword_op` skips its replacements when a keyword has
  no `*`, `"` or `'`; `int_hash` of a small int allocates nothing; int and
  float arithmetic goes straight to `Num` (`int_float_operands`); the
  recursion counters are `#[inline]` with a `#[cold]` error path.
- Freeing: `PyValue::drop` returns at once for scalars and shared
  containers; `drop_nested` keeps its worklist between frees.
- The boundary: PJB1 tables read each cell once, write numbers and string
  indices in place per column, and share a `str` by address; values up to
  three levels deep are written and read without a worklist. The encoded
  bytes are unchanged (`crates/wasm/examples/encode_phases.rs` fingerprints
  them).

What remains is mostly the work Python does too: allocation (about a fifth
of native time), `PyValue` copies, the PostScript lexer, word extraction,
and `page.edges`/`merge_edges` for tables. An arena or fixed-shape dicts
would change the value model of `crates/pdfplumber`.

**Measuring.** The `quick` set (150 files, their first 20 pages: 302
pages), each phase as pdfplumber runs it (open with `pdf.pages`, `layout`,
`objects`, `extract_text`, `extract_words`, `find_tables`, and
`extract_tables` apart), through `scripts/heavy.sh`, one job at a time:

- native: `cargo build --release -p pdftrace --example plumber_phases`,
  then `target/release/examples/plumber_phases <cmap-dir> < list`
  (`PLUMBER_PHASES` selects phases); Python: its twin
  `crates/pdftrace/examples/plumber_phases.py`; Node: `js/bench/phases.mjs`
  on the `dist` build through the public API (a second read of
  `page.objects` measures the boundary alone); browsers:
  `js/bench/browser-phases.mjs` with `phases.html`, page and worker; the
  result encoding alone: `js/bench/node.mjs`, `js/bench/browsers.mjs`
  ([Result Encoding](js-runtime.md#result-encoding)).
- Compare two builds by alternating them in the same slot; times vary by
  several percent from run to run on a shared machine. Playwright's
  Firefox (a Nightly build) runs WebAssembly several times slower than a
  release Firefox: it is not a speed reference.
- Profiling without `perf`: `scripts/perf/sprof.c`, an `LD_PRELOAD`
  sampler that walks frame pointers (build with `RUSTFLAGS="-C
  force-frame-pointers=yes"` and `CARGO_PROFILE_RELEASE_DEBUG=line-tables-only`;
  reports by `sprof_report.py`, `sprof_lines.py`); `scripts/perf/mcount.c`
  counts allocations; for WebAssembly, `node --cpu-prof` on a `dist` build
  optimized with `wasm-opt -O3 -g` (`prof_sum.py`, `prof_callees.py`).
  Memory: peak RSS by `/usr/bin/time -v`; the module's linear memory never
  shrinks.

Last measurement, milliseconds per page on `quick` (shared 8-core Linux
machine; median of 3 runs, Python 1 run):

| | open | objects (layout included) | `extract_text` | `extract_words` | `find_tables` | total | `extract_tables` |
|---|---|---|---|---|---|---|---|
| Python pdfplumber 0.11.9 (CPython 3.12) | 5.02 | 118.27 | 7.31 | 4.48 | 4.69 | 139.8 | 5.20 |
| public API in Node 26 | 1.20 | 15.10 | 1.83 | 1.64 | 1.46 | 21.2 | 1.27 |
| Rust core, native | 0.32 | 10.68 | 1.72 | 1.36 | 1.71 | 15.8 | 1.12 |

In Node, moving the objects into JavaScript values costs about 3.2 ms of
the 15.1 ms of `objects`. Chromium and WebKit are on par with Node (about
22 ms per page in the page, 25 in a worker). Peak RSS: native 318 MiB,
Python 412 MiB, Node 505 to 554 MiB. `dist/pdfplumber.wasm` is about
2.6 MB (660 kB brotli).

## Upstream Data

- pdfminer's tables (`glyphlist.py`, `latin_enc.py`, `fontmetrics.py`,
  `encodingdb.py`, `pdffont.py`'s `NIBBLES` and CFF standard strings), the
  CCITT codes, the libm tables, CPython's `unicodedata` and the
  `stringprep` tables are **generated** into Rust by the scripts of
  `oracle/codegen/` from the Python modules, never copied by hand.
  `scripts/check.sh` runs each with `--check` (`unicode_tables.py` is run by
  hand). The option tables of the `plumber_*` stages are generated too
  (`plumber_ops.py` → `crates/pdftrace/src/stages/plumber_ops.rs` and
  `js/trace/plumber-ops.json`).
- The predefined CMaps (`pdfminer/cmap/*.json.gz`, about 8 MB) are not
  embedded. `oracle/codegen/cmaps.py` converts them to a compact format
  (`<cache>/cmap/<fingerprint>/`, not committed), which the core reads
  through a provider: files natively and in Node, a lazy `fetch` in the
  browser ([CMaps](js-runtime.md#cmaps)). Every entry point that loads fonts
  installs one: `CMapDB::set_default_provider` once per process, or
  `CMapDB::set_provider` per thread (which takes priority; `None` means
  installed without data: `CMapNotFound`). With no provider at all, loading
  a predefined CMap raises `RuntimeError`, never a silent empty CMap.
- `upstream/glibc` and `upstream/arm-optimized-routines` hold the sources
  of the ported `atan2` and `pow` (`pycompat::libm`), with their licenses.
