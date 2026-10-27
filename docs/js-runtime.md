# JavaScript Runtime

How the Rust core (`crates/`) is served to JavaScript: the `crates/wasm`
crate (ABI, result encoding, errors, diagnostics, CMaps, documents and pages
behind handles) and the `js/` package (loading, the public API, worker
mode). The public API is described in [`public-api.md`](public-api.md), the
porting rules and the value model in
[`architecture.md`](https://github.com/AxENSRennes/pdfplumber.js/blob/HEAD/docs/architecture.md).

## Overview

```
JS (js/src)                                    Rust (crates/wasm)
open(input) ── platform.readInput ─► bytes
            ── core.call("doc_open", ..., data) ─► pp_alloc ×2, pp_call ─► ops::Core::call
                                                   request PJB1 [op, handle, args, level]
            ◄─ decodeResponse ◄── response PJR1 (status, PJB1|JSON payload, diagnostics)
status 2 (missing CMaps): fetch/fs the files, "cmap_add", the same request again
```

The boundary is coarse-grained: JavaScript sends a document's bytes and
requests (an operation on a document, page, table or structure tree, with
its arguments); the core returns complete results. Each operation is one
statement of a Python script, so that results and recursion are those of
Python running that statement ([Documents And Pages](#documents-and-pages)).

## ABI

- **No `wasm-bindgen`.** The module imports nothing and exports `memory`,
  `pp_alloc(len) -> ptr`, `pp_free(ptr)`, `pp_call(req, data) -> status`,
  `pp_result_ptr()`, `pp_result_len()`, `pp_result_free()`, `pp_buffers()`
  (`crates/wasm/src/abi.rs`). The names are prefixed so that a native build
  does not export a `free` that would replace the C library's.
- **Memory.** `pp_alloc` creates the shared bytes the core keeps
  (`Rc<[u8]>`, what `pycompat::io::BytesIO` reads), in a table indexed by
  address; JavaScript writes into them and `pp_call` *takes* the buffer, so
  a PDF is in the module's memory once (linear memory never shrinks; a copy
  would double it). The response stays in a Rust buffer until the next
  call; JavaScript decodes it in place, then calls `pp_result_free`. No
  `unsafe` block: JavaScript reads and writes the memory, Rust only hands
  out addresses of buffers it owns. The crate has its own lints
  (`unsafe_code = "deny"` instead of the workspace's `forbid`) so that only
  `abi.rs` allows `#[no_mangle]`.
- **Addresses** returned by the module are wasm32 `i32`s, which JavaScript
  reads signed: `WasmCore` reads them unsigned (`>>> 0`), or any buffer
  beyond 2 GiB of linear memory would give a negative offset.
- **Handles** (`crates/wasm/src/handles.rs`): documents, pages and derived
  pages, tables, text maps, structure trees, streams. A `u32` handle is an
  index (20 bits) and a generation (12 bits): a freed handle reused later is
  refused (`ValueError("invalid handle")`).
- **Requests** (`crates/wasm/src/ops.rs`): the PJB1 list `[op, handle,
  args, level]` plus an optional data buffer. **Response**: a `PJR1` header
  (status 0 ok, 1 exception, 2 missing CMaps; payload format 0 PJB1, 1
  JSON, 2 typed; flags), the payload, the diagnostics block, each aligned on
  8 bytes. Flags: bit 0, the JSON needs its correction pass; bit 1
  (`FLAG_HISTORY_LONG`), the documents' history is past its limit
  ([Bounded History](#bounded-history)).
- **Panics.** The `dist` profile has `panic = "abort"`: a panic (a bug)
  becomes a WebAssembly `unreachable`. Any exception out of an export (that
  trap, a stack overflow: `RangeError` in V8 and JavaScriptCore,
  `InternalError` in SpiderMonkey, an impossible allocation) leaves the
  Rust state undefined: JavaScript raises `CoreCrash`, marks the instance
  dead, and the next `open()` instantiates a new core. A PDF too large for
  wasm32's 4 GiB ends that way, where Python would raise `MemoryError`.
- **Depth.** The core reads, writes and frees values without recursion
  (`PyValue` frees with a worklist, like CPython's trashcan; `PDFObject`
  too): a list nested 200,000 levels deep crosses the boundary both ways.
  Where Python recurses, the core recurses up to Python's limit, on an
  8 MiB stack ([Stack Size](https://github.com/AxENSRennes/pdfplumber.js/blob/HEAD/docs/architecture.md#stack-size)).

## Result Encoding

Two encodings are implemented (`crates/wasm/src/wire/`) and give the same
JavaScript objects (checked on the measurement corpus and by the random
tests of `crates/wasm/tests/boundary.rs`):

- **PJB1** (binary, the default): tagged values (small ints as `i32`, ints
  up to 2^53 as `f64`, beyond in decimal → `BigInt`, floats as their 8 IEEE
  bytes: NaN, ±inf and -0.0 cross as they are); a **string table** (each
  `str` once; strings without surrogates form one UTF-8 blob that
  `TextDecoder` decodes at once, cut at their ends in UTF-16 units; strings
  with a lone surrogate are stored in UTF-16); and **tables**: a list of
  dicts (a page's objects) is written by columns, grouped by key sequence
  (at most 64 shapes, a shape for two rows at least), each column typed when
  all its cells allow it (`f64`, `f64` or `None`, string indices, booleans,
  `None`, number sequences such as `matrix` and colors, otherwise generic
  values). JavaScript builds the rows of a shape with a generated function
  (`new Function`, an object literal with the keys in order; a fallback
  without code generation when a CSP forbids it, tested), about six times
  faster than per-key assignments in V8. The functions are kept by shape,
  under the key `JSON.stringify([types, keys])`.
- **JSON**: ordinary JSON (shortest numbers, which `JSON.parse` reads back
  exactly, `-0` included); what JSON lacks (NaN, ±inf, big ints, `bytes`,
  pdfminer objects) becomes an object with the single key `"\u0000"`, and a
  real key starting with U+0000 gets one more; a response flag says
  whether JavaScript must walk the value to undo these escapes.
- **Typed** (format 2, `set_format("typed")`): PJB1 without tables
  (`BinWriter::without_tables`), every value keeping its tag: `int` and
  `float`, `list` and `tuple`, a dict's key order. JavaScript decodes it into
  typed values (`js/src/core/typed.ts`: `PyFloat`, `PyTuple`, `PyDict`) and
  gives the API their projection; `core.lastTyped` is the typed value of the
  last result. This is the format of the harness that writes stage traces
  through the API (`js/trace/`), never the API's.
- **Values by reference.** A pdfplumber `Page` in a result is its own kind
  (PJB1 `0x12`, JSON `"page"`): the page's handle and `page_number`. A
  `PDFStream` is written by reference (PJB1 `0x13`, JSON `"streamref"`:
  its handle, `objid`, `genno`, `attrs`) when the runtime asked for stream
  references (`set_stream_refs`), by value otherwise (`0x0f`: `rawdata` and
  `data` copied; always by value in the typed format).

**Why PJB1.** On the `chars` and `edges` of the 215 real pages of
`oracle/unit/text_tables_real.py` (229,555 chars, 39,236 edges), one call
per page, PJB1 is 2.8 to 7 times faster end to end than JSON depending on
the engine (Node, Chromium, Firefox, WebKit), 2.3 times smaller, and its
decoder is iterative (a value nested 10^5 levels deep crosses everywhere).
JSON stays available (`set_format`, debugging) and tested. To reproduce:

```sh
cargo run --release -p pdfplumber-wasm --example bench_corpus -- $PDFPLUMBER_ROOT/.cache/bench/pages_real.pjb
node scripts/build.mjs
node js/bench/node.mjs --passes 5
node js/bench/browsers.mjs --passes 5   # Playwright's Chromium, WebKit, Firefox, and a system Firefox
```

## Values

| Python | JavaScript |
|---|---|
| `None`, `bool` | `null`, boolean |
| `int` | `number` when \|n\| ≤ 2^53, else `bigint` |
| `float` | `number` (NaN, ±Infinity, -0 included); `3` and `3.0` give the same number |
| `str` | string; a lone surrogate is kept |
| `bytes` | `Uint8Array` |
| `list`, `tuple` | `Array` |
| `dict` | plain object, keys in Python's order (except integer-like keys, which JavaScript puts first); `__proto__` is an own property |
| `PSLiteral`, `PSKeyword`, `PDFObjRef` | the classes `PSLiteral`, `PSKeyword`, `PDFObjRef` |
| `PDFStream` | a `PDFStream` by reference (`objid`, `genno`, `attrs`; `rawdata`, `data`, `getData()`, `getRawdata()` read on demand), or a snapshot (typed format) |
| pdfplumber `Page` (`annot["data"]["P"]`) | `PageRef` (`handle`, `page_number`) |

The handle of a `Page` value is the first live handle of that page, else a
new one the operation creates (a replay creates it again), in the order
values are written (`Core::label_pages`; streams likewise,
`Core::label_values`); `oracle/unit/js_pages_run.py` names pages the same
way.

**Arguments** (JavaScript → Python, PJB1 without tables, `encodeValue`): a
safe integer is an `int`, another number a `float`, a `bigint` an `int`,
`PyFloat(x)` a `float`, `PyTuple([...])` a `tuple`, an array a `list`, a
plain object or a `Map` with string keys a `dict` (`undefined` values left
out), `Uint8Array`/`ArrayBuffer` `bytes`. Objects of a result given back as
arguments become the objects they stand for (`binary::decode_args`,
`Core::object_arg`): `PSLiteral` and `PSKeyword` interned again (the same
object pdfminer made), a `PDFObjRef` bound to the document of the request's
target, a `PageRef` the page of its handle, a `PDFStream` by reference the
stream of its handle; a snapshot `PDFStream` is not an argument
(`TypeError`). `js_pages_run.py` does the same (`bind_args`). The encoder is
not recursive; a container that holds itself is a `TypeError`.

**Strings and surrogates.** The result writer accepts a `str` in any form a
ported module keeps (`strings::PyStrRef`: `&str`, generalized UTF-8 as
Python's `surrogatepass`, UTF-16 units, code points) and sends UTF-16 units,
tested with hand-made buffers in every engine. The only loss is on the
JavaScript side: a high then a low surrogate code point become the astral
character they encode. The other way, JavaScript encodes generalized UTF-8,
which the core reads as a `pycompat::PyStr`: a lone surrogate in an
argument (value or dict key) arrives as it is.

## Errors

An exception (`pycompat::PyErr`) arrives with its class name, module,
`__qualname__`, MRO (the C3 linearization of `ExcKind`'s bases, with
`builtins.object`) and `str(e)`; JavaScript makes a `PDFPlumberError` whose
`name` is the Python name (`js/src/errors.ts`). The MRO of each of the 71
classes of `ExcKind` is compared with CPython's (`oracle/unit/js_runtime.py`,
`exceptions.jsonl`), in Rust and in every engine. `NotPorted` (a path not
ported) becomes the JavaScript error `NotImplemented`, which keeps module,
class and MRO. The exception a pdfplumber exception wraps
(`PdfminerException(e)`, `MalformedPDFException(e)`) arrives under the key
`cause`, in the same format: it is the `PDFPlumberError`'s `error.cause`.

## Diagnostics

`pycompat::diagnostics` is the channel of `logging` and `warnings.warn`: a
ported module calls `diagnostics::emit(source, level, || message)`, and the
message is formatted only when someone listens. Sent: pdfplumber's two
`logger.warning` calls (metadata, unknown attribute class), its
`warnings.warn` of `Page.annots` (Python's default filters through
`pycompat::warnings`: once per text, category and line in the process; the
registry is process state, which a void call restores), and each
`log.warning` of pdfminer (`pdfinterp`, `pdfdevice`, `pdffont`, `cmapdb`,
`pdfpage`, `pdfdocument`, `pdftypes`), whose text is `msg % args` as
`record.getMessage()` formats it (`%r` through the ported `repr`,
`pdfminer::log_text`); a text whose formatting raises is not sent, as
`logging` drops the record. pdfminer's `log.debug` calls count their frames
only.

The text changes no result and no recursion: the replay of `pages.jsonl`
runs once without collecting and once collecting everything. The boundary
runs each request inside `diagnostics::collect` at the level JavaScript
asked for (0: nothing collected or formatted), at most 1,000 entries per
call (the others counted), returned with the response, also when the call
raises. A void call returns none: its re-run emits Python's, once.
JavaScript hands them to `onWarning` (`api.ts`, `diagnosticsPolicy`) with
the level's name and, when known, the page number. `diagnostics.jsonl` of
`oracle/unit/js_runtime.py` (what Python shows for documents made to
produce each kind of record) is replayed by `js_runtime_unit.rs` and, through
`onWarning`, by `js/test/shared/tests.mjs`.

## CMaps

pdfminer's predefined CMaps (`pdfminer/cmap/*.json.gz`, 7.9 MB) are not in
the module. `oracle/codegen/cmaps.py` converts them to the compact format
`cmapdb::CMapModule::parse` reads (148 files, 3.9 MB);
`scripts/build-cmaps.mjs` compresses them with zlib into
`dist/cmaps/<name>.pdfcmap.zlib` (3.0 MB in all; a Japanese document loads
`UniJIS-UCS2-H` and `to-unicode-Adobe-Japan1`: 156 kB) and writes the list
of names to `js/src/generated/cmap-names.ts` (committed; `--check-names` in
`scripts/check.sh`).

- **zlib, not gzip or brotli**: the core inflates the files with its own
  zlib port (`pycompat::zlib`), so every environment decodes them with the
  same code, without `DecompressionStream`. A file already decompressed (a
  server applying its own `Content-Encoding`) is accepted as is.
- **Lazy loading and void calls** (`crates/wasm/src/cmaps.rs`,
  `WasmCore.call`): JavaScript gives the core the names that exist
  (`cmap_configure`). When the core asks for a known CMap not yet provided,
  the provider records the name and answers "absent", as an installation
  without that file would: the ported code goes on (pdfminer replaces a
  CMap it cannot find with an empty `CMap()`, which the document's font
  cache keeps). The call is then **void** (`ops.rs`, "Void calls"): the
  boundary drops its result and diagnostics, removes the handles it created
  and its entries from the history, restores the modelled Python process
  state as it was before the call (`pdfminer::process_state`: interned
  names, compiled `re` patterns, `typing` cache, logger caches, specialized
  sites, `CMapDB` caches), restores every document the call reached
  ([Void Calls And Replay](#void-calls-and-replay)), and answers "missing
  CMaps" with the names. JavaScript fetches them (`fetch` in the browser and
  the worker, `fs` in Node, or the `cmaps.load` function of `configure`),
  hands them over (`cmap_add`) and sends the same request again, which sees
  the CMaps from the start: its result is Python's with every CMap
  installed. Each name is asked once (the provider keeps the files it got,
  and the core reads again from them what the state restore removed from
  the caches); a name asked twice is an error. A name outside the list is
  `CMapNotFound` without any request; with `cmaps: false`, every CMap is
  absent, as in an installation of pdfminer without its `cmap` folder.
- **Preloading at the first miss** (`crates/wasm/src/cmap_scan.rs`): the
  first void call of a document also asks for every predefined CMap its
  fonts name. The core reads the document apart (a new `PDFDocument` of the
  same bytes and password, its page tree and the fonts of each node's
  resources; no content stream or XObject) and adds the `/Encoding` of each
  `Type0` font and the `to-unicode-<Registry>-<Ordering>` map of its
  descendants' `/CIDSystemInfo`. One void call per document instead of one
  per CMap; a document that misses none pays nothing (scanning at every
  open cost up to three quarters of opening a large document). The scan
  has no Python effect: the process state is restored after it, its
  diagnostics dropped, its errors stop it, it does not enter the history
  and loads no CMap. A CMap it misses (a font of a Form XObject) is loaded
  by a later void call.
- Tests: `cmaps.jsonl` of `oracle/unit/js_runtime.py` (decoding and
  `get_unichr` on ten CMaps and four collections) goes through this
  protocol in Rust (`crates/wasm/tests/js_runtime_unit.rs`, zlib and
  decompressed files) and in every engine; `documents.jsonl` keeps two CJK
  documents with pdfplumber's text and the CMaps loaded;
  `crates/pdftrace/tests/lazy_cmap.rs` checks the re-run through the
  interpreter at the recursion boundary.

## Documents And Pages

`crates/wasm/src/pages.rs` keeps pdfplumber's `PDF` and its pages behind
handles.

### Operations

- **Document**: `doc_open` (the bytes and the keywords of `pdfplumber.open`)
  runs `pdfplumber.open(io.BytesIO(data), **options)`; `doc_open_file`
  (`[path, options]`) runs `pdfplumber.open(path, **options)` on bytes
  JavaScript read (`Source::Path`: the `PDF` owns the file, `close()` closes
  it). The handle keeps the `PDF` (`Document::state`), its
  `PDFResourceManager` and its pages. Then `doc_metadata`, `doc_pages` (a
  handle per page of `pdf.pages`, with its geometry), `doc_objects`,
  `doc_attr` (`getattr(pdf, name)`), `doc_annots`, `doc_hyperlinks`,
  `doc_structure_tree`, `doc_to_dict`/`to_json`/`to_csv`, `doc_close`
  (`PDF.close()`, the handle stays; `free` frees it).
- **Pages**: `page_info` (the geometry, then `width` and `height` each read
  in its own statement: the value, or its exception as
  `width_error`/`height_error`), `page_objects`, `page_attr`,
  `page_structure_tree`, `page_extract_text`, `page_extract_text_simple`,
  `page_extract_words`, `page_extract_text_lines`, the search in two calls
  (`page_search_text`: the text the pattern runs over, computed where
  `search` computes it; JavaScript runs its `RegExp` on it; `page_search`
  takes the spans, in UTF-16 units, or in code points with `code_points`,
  its seventh argument, for the trace harness), `page_crop`,
  `page_within_bbox`, `page_outside_bbox`, `page_filter` (the mask
  JavaScript computed on the parent's objects, by kind, read by identity
  by the test function; objects computed again after the parent's
  `flush_cache` are not in it: `NotPorted`), `page_dedupe_chars` (these five
  give a derived page's handle), `page_find_tables`/`page_find_table` (a
  handle per table), `page_debug_tablefinder`, `page_extract_tables`,
  `page_extract_table`, `page_to_dict`/`to_json`/`to_csv`, `page_close`,
  and `table_extract`.
- **Structure trees** (`crates/wasm/src/structs.rs`): `struct_tree [page or
  None]` (`PDFStructTree(obj, page)`: the tree's handle and its elements,
  the dataclass's fields, written without recursion), then, on the tree, an
  element named by its path (indices from `tree.children`):
  `struct_find_all` (`list(x.find_all(m))`), `struct_find` (`x.find(m)`),
  where `m` is a type or another value called as Python calls it; with a
  third argument, the mask (the truth of the function or `RegExp` that
  JavaScript evaluated on each element, in `_find_all`'s order), `m` is not
  read: the mask has its own argument, so that no caller's value (a dict
  `{mask: [...]}`) is taken for it. Then `struct_element_bbox`,
  `struct_all_mcids`, `struct_element_repr`, `struct_element_to_dict`.
- **Streams** (`crates/wasm/src/streams.rs`): with `set_stream_refs`, each
  `PDFStream` of a result gets a handle that belongs to its document and
  lives as long as it; `stream_rawdata`, `stream_data`, `stream_get_data`
  (which decodes the stream), `stream_get_rawdata` reach the document. After
  `PDF.close()`, a stream keeps its bytes, as in Python.

### Depth

Each request is a statement at the top level of a Python script
(`ENTRY_DEPTH`: one frame, two C units, measured by the script): a property
is read at a site of its own (`pycompat::specialize::AttrSite`: the getter
called from C the first time, two units), a method called with `*args,
**kwargs` (`CALL_FUNCTION_EX`: the bound method entered from C, two units),
an attribute by name with `getattr`. That script is
`oracle/unit/js_pages_run.py`: a `RecursionError` reaches JavaScript where
it happens in that script. Arguments are bound as CPython binds the call's
(`pycompat::call`), before the call; `table_settings` is resolved as
`TableSettings.resolve` resolves any value; `page_filter` computes the
parent's objects as the script's `mask_of` does (one frame, a load site of
its own). A Python statement written otherwise (explicit keywords instead of
`**options`, a property read at a new site) takes up to two C units more or
less: visible only at the C limit.

### Void Calls And Replay

A document that a void call reached drops its state (its `PDF`, the pages of
its page and table handles: `Object::reset`), then the core **replays its
history**: each call that reached it (from `doc_open` to the previous call;
`Entry`), results and diagnostics dropped, from `ENTRY_DEPTH`, each filling
the handles it had created. Each entry keeps the process state its first
run started from (`ProcessState`: interned symbols in a persistent chain
shared between states; the rest shared with the previous state when
unchanged) and how it ended (an exception or nothing): its replay starts
from that state, or a name interned, a site specialized or a CMap cached by
a later call would let it go deeper than the first time, succeed where it
had raised `RecursionError`, or cache objects Python never had. A replay
that ends otherwise than the first run (another exception, other objects
created, a CMap missed) leaves its documents unusable: every operation that
reaches them raises `RuntimeError`, never a silently different document.
The void call's own process state is restored after the replay. The
document is then the object Python's single run has at that point (page
caches, decoded streams, metadata decoded in place), not a reopened one.

### Freed Handles

Freeing a page or a table frees its handle (its slot will serve again); the
history names objects by identity (`Target::Object`), not by handle: a
replay rebuilds a freed object outside the table, for the calls that used
it, then drops it. Freeing a document frees its pages, tables and history;
`free` cannot enter a `sequence` (a void call would not undo it).

### Bounded History

A document's history only shrinks when the document is freed. Once every
known CMap has been handed to the core (`cmaps::complete`; an installation
without CMaps is complete from the start), no call can be void any more:
the history is emptied at the next call and nothing more is recorded (the
documents are marked: if CMap names became known later, a void call on
them would leave them unusable, never different). Beyond `HISTORY_LIMIT`
entries (4,096) or `HISTORY_BYTES_LIMIT` bytes of requests (4 MiB: an
entry keeps its call's arguments, such as a `filter` mask), each response
says so (flag 1) and the runtime hands over every CMap (`WasmCore.loadAll`,
about 3 MB). A file the loader cannot provide then (404, network) is marked
unavailable in the core (`cmap_unavailable`): `complete` holds without it,
the history goes, and a call that needs it is still void, the runtime tries
to load it again, and a document whose history is gone cannot be rebuilt
(each later operation on it raises) where, without the file, its call could
only fail. A name handed over later (`cmap_add`) becomes available again.

## API Layer

`js/src/api.ts` implements the public API (`open()`, `configure()`,
`PDFPlumberDocument`, `PDFPlumberPage`, `Table`) over the operations above;
`js/src/api/structure.ts` the structure trees, `js/src/search.ts` the
patterns of `search()`, `js/src/types.ts` the public types.

### Jobs And Order

The core behaves like one Python process running one statement after
another. Every member is one *job* of its core, queued when the member is
called (`Core.exclusive`, a FIFO per core, in the page and in worker mode):
its operations run in the order the members were called, whatever order
their promises are awaited in, each job to its end (its re-runs after a
void call included) before the next starts. Without it, a call made while
another waited for its CMaps would pass before its re-run. `search` (the
`RegExp` runs between `page_search_text` and `page_search`) and `filter`
(the test function runs between `page_objects` and `page_filter`) are one
job each. Every member returns a promise and never throws synchronously;
only a page's geometry is synchronous.

Caching follows Python: `pdf.pages` keeps the array of the first read that
resolves until `close()` (which flushes `_pages`), a rejected read is not
kept, and each read is a job decided in call order. Other getters run their
statement at each read: a statement that raised can succeed when run again,
as in Python, where a specialized site holds fewer C units.

### API Operations

Each member runs these operations in one job; each operation is one
statement at the top level of `oracle/unit/js_pages_run.py`, which is what
the sessions of `oracle/unit/js_api.py` run in Python to give the expected
results.

| Member | Operations (statements) |
|---|---|
| `open(bytes)` / `open(path)` | `doc_open` (`pdfplumber.open(io.BytesIO(data), **options)`) / `doc_open_file` (`pdfplumber.open(path, **options)`) |
| `pdf.pages` | a job: `doc_pages` (`obj.pages`, then each page's geometry) at the first read and after `close()`; none otherwise |
| `pdf.metadata`, `objects`, `annots`, `hyperlinks`, `structure_tree` | `doc_metadata`, `doc_objects`, `doc_annots`, `doc_hyperlinks`, `doc_structure_tree` (a property load at a site of its own) |
| `pdf.chars` ... | `doc_attr` (`getattr(obj, name)`) |
| `pdf.toDict`, `toJson`, `toCsv` | `doc_to_dict`, `doc_to_json`, `doc_to_csv` (`obj.to_dict(**kwargs)`...) |
| `pdf.close()` | `doc_close` (`obj.close()`) |
| `pdf.release()` | `free` (no statement) |
| `pdf.page(ref)` | a job: none when the API holds an object of that page (the ref's handle's, or the current `pdf.pages` list's for a ref read since the last `close()`), else `page_info` |
| `page.objects`, `structure_tree` | `page_objects`, `page_structure_tree` |
| `page.chars` ..., `annots`, `hyperlinks` | `page_attr` (`getattr(obj, name)`) |
| `extractText`, `extractTextSimple`, `extractWords`, `extractTextLines` | `page_extract_text`, `page_extract_text_simple`, `page_extract_words`, `page_extract_text_lines` (`obj.method(*args, **kwargs)`) |
| `search` | `page_search_text` then `page_search` (one statement, `obj.search(pattern, **options)`; JavaScript runs its `RegExp` between the two; a pattern that is neither a string nor a `RegExp` goes to `page_search`, which raises Python's error); none without a pattern |
| `crop`, `withinBbox`, `outsideBbox`, `dedupeChars` | `page_crop`... `page_dedupe_chars`, then `page_info` of the derived page (`obj.width`, `obj.height` read at sites of their own) |
| `filter` | `page_objects`, `page_filter` (`mask_of(page, ...)`: the mask), `page_info`; for a test that is not a function, `page_filter` (`mask_of(page, {"value": v})`) and `page_info`; none without a test |
| `findTables`, `findTable`, `extractTables`, `extractTable` | `page_find_tables`... (`obj.find_tables(*args)`) |
| `table.extract` | `table_extract` (`obj.extract(**kwargs)`) |
| `page.toDict`, `toJson`, `toCsv`, `close` | `page_to_dict`, `page_to_json`, `page_to_csv`, `page_close` |
| `debugTablefinder` | `page_debug_tablefinder` (`obj.debug_tablefinder(*args)`) |
| `pdf.structTree(page?)` | `struct_tree` (`PDFStructTree(obj, page)`) |
| tree's or element's `findAll`, `find` | `struct_find_all` (`list(x.find_all(m))`), `struct_find` (`x.find(m)`); a `RegExp` or a function as the truth of each element (the mask), run in the job first |
| `elementBbox`, element's `allMcids`, `repr`, `toDict` | `struct_element_bbox` (`tree.element_bbox(el)`), `struct_all_mcids` (`list(el.all_mcids())`), `struct_element_repr` (`repr(el)`), `struct_element_to_dict` (`el.to_dict()`) |
| `PDFStream.rawdata`, `data`, `getData`, `getRawdata` | `stream_rawdata`, `stream_data`, `stream_get_data`, `stream_get_rawdata` (a job of the stream's document) |
| a derived page's, table's, tree's finalizer | `free` (no statement) |

### Lifetimes

- `release()` frees the document in the core (pages, derived pages,
  tables, streams, history); a job queued before it still runs. A
  `FinalizationRegistry` frees unreachable documents, derived pages, tables
  and structure trees; a page, table, tree or stream holds its document.
  Each document has a `Life`: a handle of a released document is never
  freed (the core may have given its slot to another object).
- Finalizers run in tasks of their own, which a loop that awaits only the
  API's promises never lets run: every 64 tracked objects, the next job
  first yields a turn of the event loop (`setImmediate` in Node, a
  `MessageChannel` message elsewhere), so the finalizers of the objects
  collected by then run and their frees are queued. Without it, such a loop
  over `crop` and `extractText` kept every derived page in the core until
  it crashed.
- A derived page whose annotations were read (their `data.P` names it) is
  kept by its document, so that `pdf.page(ref)` gives it back.
- Streams by reference (`StreamRefs`) are kept per document in a `WeakMap`
  keyed by the document, so nothing of the runtime keeps a document alive.
  Each `PageRef` and `PDFStream` is recorded as an object of its core and
  document: a request of another core cannot carry it (`TypeError`), and
  `pdf.page(ref)` takes only references from `pdf`'s own results.
- A `free` never changes `core.lastTyped` (the trace harness reads it right
  after a member's request).

### Worker Mode

`configure({ worker: true })` runs the core in a dedicated worker
(`js/src/worker/`): requests are encoded and results decoded in the calling
thread (the classes survive), jobs are queued there too, and the test
functions of `filter` and the `RegExp` of `search` run there, between the
operations of their job; the worker runs its messages one at a time and
fetches the CMaps itself (`cmaps.baseUrl`; a loader function cannot cross
to a worker). A core that crashes in worker mode stops its worker at once
(`WorkerCore` receives the `CoreCrash` and calls `terminate()`; `getCore()`
does it too before starting another), and pending requests get `CoreCrash`.
A worker script that cannot load rejects `open()` with an error naming its
URL (the browser's error event has no message).

## JavaScript Package

```
js/src/index.ts, browser.ts   Node and browser entry points (platform + exports.ts)
js/src/api.ts, api/           open(), configure(), PDFPlumberDocument, PDFPlumberPage, Table, structure trees
js/src/core/runtime.ts        WasmCore: ABI, requests, job queue, re-runs after missing CMaps
js/src/core/wire.ts           PJB1/JSON decoders, argument encoder
js/src/core/typed.ts          typed values (PyDict, project) of the typed format
js/src/platform/*.ts          Node (fs, fetch) and browser (fetch, compileStreaming, Worker)
js/src/worker/*.ts            worker mode: requests encoded here, run (CMaps included) in the worker
js/src/search.ts              search() patterns (JavaScript RegExp → spans)
js/src/internal.ts            internal API of the tests and benchmarks (pdfplumber-js/internal)
js/src/hooks.ts               internal hooks of the trace harness (never public)
js/trace/                     plumber_* stage traces through the public API (oracle.verify --engine js-api)
```

A new core (after a crash, `configure()`, or the tests' `dropCore()`)
instantiates the module already compiled from the same source (default
URL, string or `URL`; given bytes or a `WebAssembly.Module` are used as
they are) instead of fetching and compiling it again, and `dropCore()`
stops the abandoned core's worker.

**Build.** `node scripts/build.mjs` runs `build-wasm.mjs` (cargo `--profile
dist`, then `wasm-opt -O3` from the npm package `binaryen`; it fails without
it unless `--no-opt` is given), `build-cmaps.mjs` and `build-js.mjs` (`tsc`
→ `dist/src` with the `.d.ts` files; esbuild →
`dist/browser/{index,internal}.js` with shared chunks, and
`dist/browser/worker.js` alone, one file without shared chunks, which an
application can copy next to its bundle, exported as
`pdfplumber-js/worker`). The default worker is created in the form bundlers
recognize (`new Worker(new URL("./worker.js", import.meta.url), { type:
"module" })`). The declarations check without the DOM library and bring
`esnext.disposable` themselves. No runtime dependency.

**Package check.** `scripts/check-package.mjs` (in `scripts/check.sh`)
checks the package as `npm pack` makes it: files shipped (`LICENSE`,
`THIRD_PARTY_NOTICES`, module, CMaps, `docs/public-api.md`,
`docs/js-runtime.md`) and excluded, relative links of the shipped Markdown,
a one-file worker, then, installed in a new project, its declarations (a
Node project without DOM, with and without `await using`; a browser
project), `import` and `require()`, and its subpaths; with `--bundles`
(`scripts/check-browsers.sh`), two applications that bundle it (esbuild,
Vite) following the README's recipes, which `bundlers.spec.ts` runs in
Chromium (page and worker, a CJK document). `THIRD_PARTY_NOTICES` is written
by `scripts/third-party-notices.mjs` (pdfminer.six, pdfplumber, Adobe's
CMaps, CPython, zlib, Arm optimized-routines, the linked crates); `--check`
verifies the list of crates and that the module does not contain the port
of glibc's `atan2` (LGPL).

**Oracle module.** `node scripts/build-wasm.mjs --oracle` builds
`dist/pdfplumber-oracle.wasm`: the same module with the hooks of the trace
harness (feature `oracle-hooks`, `crates/wasm/src/oracle.rs`), in its own
target directory (`target/oracle`), never shipped (`package.json` lists
only `dist/pdfplumber.wasm`; see
[Parity Through The JavaScript API](https://github.com/AxENSRennes/pdfplumber.js/blob/HEAD/docs/verification.md#parity-through-the-javascript-api)).

## Size

`node scripts/size.mjs` (gzip -9, brotli 11); `--budget` in
`scripts/check.sh`: 1 MB brotli for the module, 40 kB for the browser entry.

| File | raw | gzip | brotli |
|---|---|---|---|
| `dist/pdfplumber.wasm` (`wasm-opt -O3`) | 2,576,399 | 939,137 | 661,630 |
| `dist/browser/index.js` and its chunks | 46,099 | 15,158 | 13,757 |
| `dist/browser/worker.js` | 25,143 | 8,908 | 8,162 |
| `dist/src` (Node, `.js`) | 139,908 | 42,167 | 37,087 |
| `dist/cmaps` (148 zlib files, loaded on demand) | 2,976,819 | | |

## Tests

- **Rust** (`cargo test -p pdfplumber-wasm`, natively and on
  `wasm32-wasip1` in `scripts/check.sh`): `tests/boundary.rs` (4,000 random
  values encoded in PJB1 and JSON and decoded by Rust decoders that follow
  `wire.ts`, every column kind, several shapes, surrogates, integers at the
  bounds, PDF cycles, 200,000 levels of nesting written, read and freed,
  random arguments, truncated or altered input without panic, the ABI, the
  requests' errors, diagnostics) and `tests/js_runtime_unit.rs` (the Python
  cases of `oracle/unit/js_runtime.py` through `Core::call`, in both
  formats: MROs, lazy CMaps, void calls and replays, `search`'s
  `main_group`, text and tables, and the sessions of boundary operations of
  `pages.jsonl`, run in a fresh process each, with their depth windows).
- **JavaScript**: `js/test/shared/tests.mjs`, the same tests for Node
  (vitest, `js/test/node/`, with and without code generation) and the
  browsers (Playwright, `js/test/browser/`, core in the page and in a
  worker): loading, adversarial values (NaN, -0, big integers, lone
  surrogates from hand-made buffers, `__proto__`, 10^5 levels both ways,
  cycles), hand-made PJB1 blocks, errors and MROs against CPython,
  `CoreCrash` on a stack overflow, diagnostics, lazy CMaps and void calls
  against pdfminer, text, words, search and tables against pdfplumber,
  `open()` inputs and options, a PDF once in memory, no leaked buffer.
  `js/test/shared/api-sessions.mjs` replays the sessions of the public API
  ([Parity Through The JavaScript API](https://github.com/AxENSRennes/pdfplumber.js/blob/HEAD/docs/verification.md#parity-through-the-javascript-api)).
- **Browsers**: Playwright's Chromium, Firefox and WebKit
  (`js/playwright.config.ts`). WebKit needs system libraries (GStreamer,
  libwoff2, libavif, Mesa's EGL...): on a Debian or Ubuntu machine without
  root, `scripts/webkit-local-libs.sh` downloads them (`apt-get download`,
  no installation) under `.cache/webkit-libs` and writes a launcher that
  `scripts/check-browsers.sh` hands to Playwright (`PDFPLUMBER_WEBKIT`).
- **Mutation testing** of `crates/wasm/src` and of the boundary code in
  `crates/pdfplumber` takes the `pdfplumber-wasm` tests:

  ```sh
  scripts/heavy.sh cargo mutants -f crates/wasm/src/{abi,cmaps,errors,handles,ops,strings}.rs \
    -f crates/wasm/src/wire/{binary,json,mod}.rs -f crates/pdfplumber/src/{obj,table,utils/text}.rs \
    -F '^crates/wasm/src/|take_items|Drop for PyValue|search_with|replace resolved|in resolved' \
    --test-package pdfplumber-wasm --profile mutants -j 1 --timeout 20
  ```

  The JavaScript layers have their hand-mutant scripts
  ([Mutation Testing](https://github.com/AxENSRennes/pdfplumber.js/blob/HEAD/docs/verification.md#mutation-testing)).
