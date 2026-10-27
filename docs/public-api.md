# Public Extraction API

`pdfplumber.js` exposes a browser-capable JavaScript API modeled on Python
`pdfplumber` 0.11.9. The extraction engine is a faithful Rust port of
pdfplumber and pdfminer.six compiled to WebAssembly (`crates/`); results are
those of Python pdfplumber (CPython ≥ 3.12), verified against it through
this API itself. Callers use the same `open()` entry point in Node and
browsers. How the runtime works (WebAssembly loading, result encoding,
CMaps, workers) is described in [`js-runtime.md`](js-runtime.md); the
contract of wave 5, its workstreams and its harness in
[`architecture.md`](https://github.com/AxENSRennes/pdfplumber.js/blob/HEAD/docs/architecture.md) ("Vague 5").

> **Status (wave 5): implemented.** Every member below is implemented over
> the operations of the core and checked against Python through this API:
> by the sessions of `oracle/unit/js_api.py` (every member, option, alias,
> error path and lifetime case, on real and adversarial PDFs; replayed in
> Node on the sources and on the dist build, and in Chromium, Firefox and
> WebKit, in the page and in worker mode; and the depth windows of the
> adversarial sets, with each family's window at the API's own
> `RecursionError` limit, in Node), by `js/test/node/api-lifetime.test.ts`
> (identities and the garbage collector) and `api-runtime.test.ts`, and by
> the four `plumber_*` stage traces written through this API
> (`oracle.verify --engine js-api`) on `all` and the adversarial sets.
> `NotImplemented` remains only for the paths marked "not ported" below.

## Differences From Python

Results are Python pdfplumber's everywhere else. These differences are
known, deliberate or out of reach (they depend on CPython memory
addresses), and each is described in its section:

- **`filter(test)` evaluates `test` once, when it is called**, on the
  parent page's objects. Python calls it later, at the first read of the
  derived page's objects. So a closure variable changed in between is not
  seen, a test with side effects runs earlier, and an error (a test that
  throws, or parent objects that cannot be computed because of a corrupt
  content stream) rejects the `filter()` promise itself, where Python
  returns the page and raises at its first read. Same error, earlier.
  ([Cropping, Filtering, And Dedupe](#cropping-filtering-and-dedupe))
- **`search()` and structure-tree matchers use JavaScript `RegExp`
  semantics**, not Python's `re`: ASCII `\w`/`\d`/`\b`, no `(?P<name>…)`,
  `$` before a final newline... ([Search](#search))
- **Matcher functions of the structure tree run on every element**, even
  where Python stops at the first match. ([Structure Tree](#structure-tree))
- **Layout ties**: where boxes are at equal distance, pdfminer breaks the
  tie by memory address (which changes from one Python run to another);
  the port uses creation order, which gives Python's result on 98.6 % of
  the corpus pages where Python itself is stable from run to run.
  ([Not Ported](#not-ported))
- **Inline image names** are `inline-<k>` (rank in the page) instead of a
  memory address, so they repeat across pages. ([Not Ported](#not-ported))
- **NaN coordinates** come last in the sets of `cluster_objects` (words,
  lines, table edges); Python orders them by memory address.
  ([Not Ported](#not-ported))
- **`structure_tree` of a document read from bytes** does not deep-copy
  the whole document behind a reference attribute, so a very deep tree
  that makes Python raise `RecursionError` is returned.
  ([Not Ported](#not-ported))
- **Not modelled inputs** raise `NotImplemented` where Python would go on
  (a non-numeric `laparams` margin, a non-string password, serializing into
  a stream...), and `to_image`, `repair=True` and the command line are not
  ported. ([Not Ported](#not-ported))

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

`open()` options (`OpenOptions`, a plain object or a `Map`; `null` is no
option): every option is a keyword argument of `pdfplumber.open`, as given,
except the API's own `logLevel` and `onWarning`:

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
filters' registry), and of pdfminer's `log.warning` calls (malformed
operators, page boxes, fonts, xref entries, corrupt streams...: `source`
is the logger name, `pdfminer.pdfinterp`..., the text `msg % args` as
`logging` formats it; `js-runtime.md`, "Diagnostics"); pdfminer's
`log.debug` records (one per token, operator or object read) are not
forwarded. Diagnostics never change a result. Python prints warnings to
stderr by default; the JavaScript API is silent by default.

Runtime settings, before the first `open()` (`configure(options)`,
`RuntimeOptions`): where the WebAssembly module is (`wasm`: URL, path,
bytes or compiled module), where the predefined CMaps are or how to load
them (`cmaps`: `{ baseUrl }`, `{ load(name) }`, or `false`), and, in
browsers, `worker: true` to run the core in a dedicated worker. There is no
engine selection: the Rust core is the engine.

The package exports the classes `PDFPlumberDocument`, `PDFPlumberPage`,
`Table`, `PDFStructTree`, `PDFStructElement`, `PDFPlumberError`,
`NotImplemented`, `CoreCrash`,
`DocumentReleased`, `PSLiteral`, `PSKeyword`, `PDFObjRef`, `PDFStream`,
`PageRef`, `PyFloat`, `PyTuple` (and `pyFloat`, `pyTuple`), and the types
`PDFInput`, `OpenOptions`, `RuntimeOptions`, `PDFPlumberWarning`,
`PDFObject`, `Word`, `SearchResult`, `ExtractTextOptions`, `WordOptions`,
`SearchOptions`, `TextLineOptions`, `TextDirection`, `CropOptions`,
`DedupeOptions`, `TableOptions`, `TableStrategy`, `ExplicitTableLine`,
`TableAxisGroup`, `TableFinderResult`, `StructMatcher`, `SerializeOptions`, `Bbox`,
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
again at the next read). Each read is a job, decided in the order of the
calls: `pdf.close()` then `pdf.pages`, the first not awaited yet, gives
the pages `close()` makes again, as Python's `pdf.close(); pdf.pages`. Other getters run their statement at each read
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
  (`objid`), `PDFStream` (`objid`, `genno`, `attrs`, and its data read on
  demand: a reference to the stream in the core, below, "PDFStream").
- a pdfplumber `Page` held as a value (an annotation's `data.P`: pdfplumber
  sets it to the page whose `annots` were read, a cropped page for a
  cropped page's): `PageRef` (`handle`: the page in the core, the first live
  handle of that page, else a new one; `page_number`); `await
  pdf.page(ref)` gives its `PDFPlumberPage`: the same object as `pdf.pages`
  gives for that page (as `annot["data"]["P"] is pdf.pages[i]` in Python),
  or as the derived page it names, for a reference read since the last
  `close()` (one read before names a page of the list `close()` flushed: a
  page object of its own). `pdf.page()` takes the references of `pdf`'s
  own results only: one of another document (or of a core of before
  `configure()` or a crash, or made with `new PageRef(...)`) is a
  `TypeError` (its handle names a page of another document, or nothing).
- Integer-like keys (`"1"`) come first in a JavaScript object's key order,
  whatever their position in the Python dict.

### Arguments

JavaScript arguments become Python values as the boundary encodes them
(`js-runtime.md`, "Valeurs"): a safe integer is an `int`, any other number
a `float` (`-0` included), a `bigint` an `int`, `PyFloat(x)` a `float`,
`PyTuple([...])` a `tuple`, an array a `list`, a plain object (`{...}`,
`Object.create(null)`) or a `Map` with string keys a `dict` (values
`undefined` are left out, in both: an option `undefined` is not given),
`Uint8Array`/`ArrayBuffer` `bytes`, a string a `str`, `null`/`undefined`
`None`. A container holding itself is a JavaScript `TypeError`; a
function, a symbol or a key that is not a string too, and so is any other
object (a `Set`, a `Date`, a `Float64Array`, an instance of a class of
the caller's...: no Python value; never an empty `dict` of its
properties: `Array.from(set)` gives a `list`). Nothing else is checked on
the JavaScript side: a value Python refuses (`x_tolerance: "1"`, `bbox:
["a", 0, 10, 10]`) raises Python's error where Python raises it.

`null` is `None`, given: the options a method takes itself (`strip` and
`return_chars` of `extractTextLines`, `regex`, `case`, `main_group`,
`return_chars`, `return_groups` of `search`) have their Python default
only when they are `undefined`; `strip: null` is `strip=None` (false),
`main_group: null` is `m.group(None)` (`IndexError`).

An object of a result given back as an argument (`explicit_*_lines` of a
table setting, which pdfplumber reads `x0`, `top`... of; a word's
coordinates as a `bbox`) crosses by value: an integral float crosses as an
`int` (the same JavaScript number in every result; visible only in texts,
`to_json`), a tuple as a list, and the pdfminer objects inside it as the
objects they stand for (`PSLiteral` and `PSKeyword` interned again,
`PDFObjRef` bound to the document of the call's target, `PageRef` and a
`PDFStream` by reference as the core's page and stream, whatever document
of the core they belong to, as Python takes any object). A `PageRef` or a
`PDFStream` of a core of before (`configure()`, a `CoreCrash`) names
nothing in the current one: a `TypeError`.

Options objects (`extractText(options)`, `search(pattern, options)`...)
are dicts as well: a plain object or a `Map`; a value that is not a mapping
(`null`) goes to Python as it is, which raises its `TypeError` ("argument
after ** must be a mapping"), except for `open()`, where `null` is no
option.

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
`pageNumber`/`page_number`, `initialDoctop`/`initial_doctop`, and on the
structure tree and streams `findAll`/`find_all`, `elementBbox`/
`element_bbox`, `allMcids`/`all_mcids`, `getData`/`get_data`,
`getRawdata`/`get_rawdata` (an element's `alt_text`, `actual_text`,
`page_number` fields have the getters `altText`, `actualText`,
`pageNumber`). Both names are one member (one implementation); option
names and object keys stay pdfplumber's (snake_case).

## Document (`PDFPlumberDocument`)

| Member | Python |
|---|---|
| `pages` (promise) | `pdf.pages` |
| `metadata` | `pdf.metadata` |
| `objects` | `pdf.objects` |
| `chars`, `rects`, `lines`, `curves`, `images`, `edges`, `rect_edges`, `curve_edges`, `horizontal_edges`, `vertical_edges`, `textboxhorizontals`, `textboxverticals`, `textlinehorizontals`, `textlineverticals` | `getattr(pdf, name)` |
| `annots`, `hyperlinks` | `pdf.annots`, `pdf.hyperlinks` |
| `structure_tree` | `pdf.structure_tree` (the elements' `to_dict()`) |
| `structTree(page?)` | `PDFStructTree(pdf, page)` (`page`: a page or derived page of this document) |
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
| `debugTablefinder(settings?)` | `page.debug_tablefinder(table_settings)` |
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
- Any other pattern (a number, `null`, bytes, an array...) is Python's:
  the core raises the `TypeError` Python's `re` raises for it, once the
  text map is computed (`re.compile`'s "first argument must be string or
  compiled pattern", "unhashable type: 'list'", or with `regex: false`
  `re.escape`'s "decoding to str: ..."; bytes: "cannot use a bytes pattern
  on a string-like object"). `search()` without a pattern rejects with
  Python's `TypeError` of the missing argument, before any statement.
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
the derived page's objects are computed, on the same objects. **This is a
difference**: the function runs once, when `filter()` is called; what it
reads that changes afterwards is not seen (a closure variable changed
between `filter()` and the first read gives other results than Python's,
with no error), and a function with side effects runs at another time
than Python's. A parent's cache flushed in between (`close()`) is
reported by the core as `NotImplemented` (its objects are new ones). A
function that throws rejects the `filter()` promise (Python raises at the
first read of the derived page's objects). So do the parent's objects
when they cannot be computed (a corrupt content stream...): `filter()`
rejects with their error, where Python's `page.filter()` returns the
`FilteredPage` and raises that error at the first read of its objects
(`objects`, `chars`, `extract_text`...); its `bbox`, `width`, `height`
are the parent's.

A test that is not a function is Python's: `filter(null)` is
`page.filter(None)` (the builtin `filter` keeps the true objects: every
object), any other value (`filter(5)`) is not callable, Python's
`TypeError` (`'int' object is not callable`) at the first object, when
`filter()` is called (none for a page without objects); `filter()`
without an argument is Python's missing-argument `TypeError`.

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

`debugTablefinder(settings)` resolves to a `TableFinderResult`: `edges`,
`intersections` (the items of Python's dict: `[[x, top], {v, h}]`),
`cells`, `tables` (`Table` objects), `settings` (the resolved
`TableSettings`, its fields as a dict).

## Structure Tree

`structure_tree` (document and page) is pdfplumber's list of the elements'
`to_dict()`. `pdf.structTree(page?)` resolves to a `PDFStructTree`
(`PDFStructTree(pdf, page)`, `page` a page or a derived page of the
document; `StructTreeMissing` rejects it): `pdf`, `page` (`null` for the
document's tree), `children` (`PDFStructElement` objects: the dataclass's
fields `type`, `revision`, `id`, `lang`, `alt_text`, `actual_text`,
`title`, `page_number`, `attributes`, `mcids`, and `children`; an element
reached twice in a tree that is not a tree, a DAG, is two objects),
`findAll(matcher)` and `find(matcher)`, and `elementBbox(el)`. An element
has `findAll(matcher)` and `find(matcher)` over its subtree, `allMcids()`
(`list(el.all_mcids())`: `[[page_number, mcid], ...]`), `repr()`
(`repr(el)`) and `toDict()`/`to_dict()` (`el.to_dict()`: the compacted
dict `structure_tree` gives for each element). A tree and an element are
iterable: `for (const el of tree)` goes over its children
(`__iter__`). A matcher is a tag name, a `RegExp` run on each element's
type as `re.match` runs (JavaScript semantics, anchored at the start: the
pattern runs sticky from 0, `g` and `y` dropped), or a function of an
element; a `RegExp` or a function is run on every element of the tree, in
the order of `_find_all` (depth-first, preorder), in the member's job (as
`filter`'s), even where Python stops at the first match (`find`) or walks a
subtree only: a function with side effects sees every element. Any other
matcher reaches Python as it is (`TypeError: '...' object is not
callable`), a plain object included, whatever its keys (`{ mask: [...] }`
is a `dict` Python calls, not the truth the API computes for a function);
an object with no Python value (a `Set`, an element...) is the encoder's
JavaScript `TypeError` ("Arguments"). An element found is the element object of the tree at its
path. `elementBbox` takes an element of this tree (`TypeError` otherwise;
Python accepts any `PDFStructElement`).

Each tree, like a derived page or a table, lives in the core while it is
reachable (below, "Lifetime").

## PDFStream

A `PDFStream` in a result is a reference to the stream in the core
(workstream BOUNDARY, `js-runtime.md`):
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
  `FinalizationRegistry` (where the engine has one): a page, a table, a
  structure tree or a stream holds its document (and nothing of the
  runtime holds a document: the streams it read are kept by document, in a
  `WeakMap`). Releasing never changes a result (the core keeps no
  Python-visible state of a released document); the order of the release
  among other calls is the garbage collector's.
- A derived page, a table or a structure tree that becomes unreachable has
  its handle freed in the core (`free`, after the calls made before), while
  its document lives (never once the document is released: its handles may
  name other objects then). Finalizers run in tasks of their own: a loop
  that awaits only the API's promises (`for (...) { const c = await
  page.crop(b); await c.extractText(); }`) never gives the event loop a
  turn, so every 64 objects tracked this way the next job first waits for
  one (a task: `setImmediate`, a `MessageChannel` message), which lets the
  finalizers of the objects collected by then run; their frees go after it
  in the queue. This changes no result, and bounds what such a loop keeps
  in the core (Python frees each `CroppedPage` by reference counting). A
  derived page whose annotations were read
  (`annots`, `hyperlinks`, `toDict`: their `data.P` names it) is kept by its
  document instead, so that `pdf.page(ref)` gives it back. The pages of
  `pdf.pages` are kept until `close()` (and the pages `pdf.page(ref)` made,
  too).
- After a `CoreCrash`, the documents of the crashed core are lost; the next
  `open()` starts a new core (in worker mode, the crashed core's worker is
  stopped at once, its memory with it).

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

- `NotImplemented` (a `PDFPlumberError`): a core path marked "not ported"
  (`NotPorted`, whose module and class it keeps; "Not Ported" above).
- `DocumentReleased`: a member of a released document (`release()`,
  `await using`, `FinalizationRegistry`).
- `CoreCrash`: the WebAssembly core stopped on an internal error (a bug);
  its documents are lost, the next `open()` starts a new core.
- `Error` and `TypeError`: input adaptation failures (a missing file, a
  failed HTTP fetch, an unsupported input type, an argument that contains
  itself, a function, a symbol or an object that is neither a plain object
  nor a `Map` (a `Set`, a `Date`...) in an argument, a `PageRef` or a
  `PDFStream` of a core of before in an argument, `pdf.page()` of
  something that is not a `PageRef` of this document's results,
  `structTree()` of a page of another document, `elementBbox()` of an
  element of another tree), and failures to load the module, the worker
  script (its URL named) or a CMap file.

Callers should branch on `error.name` (or `isInstance`) and treat the message
as diagnostic text.
