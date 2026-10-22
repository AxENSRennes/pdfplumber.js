# Public Extraction API

`pdfplumber.js` exposes a browser-capable JavaScript API modeled on Python
`pdfplumber` 0.11.9. The extraction engine is a faithful Rust port of
pdfplumber and pdfminer.six compiled to WebAssembly (`crates/`); results are
those of Python pdfplumber (CPython ≥ 3.12), verified against it through
this API itself. Callers use the same `open()` entry point in Node and
browsers. How the runtime works (WebAssembly loading, result encoding,
CMaps, workers) is described in [`js-runtime.md`](js-runtime.md); the
contract of wave 5, its workstreams and its harness in
[`architecture.md`](architecture.md) ("Vague 5").

> **Status (wave 5, base).** Every member below is implemented over the
> operations of the core and checked against Python by the sessions of
> `oracle/unit/js_api.py` (Node, the dist build, Chromium, Firefox and
> WebKit, in the page and in worker mode) and by the `plumber_*` stage
> traces written through this API (`oracle.verify --engine js-api`),
> except those marked **pending**, which reject with `NotImplemented`
> until their boundary operation lands (workstream BOUNDARY):
> `page.debugTablefinder()`, `pdf.structTree()` (`PDFStructTree`), the
> `PDFStream` by reference.

## Entrypoint

```ts
import { open } from "pdfplumber-js";

await using pdf = await open(input, options); // close() and release() at the end of the scope
const page = (await pdf.pages)[0];
console.log(await page.extractText());
```

Supported inputs (`PDFInput`):

- Node: filesystem path or `file:` URL, opened as `pdfplumber.open(path)`
  (the `PDF` owns the file: `close()` closes it, and an object not read
  before then raises `ValueError: seek of closed file`, as in Python);
  HTTP(S) `URL` or URL string, `ArrayBuffer`, any `ArrayBufferView`
  (`Uint8Array`, `Buffer`), `Blob`, opened as
  `pdfplumber.open(io.BytesIO(bytes))` (the caller's stream:
  `close()` leaves it open).
- Browser: `ArrayBuffer`, `ArrayBufferView`, `Blob`, `URL`, and URL string
  (resolved against the document's base URL), all opened from their bytes.

`open()` options (`OpenOptions`): every option is a keyword argument of
`pdfplumber.open`, as given, except the API's own `logLevel` and
`onWarning`:

- `pages`: one-based page numbers to load.
- `password`: password for encrypted PDFs (a string; an unencrypted
  document never reads it).
- `laparams`: `LAParams` keyword arguments (layout analysis), or `null`.
- `unicode_norm`: one of `NFC`, `NFKC`, `NFD`, or `NFKD`.
- `raise_unicode_errors`, `strict_metadata`: as in pdfplumber.
- `repair`, `gs_path`, `repair_setting`: accepted as pdfplumber accepts
  them; `repair: true` is not ported (`NotImplemented`).
- Any other name: Python's `TypeError` (`PDF.open() got an unexpected
  keyword argument ...`), raised by `open()`.

As `pdfplumber.open`, `open()` does not check the values: they go to the
core as they are, and the ported code raises Python's error where Python
raises it (an unknown `unicode_norm` when text is normalized, a `pages`
that is not a list of numbers when the pages load...).

- `logLevel`: where diagnostics go besides `onWarning`: `silent` (default),
  `warning` (warnings and above to `console.warn`) or `debug` (everything).
- `onWarning`: callback receiving `{ source, level, message, page? }` for
  each diagnostic at warning level and above (every level with
  `logLevel: "debug"`).

Diagnostics are the text of pdfplumber's `logging` calls (`source` is the
logger name: `pdfplumber.pdf` for a metadata value that cannot be
decoded, `pdfplumber.structure` for an unknown attribute class) and of
its `warnings.warn` (`source` is the warning category: `UserWarning` for
an annotation text that cannot be decoded), shown as Python's default
filters show them: a warning once per text for the whole runtime (the
filters' registry). pdfminer's own `logging` calls (malformed operators,
fonts, xref entries...) are forwarded by workstream BOUNDARY (pending: the
core counts their Python frames but drops their text). Diagnostics never
change a result. Python prints warnings to stderr by default; the
JavaScript API is silent by default.

Runtime settings, before the first `open()` (`configure(options)`,
`RuntimeOptions`): where the WebAssembly module is (`wasm`: URL, path,
bytes or compiled module), where the predefined CMaps are or how to load
them (`cmaps`: `{ baseUrl }`, `{ load(name) }`, or `false`), and, in
browsers, `worker: true` to run the core in a dedicated worker. There is no
engine selection: the Rust core is the engine.

The package exports the classes `PDFPlumberDocument`, `PDFPlumberPage`,
`Table`, `PDFPlumberError`, `NotImplemented`, `CoreCrash`,
`DocumentReleased`, `PSLiteral`, `PSKeyword`, `PDFObjRef`, `PDFStream`,
`PageRef`, `PyFloat`, `PyTuple` (and `pyFloat`, `pyTuple`), and the types
`PDFInput`, `OpenOptions`, `RuntimeOptions`, `PDFPlumberWarning`,
`PDFObject`, `Word`, `SearchResult`, `ExtractTextOptions`, `WordOptions`,
`SearchOptions`, `TextLineOptions`, `TextDirection`, `CropOptions`,
`DedupeOptions`, `TableOptions`, `TableStrategy`, `ExplicitTableLine`,
`TableAxisGroup`, `TableFinderResult`, `SerializeOptions`, `Bbox`,
`LAParams`.

## Asynchrony

The core is synchronous, but predefined CMaps (CJK fonts) are fetched lazily
and a browser can only fetch asynchronously; worker mode is asynchronous too.
So every member that runs the core returns a `Promise`: `pdf.pages`,
`pdf.metadata`, the object getters (`await page.chars`), `extractText()`,
`search()`, tables, the pages that derive a page (`crop`, `withinBbox`,
`outsideBbox`, `filter`, `dedupeChars`), serializations, `close()`. A
member never throws synchronously; an error is a rejected promise.

**Order.** The core is one Python process running one statement after the
other. Every member is one *job* of the core, queued when the member is
called (FIFO per core): its operations run in the order the members were
called, whatever order their promises are awaited in, each job to its end
(the re-runs after a missing CMap included) before the next one starts.
`search` (a `RegExp` run between two operations) and `filter` (the test
function run between two operations) are one job each: nothing else runs
in between.

**Laziness and caching.** pdfplumber makes its pages lazily, at the first
read of `pdf.pages` (an error of the page tree or of a page's boxes is
raised there, not by `pdfplumber.open`), and so does `await pdf.pages`:
`open()` resolves for a document whose pages Python cannot make. The API
keeps the array of the first read that resolves (the same
`PDFPlumberPage` objects every time, as Python returns its cached list)
until `close()` flushes it (`PDF.close()` flushes `_pages`: the next read
makes new pages); a rejected read is not kept (Python runs `pdf.pages`
again at the next read). Other getters run their statement at each read
(a later workstream may keep fulfilled values, never rejections: the
second run of a statement that raised can succeed, as in Python, where a
specialized site holds fewer C units).

A page's geometry (`page_number`, `bbox`, `width`, `height`, the boxes) is
known when its promise resolves (read by the same job); a geometry Python
cannot compute (a crop box kept as given with `strict: false`) throws when
`width` or `height` is read, as `page.width` raises in Python.

## Values

Objects are pdfplumber's dicts, keys in pdfplumber's order. Python values
map as follows (details in [`js-runtime.md`](js-runtime.md#valeurs)):

- `None` → `null`; `bool` → boolean; `str` → string (a lone surrogate is
  kept); `bytes` → `Uint8Array`; `list` and `tuple` → arrays.
- `float` → number, NaN, ±Infinity and `-0` included; `int` → number when
  within ±2^53, `bigint` beyond. An `int` and a `float` of the same value are
  the same number.
- pdfminer objects: `PSLiteral` (`name`), `PSKeyword`, `PDFObjRef`
  (`objid`), `PDFStream` (`objid`, `genno`, `attrs`, `rawdata`, `data`).
  Today a `PDFStream` carries its bytes: a result that holds images
  (`objects`, `images`, `to_dict`) copies each image's raw (and, once
  decoded, decoded) data out of the core every time. **Pending**
  (BOUNDARY): streams by reference, `rawdata` and `data` read on demand
  (contract below, "PDFStream").
- a pdfplumber `Page` held as a value (an annotation's `data.P`, a page of
  `getattr(pdf, "pages")`): `PageRef` (`handle`: the page in the core,
  the first live handle of that page, else a new one; `page_number`);
  `await pdf.page(ref)` gives the `PDFPlumberPage` (the same object as
  `pdf.pages` gives for that page).
- Integer-like keys (`"1"`) come first in a JavaScript object's key order,
  whatever their position in the Python dict.

### Arguments

JavaScript arguments become Python values as the boundary encodes them
(`js-runtime.md`, "Valeurs"): a safe integer is an `int`, any other number
a `float` (`-0` included), a `bigint` an `int`, `PyFloat(x)` a `float`,
`PyTuple([...])` a `tuple`, an array a `list`, a plain object or a `Map`
with string keys a `dict` (properties `undefined` are left out: an option
`undefined` is not given), `Uint8Array`/`ArrayBuffer` `bytes`, a string a
`str`, `null`/`undefined` `None`. A container holding itself is a
JavaScript `TypeError`; a function, a symbol or a key that is not a string
too. Nothing else is checked on the JavaScript side: a value Python
refuses (`x_tolerance: "1"`, `bbox: ["a", 0, 10, 10]`) raises Python's
error where Python raises it.

An object of a result given back as an argument (`explicit_*_lines` of a
table setting, which pdfplumber reads `x0`, `top`... of) crosses by value:
an integral float crosses as an `int` (the same JavaScript number in every
result; visible only in texts, `to_json`) and pdfminer objects inside it
cross as dicts until BOUNDARY decodes them as the objects they stand for.

### Aliases

Every pdfplumber name with an underscore has a camelCase alias, and the
reverse: `extractText`/`extract_text`, `extractTextSimple`/
`extract_text_simple`, `extractWords`/`extract_words`,
`extractTextLines`/`extract_text_lines`, `withinBbox`/`within_bbox`,
`outsideBbox`/`outside_bbox`, `dedupeChars`/`dedupe_chars`,
`findTables`/`find_tables`, `findTable`/`find_table`,
`extractTables`/`extract_tables`, `extractTable`/`extract_table`,
`debugTablefinder`/`debug_tablefinder`, `toDict`/`to_dict`,
`toJson`/`to_json`, `toCsv`/`to_csv`, `structureTree`/`structure_tree`,
`horizontalEdges`/`horizontal_edges`, `verticalEdges`/`vertical_edges`,
`rectEdges`/`rect_edges`, `curveEdges`/`curve_edges`,
`pageNumber`/`page_number`, `initialDoctop`/`initial_doctop`. Both names
are one member (one implementation); option names and object keys stay
pdfplumber's (snake_case).

## Document (`PDFPlumberDocument`)

| Member | Python |
|---|---|
| `pages` (promise) | `pdf.pages` |
| `metadata` | `pdf.metadata` |
| `objects` | `pdf.objects` |
| `chars`, `rects`, `lines`, `curves`, `images`, `edges`, `rect_edges`, `curve_edges`, `horizontal_edges`, `vertical_edges`, `textboxhorizontals`, `textboxverticals`, `textlinehorizontals`, `textlineverticals` | `getattr(pdf, name)` |
| `annots`, `hyperlinks` | `pdf.annots`, `pdf.hyperlinks` |
| `structure_tree` | `pdf.structure_tree` (the elements' `to_dict()`) |
| `structTree(page?)` **pending** | `PDFStructTree(pdf, page)` |
| `toDict(objectTypes?)` | `pdf.to_dict(object_types)` |
| `toJson(options?)`, `toCsv(options?)` | `pdf.to_json(**options)`, `pdf.to_csv(**options)` (`stream: null`: the text) |
| `page(ref)` | the page of a `PageRef` |
| `close()` | `pdf.close()` |
| `release()` | (no Python counterpart: frees the core's document) |
| `[Symbol.asyncDispose]()` | `close()` then `release()` (`with pdfplumber.open(...)`) |
| `options` | the options of `open()` |

## Page (`PDFPlumberPage`)

Geometry (synchronous): `page_number`, `initial_doctop`, `rotation`,
`mediabox`, `cropbox`, `trimbox`, `bleedbox`, `artbox` (absent when the
page has none), `bbox`, `width`, `height`; `pdf` (the document), `parent`
(the page a derived page derives from); `toString()` is `repr(page)`
(`<Page:1>`).

All object coordinates use pdfplumber's top-left page coordinate convention:
`x0`, `x1`, `y0`, `y1`, `top`, `bottom`, `doctop`, `width`, and `height`.

| Member | Python |
|---|---|
| `objects` (promise) | `page.objects` |
| `chars` ... `textlineverticals`, `annots`, `hyperlinks` (the document's list) | `getattr(page, name)` |
| `structure_tree` | `page.structure_tree` |
| `extractText(options?)` | `page.extract_text(**options)` |
| `extractTextSimple(options?)` | `page.extract_text_simple(**options)` |
| `extractWords(options?)` | `page.extract_words(**options)` |
| `extractTextLines(options?)` | `page.extract_text_lines(strip, return_chars, **options)` |
| `search(pattern, options?)` | `page.search(pattern, **options)` (JavaScript `RegExp` semantics, below) |
| `crop(bbox, options?)`, `withinBbox`, `outsideBbox` | `page.crop(bbox, **options)`... |
| `filter(test)` | `page.filter(test)` (below) |
| `dedupeChars(options?)` | `page.dedupe_chars(**options)` |
| `findTables(settings?)`, `findTable`, `extractTables`, `extractTable` | `page.find_tables(table_settings)`... |
| `debugTablefinder(settings?)` **pending** | `page.debug_tablefinder(table_settings)` |
| `toDict(objectTypes?)`, `toJson(options?)`, `toCsv(options?)` | `page.to_dict(object_types)`... |
| `close()` | `page.close()` (flushes the page's caches; it stays usable) |

Text and word options follow pdfplumber's keyword arguments, including
layout options, tolerances, text direction, punctuation splitting, flow
ordering, `return_chars`, `extra_attrs`.

### Search

`search()` takes a JavaScript `RegExp` or a string, with **JavaScript
`RegExp` semantics**: Python's `re` is not ported. The text the pattern runs
over (`TextMap.as_string`) and every result (text, bbox, groups, chars) are
pdfplumber's; only the matching differs. How a pattern is run:

- A `RegExp` keeps its flags; `g` and `d` (match indices) are added and `y`
  (sticky) removed. As in pdfplumber, `regex: false` or `case: false` with a
  `RegExp` rejects with `ValueError`, once the text map is computed
  (`TextMap.search` raises it after `get_textmap`).
- A string is compiled with the `u` flag (and `i` when `case: false`),
  escaped first when `regex: false`.
- Matches come from `String.prototype.matchAll`; pdfplumber then drops the
  empty and whitespace-only ones, as it does in Python.
- `main_group` is read for each match as Python's `m.group(main_group)`
  reads it: a group number (a boolean counts as 0 or 1), or the name of a
  named group (`(?<name>…)`); a number out of range or an unknown name
  rejects with `IndexError` (only when there is a match), a group that did
  not take part `AttributeError`, as in Python. The core converts
  JavaScript's UTF-16 offsets to Python's code point offsets; a match that
  starts or ends inside a surrogate pair (a pattern without `u` matching half
  of an astral character) rejects with `ValueError`.

Differences with Python's `re`, which the same pattern can hit:

- Syntax: no `(?P<name>…)` (JavaScript writes `(?<name>…)`), no inline
  flags such as `(?i)` in older engines, no possessive quantifiers or atomic
  groups; `\Z`, `\A` do not exist (`$` and `^`).
- Classes: in Python (`str` patterns) `\w`, `\d`, `\s` and `\b` are Unicode;
  in JavaScript they are ASCII-only (`\s` excepted), even with `u`. Use
  `\p{L}`, `\p{Nd}`... for Unicode classes.
- `$`: Python's `$` also matches before a final newline; JavaScript's does
  not (without `m`).
- Case-insensitive matching follows each engine's case folding (Python's
  `re.IGNORECASE` and JavaScript's `iu` differ on a few characters, such as
  `ſ` and the Kelvin sign).
- Empty matches: after an empty match, Python's `finditer` looks for a
  non-empty match at the same position (`x*|abc` on `abc` finds `abc`);
  JavaScript moves on one position. Empty matches themselves are dropped by
  pdfplumber.
- A Python `str` holding a high and a low surrogate as two code points is,
  in JavaScript, one astral character.

### Cropping, Filtering, And Dedupe

The derived pages (`crop`, `withinBbox`, `outsideBbox`, `filter`,
`dedupeChars`) are kept by the core; crop options are `relative` and
`strict`, dedupe options `tolerance` and `extra_attrs`. An error Python
raises at the call (a crop box outside the page) rejects the promise.

`filter(testFunction)` runs the function on the parent page's objects
(`page_objects`, which computes them), in the same job, and hands the core
the truth of each result (JavaScript's truthiness); the derived page's test
function looks each object up there. Python calls the function later, when
the derived page's objects are computed, on the same objects: the results
are the same, unless the parent's cache is flushed in between (`close()`),
which the core reports as `NotImplemented` (its objects are new ones). A
function that throws rejects the `filter()` promise (Python raises at the
first read of the derived page's objects).

## Tables

`findTable()` and `findTables()` return `Table` objects with `bbox`,
`cells`, `rows` and `columns` (`TableAxisGroup`: `bbox` and `cells`, `null`
where a row or column has no cell), `page` (the page it was found on) and
`extract(options?)` (`Table.extract(**options)`). `extractTable()` and
`extractTables()` return arrays of cell strings or `null` values.

Supported table settings include `vertical_strategy` and
`horizontal_strategy` (`lines`, `lines_strict`, `text`, `explicit`),
`explicit_vertical_lines` and `explicit_horizontal_lines`, snap, join,
intersection, edge-min-length and text tolerances, minimum word thresholds,
and `text_*` options forwarded to cell text extraction. `settings` absent
is `table_settings=None`; any value is resolved as `TableSettings.resolve`
resolves it (Python's `ValueError` for anything else).

**Pending** (BOUNDARY `page_debug_tablefinder`): `debugTablefinder(settings)`
resolves to a `TableFinderResult`: `edges`, `intersections` (the items of
Python's dict: `[[x, top], {v, h}]`), `cells`, `tables` (`Table` objects),
`settings` (the resolved `TableSettings` as a dict).

## Structure Tree

`structure_tree` (document and page) is pdfplumber's list of the elements'
`to_dict()`. **Pending** (BOUNDARY, `struct_*` operations):
`pdf.structTree(page?)` resolves to a `PDFStructTree` (`PDFStructTree(pdf,
page)`; `StructTreeMissing` rejects it): `children` (`PDFStructElement`
objects: `type`, `revision`, `id`, `lang`, `alt_text`, `actual_text`,
`title`, `page_number`, `attributes`, `mcids`, `children`), `findAll(matcher)`
and `find(matcher)` (a tag name, a `RegExp` run on the element's type as
`re.match` does — JavaScript semantics, anchored at the start — or a
function of an element, run on every element in Python's order, in the
same job, like `filter`), `elementBbox(el)`; an element's `allMcids()`
(`[[page_number, mcid], ...]`) and `repr()` (`repr(el)`).

## PDFStream (pending, BOUNDARY)

A `PDFStream` in a result is a reference to the stream in the core:
`objid`, `genno` and `attrs` are known (values); `rawdata` and `data` are
promises reading the stream as it is when they are read (`stream.rawdata`,
`stream.data`: `null` once decoded, resp. before), `getData()` and
`getRawdata()` are `stream.get_data()` (which decodes, `rawdata` then
`null`) and `stream.get_rawdata()`. The same stream is the same
`PDFStream` object while it is alive; the reference lives as long as the
document (released with it, or when unreachable).

## Lifetime

- `close()` is Python's `PDF.close()` (`Page.close()` for a page): caches
  flushed, a file opened by path closed; the document stays usable, as in
  Python.
- `release()` frees the document in the core (its pages, derived pages,
  tables and streams, and the history of its calls); idempotent. Any later
  member that needs the core rejects with `DocumentReleased` (a JavaScript
  error: Python has no released object); the geometry of a page stays
  readable.
- `await using pdf = await open(...)` (`Symbol.asyncDispose`) is
  `close()` then `release()`.
- A document that becomes unreachable is released by a
  `FinalizationRegistry` (where the engine has one): a page or a table
  holds its document. Releasing never changes a result (the core keeps no
  Python-visible state of a released document); the order of the release
  among other calls is the garbage collector's.
- After a `CoreCrash`, the documents of the crashed core are lost; the next
  `open()` starts a new core.

## Worker Mode

`configure({ worker: true })` (browsers) runs the core in a dedicated
worker: the same API, the same results. Requests are encoded and results
decoded in the calling thread (the classes survive); jobs are queued in
the calling thread; the test functions of `filter` and the `RegExp` of
`search` run in the calling thread, between the operations of their job.
CMaps are fetched by the worker (`cmaps.baseUrl`; a loader function cannot
cross to a worker).

## Operations

Each member runs the operations of `crates/wasm/src/pages.rs` below, in
one job; each operation is one statement at the top level of a Python
script (`oracle/unit/js_pages_run.py`): this is what the sessions of
`oracle/unit/js_api.py` run in Python to give the expected results, and
where a `RecursionError` happens through the API.

| Member | Operations (statements) |
|---|---|
| `open(bytes)` / `open(path)` | `doc_open` (`pdfplumber.open(io.BytesIO(data), **options)`) / `doc_open_file` (`pdfplumber.open(path, **options)`) |
| `pdf.pages` | `doc_pages` (`obj.pages`, then each page's geometry) at the first read and after `close()`; none otherwise |
| `pdf.metadata`, `objects`, `annots`, `hyperlinks`, `structure_tree` | `doc_metadata`, `doc_objects`, `doc_annots`, `doc_hyperlinks`, `doc_structure_tree` (a property load at a site of its own) |
| `pdf.chars` ... | `doc_attr` (`getattr(obj, name)`) |
| `pdf.toDict`, `toJson`, `toCsv` | `doc_to_dict`, `doc_to_json`, `doc_to_csv` (`obj.to_dict(**kwargs)`...) |
| `pdf.close()` | `doc_close` (`obj.close()`) |
| `pdf.release()` | `free` (no statement) |
| `pdf.page(ref)` | none for a known page, else `page_info` |
| `page.objects`, `structure_tree` | `page_objects`, `page_structure_tree` |
| `page.chars` ..., `annots`, `hyperlinks` | `page_attr` (`getattr(obj, name)`) |
| `extractText`, `extractTextSimple`, `extractWords`, `extractTextLines` | `page_extract_text`, `page_extract_text_simple`, `page_extract_words`, `page_extract_text_lines` (`obj.method(*args, **kwargs)`) |
| `search` | `page_search_text` then `page_search` (one statement, `obj.search(pattern, **options)`; JavaScript runs its `RegExp` between the two) |
| `crop`, `withinBbox`, `outsideBbox`, `dedupeChars` | `page_crop`... `page_dedupe_chars`, then `page_info` of the derived page (`obj.width`, `obj.height` read at sites of their own) |
| `filter` | `page_objects`, `page_filter` (`mask_of(page, ...)`: the mask), `page_info` |
| `findTables`, `findTable`, `extractTables`, `extractTable` | `page_find_tables`... (`obj.find_tables(*args)`) |
| `table.extract` | `table_extract` (`obj.extract(**kwargs)`) |
| `page.toDict`, `toJson`, `toCsv`, `close` | `page_to_dict`, `page_to_json`, `page_to_csv`, `page_close` |
| `debugTablefinder` (pending) | `page_debug_tablefinder` (`obj.debug_tablefinder(*args)`) |
| `structTree`, `findAll`, `find`, `elementBbox`, `allMcids`, `repr` (pending) | `struct_tree`, `struct_find_all`, `struct_find`, `struct_element_bbox`, `struct_all_mcids`, `struct_element_repr` |
| `PDFStream.rawdata`, `data`, `getData`, `getRawdata` (pending) | `stream_rawdata`, `stream_data`, `stream_get_data`, `stream_get_rawdata` |

Depth: each operation of the core runs as one statement at the top level of
a Python script (a property read at a site of its own, a method called with
`**options`), so a `RecursionError` (a PDF nested near Python's limits)
happens where it happens in that script. A statement written otherwise in
Python (explicit keyword arguments instead of `**options`, a property read
at a new site, a call inside a function) takes up to two C recursion units
more or less, or more frames: it shows only at the recursion limits
(`js-runtime.md`, "Documents et pages").

## Not Ported

These parts of pdfplumber 0.11.9 are out of scope (`docs/architecture.md`,
"Règles de port"):

- `Page.to_image` and `pdfplumber/display.py` (rendering through pypdfium2
  and PIL).
- `pdfplumber.open(..., repair=True)`, `gs_path`, `repair_setting` and
  `pdfplumber/repair.py` (Ghostscript run in a subprocess): the core raises
  `NotPorted` (`NotImplemented` in JavaScript).
- The command line (`pdfplumber/cli.py`).

Not exposed (Python objects with no JavaScript form): `page.layout` (the
`LTPage`: its text boxes and lines are `page.objects` with `laparams`),
`pdf.doc` (pdfminer's `PDFDocument`), `page.page_obj`, `get_textmap`,
`flush_cache` (`close()` flushes), `process_object`,
`iter_layout_objects`, `parse_objects`, `point2coord`.

Inline images: pdfminer names an inline image `str(id(stream))`, a memory
address; the port names it `inline-<k>` (`k`: its rank among the page's
inline images). This name is the `name` of the image in `page.images` and,
with `laparams`, of its figure. It is unique within one interpretation of
one page only: every page (and a page interpreted again after `close()`)
starts again at `inline-0`, so `pdf.images` and `pdf.objects` repeat names
across pages, where Python's addresses differ for images alive at the
same time. An image XObject that a PDF names `inline-0` gets the same
name as the page's first inline image.

Layout analysis: where boxes are at equal distance, pdfminer's analysis
breaks ties by `id()` (memory addresses, which change from one Python run
to another); the port ranks them by creation order (`CreationOrder`),
which gives one of Python's outcomes.

Not modelled (the core raises `NotPorted` where Python would go on):

- a `laparams` margin (`line_overlap`, `char_margin`, `line_margin`,
  `word_margin`) that is not a number: Python stores any value and fails
  at its first use in the layout analysis;
- a `password` that is neither `null` nor a string, given for an
  encrypted document (an unencrypted one never reads it, in Python as in
  the port);
- `to_dict`/`to_csv` with an `object_types` entry that names a method
  (`"extract_word"` gives `extract_words`): Python serializes the bound
  method's `str()`;
- `to_json`/`to_csv` into a stream: `stream: null` (Python's default)
  returns the text, as Python does; any other stream is not modelled.

Not modelled either, without an error: the order Python gives a NaN
coordinate in the sets of `cluster_objects` (words, lines, table edges)
follows its memory address, which the port cannot reproduce (it puts NaN
values last).

Structure tree, metadata and annotations (`docs/architecture.md`,
"Vague 4", workstream DOC):

- a cycle among the structure elements that a page's parent tree reaches
  makes pdfplumber's `PDFStructTree(pdf, page)` (and `page.structure_tree`)
  loop until memory runs out; the core raises `MemoryError` once the walk
  only repeats itself;
- `structure_tree` deep-copies the attribute values of the elements
  (`dataclasses.asdict`): for a document read from bytes (`io.BytesIO`,
  every input but a Node path), a reference among them copies the whole
  document in Python, which the core does not do (it keeps the reference,
  an equal value) nor count in the recursion depth; for a document opened
  from a path, Python and the core raise the same `TypeError` (the file
  object cannot be copied). What it changes, from JavaScript: with a
  reference among an element's attributes, Python raises `RecursionError`
  where the core returns the tree, for a chain of elements some 320 levels
  deep (instead of the deeper limit of `asdict` alone), and even for a
  shallow tree when the document holds a direct object nested a few
  hundred levels deep (the catalog, `/Info`, a cross-reference trailer,
  any object read so far: the copy walks them all);
- pdfplumber decodes the metadata in place, in the document's own
  dictionaries: whatever an `/Info` value reaches (the catalog, a page, an
  annotation, a structure element) is read decoded afterwards, by the
  pages, the structure tree and the annotations. The core does the same.

## Runtime Contract

The package is ESM. Node loads `dist/src/index.js`, browsers
`dist/browser/index.js` (bundlers pick it through the `browser` export
condition); both load `dist/pdfplumber.wasm` and fetch or read
`dist/cmaps/` next to themselves, unless `configure()` says otherwise.
Worker mode uses `dist/browser/worker.js`. The same runtime tests and the
same API sessions run in Node, Chromium, Firefox and WebKit
([`js-runtime.md`](js-runtime.md#tests)).

## Errors

An exception raised by the core is a `PDFPlumberError` (an `Error`) whose
`name` is the Python class name and `message` is `str(e)`, as pdfplumber
raises it, and whose `cause` is the exception it wraps (pdfplumber's
`PdfminerException(e)` and `MalformedPDFException(e)`: the pdfminer
exception, itself a `PDFPlumberError`), absent otherwise:

- `PdfminerException`: `open()` on an empty, invalid or unreadable PDF, and
  on an encrypted PDF with a missing or wrong password (pdfplumber wraps
  every exception of pdfminer's `PDFDocument` in it; `cause` is that
  exception: `PDFSyntaxError`, `PDFPasswordIncorrect`...).
- `MalformedPDFException`, `PDFSyntaxError`, `PSEOF`... and Python builtins
  (`ValueError`, `TypeError`, `KeyError`...) raised later by pdfminer or
  pdfplumber.

Every `PDFPlumberError` carries `pythonModule`, `pythonQualname` and
`pythonMro` (the class's `__mro__`, as `module.qualname` strings);
`error.isInstance("PDFException")` answers like Python's `isinstance`, by
class name or qualified name.

Other errors:

- `NotImplemented` (a `PDFPlumberError`): a member whose boundary
  operation has not landed yet, or a core path marked "not ported"
  (`NotPorted`, whose module and class it keeps).
- `DocumentReleased`: a member of a released document (`release()`,
  `await using`, `FinalizationRegistry`).
- `CoreCrash`: the WebAssembly core stopped on an internal error (a bug);
  its documents are lost, the next `open()` starts a new core.
- `Error` and `TypeError`: input adaptation failures (a missing file, a
  failed HTTP fetch, an unsupported input type, an argument that contains
  itself), and failures to load the module or a CMap file.

Callers should branch on `error.name` (or `isInstance`) and treat the message
as diagnostic text.
