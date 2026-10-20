# Public Extraction API

`pdfplumber.js` exposes a browser-capable JavaScript API modeled on Python
`pdfplumber` 0.11.9. The extraction engine is a faithful Rust port of
pdfplumber and pdfminer.six compiled to WebAssembly (`crates/`); results are
those of Python pdfplumber (CPython ≥ 3.12), verified against it. Callers use
the same `open()` entry point in Node and browsers. How the runtime works
(WebAssembly loading, result encoding, CMaps, workers) is described in
[`js-runtime.md`](js-runtime.md).

> **Status.** The runtime, the errors, the warnings channel and the document
> handle are in place. The page layer of the Rust port (`pdfplumber.page`,
> `pdf`, the interpreter) has not landed yet: `pdf.pages`, `pdf.metadata` and
> every page-level feature below throw `NotImplemented` until it does. Their
> names, options and result types are fixed here and in the TypeScript types.

## Entrypoint

```ts
import { open } from "pdfplumber-js";

const pdf = await open(input, options);
try {
  const page = pdf.pages[0];
  console.log(await page.extractText());
} finally {
  await pdf.close();
}
```

Supported inputs (`PDFInput`):

- Node: filesystem path, `file:` URL, HTTP(S) `URL` or URL string,
  `ArrayBuffer`, any `ArrayBufferView` (`Uint8Array`, `Buffer`), `Blob`.
- Browser: `ArrayBuffer`, `ArrayBufferView`, `Blob`, `URL`, and URL string
  (resolved against the document's base URL).

Supported `open()` options (`OpenOptions`), with pdfplumber's names:

- `pages`: one-based page numbers to load.
- `password`: password for encrypted PDFs (a string or bytes).
- `laparams`: `LAParams` keyword arguments (layout analysis), or `null`.
- `unicode_norm`: one of `NFC`, `NFKC`, `NFD`, or `NFKD`.
- `raise_unicode_errors`, `strict_metadata`: as in pdfplumber.

As `pdfplumber.open`, `open()` does not check these values: they go to the
core as they are, and the ported code raises Python's error where Python
raises it (an unknown `unicode_norm` when text is normalized, a `pages`
that is not a list of numbers when the pages load...).
- `logLevel`: where diagnostics go besides `onWarning`: `silent` (default),
  `warning` (warnings and above to `console.warn`) or `debug` (everything).
- `onWarning`: callback receiving `{ source, level, message, page? }` for
  each diagnostic at warning level and above (every level with
  `logLevel: "debug"`).

Diagnostics are the text of pdfminer's `logging` calls (`source` is the
logger name, such as `pdfminer.pdfdocument`) and of pdfplumber's
`warnings.warn` (`source` is the warning category). They never change a
result. Python prints warnings to stderr by default; the JavaScript API is
silent by default.

Runtime settings, before the first `open()` (`configure(options)`,
`RuntimeOptions`): where the WebAssembly module is (`wasm`: URL, path,
bytes or compiled module), where the predefined CMaps are or how to load
them (`cmaps`: `{ baseUrl }`, `{ load(name) }`, or `false`), and, in
browsers, `worker: true` to run the core in a dedicated worker. There is no
engine selection: the Rust core is the engine.

The package also exports TypeScript types for the documented surface:
`PDFInput`, `OpenOptions`, `RuntimeOptions`, `PDFPlumberWarning`,
`PDFPlumberDocument`, `PDFPlumberPage`, `PDFObject`, `Word`, `SearchResult`,
`ExtractTextOptions`, `WordOptions`, `SearchOptions`, `TextLineOptions`,
`TextDirection`, `CropOptions`, `DedupeOptions`, `TableOptions`,
`TableStrategy`, `ExplicitTableLine`, `Table`, `TableAxisGroup`, `Bbox`,
`LAParams`, and the classes `PDFPlumberError`, `NotImplemented`,
`CoreCrash`, `PSLiteral`, `PSKeyword`, `PDFObjRef`, `PDFStream`, `PyFloat`,
`PyTuple`.

## Asynchrony

The core is synchronous, but predefined CMaps (CJK fonts) are fetched lazily
and a browser can only fetch asynchronously; worker mode is asynchronous too.
So everything that extracts data returns a `Promise`: page object getters
(`await page.chars`), `extractText()`, `search()`, tables... `pdf.pages`,
`pdf.metadata` and page geometry are plain properties, known when `open()`
resolves.

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
- Integer-like keys (`"1"`) come first in a JavaScript object's key order,
  whatever their position in the Python dict.

## Document

`open()` resolves to a `PDFPlumberDocument` with:

- `metadata`: Info dictionary metadata using pdfplumber-compatible keys.
- `pages`: loaded `PDFPlumberPage` objects.
- `objects`, `annots`, `hyperlinks`, `edges`, `horizontal_edges`,
  `vertical_edges`: document-level objects (promises).
- `close()`: releases the document in the core (idempotent;
  `await using pdf = await open(...)` works too).

## Page Geometry

Each page exposes pdfplumber-style geometry:

- `page_number` and `pageNumber`.
- `width`, `height`, and `bbox`; the page boxes when present.

All object coordinates use pdfplumber's top-left page coordinate convention:
`x0`, `x1`, `y0`, `y1`, `top`, `bottom`, `doctop`, `width`, and `height`.

## Page Objects

Object arrays are available directly and through `page.objects` (promises):
`chars`, `rects`, `lines`, `curves`, `images`, `annots`, `hyperlinks`,
`edges` (and the other edge lists), layout objects with `laparams`.

## Text, Words, And Search

Pages expose camelCase and snake_case aliases:

- `extractText(options)` / `extract_text(options)`.
- `extractWords(options)` / `extract_words(options)`.
- `extractTextLines(options)` / `extract_text_lines(options)`.
- `search(pattern, options)`.

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
  `RegExp` throws `ValueError`.
- A string is compiled with the `u` flag (and `i` when `case: false`),
  escaped first when `regex: false`.
- Matches come from `String.prototype.matchAll`; pdfplumber then drops the
  empty and whitespace-only ones, as it does in Python.
- `main_group` is read for each match as Python's `m.group(main_group)`
  reads it: a group number (a boolean counts as 0 or 1), or the name of a
  named group (`(?<name>…)`); a number out of range or an unknown name
  throws `IndexError` (only when there is a match), a group that did not
  take part `AttributeError`, as in Python. The core converts
  JavaScript's UTF-16 offsets to Python's code point offsets; a match that
  starts or ends inside a surrogate pair (a pattern without `u` matching half
  of an astral character) throws `ValueError`.

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

## Cropping, Filtering, And Dedupe

Pages can derive filtered page views, kept by the core:

- `filter(testFunction)` (asynchronous: the function runs on the objects).
- `crop(bbox, options)`.
- `withinBbox(bbox, options)` / `within_bbox(bbox, options)`.
- `outsideBbox(bbox, options)` / `outside_bbox(bbox, options)`.
- `dedupeChars(options)` / `dedupe_chars(options)`.

Crop options support `relative` and `strict`. Dedupe options include
`tolerance` and `extra_attrs`.

## Tables

Table APIs expose pdfplumber-style table inputs and outputs:

- `findTable(options)` / `find_table(options)`.
- `findTables(options)` / `find_tables(options)`.
- `extractTable(options)` / `extract_table(options)`.
- `extractTables(options)` / `extract_tables(options)`.

Supported table settings include `vertical_strategy` and
`horizontal_strategy` (`lines`, `lines_strict`, `text`, `explicit`),
`explicit_vertical_lines` and `explicit_horizontal_lines`, snap, join,
intersection, edge-min-length and text tolerances, minimum word thresholds,
and `text_*` options forwarded to cell text extraction.

`findTable()` and `findTables()` return `Table` objects with `bbox`,
`cells`, `rows` and `columns` (`TableAxisGroup`: `bbox` and `cells`, `null`
where a row or column has no cell), and `extract()`. `extractTable()` and
`extractTables()` return arrays of cell strings or `null` values.

## Not Ported

These parts of pdfplumber 0.11.9 are out of scope (`docs/architecture.md`,
"Règles de port"):

- `Page.to_image` and `pdfplumber/display.py` (rendering through pypdfium2
  and PIL).
- `pdfplumber.open(..., repair=True)`, `gs_path`, `repair_setting` and
  `pdfplumber/repair.py` (Ghostscript run in a subprocess): the core raises
  `NotPorted`.
- The command line (`pdfplumber/cli.py`).

Every other `pdfplumber.open` option is ported (`pages`, `laparams`,
`password`, `strict_metadata`, `unicode_norm`, `raise_unicode_errors`).

Inline images: pdfminer names an inline image `str(id(stream))`, a memory
address; the port names it `inline-<k>` (`k`: its rank among the page's
inline images). This name is the `name` of the image in `page.images` and,
with `laparams`, of its figure. An image XObject that a PDF names
`inline-0` gets the same name as the page's first inline image.

Not modelled (the core raises `NotPorted` where Python would go on):

- a `laparams` margin (`line_overlap`, `char_margin`, `line_margin`,
  `word_margin`) that is not a number: Python stores any value and fails
  at its first use in the layout analysis;
- a `password` that is neither `null` nor a string;
- `to_dict`/`to_csv` with an `object_types` entry that names a method
  (`"extract_word"` gives `extract_words`): Python serializes the bound
  method's `str()`.

After `close()`, a document opened from a path (`Source::Path` in Rust)
raises `ValueError: seek of closed file` when an object not read before
is needed, as in Python (a document read from bytes is the caller's
stream, which `close()` leaves open).

Not modelled either, without an error: the order Python gives a NaN coordinate in the sets of
`cluster_objects` (words, lines, table edges) follows its memory address,
which the port cannot reproduce (it puts NaN values last).

Structure tree, metadata and annotations (`docs/architecture.md`,
"Vague 4", workstream DOC):

- a cycle among the structure elements that a page's parent tree reaches
  makes pdfplumber's `PDFStructTree(pdf, page)` (and `page.structure_tree`)
  loop until memory runs out; the core raises `MemoryError` once the walk
  only repeats itself;
- `structure_tree` deep-copies the attribute values of the elements
  (`dataclasses.asdict`): for a document read from bytes (`io.BytesIO`,
  what JavaScript gives), a reference among them copies the whole document
  in Python, which the core does not do (it keeps the reference, an equal
  value) nor count in the recursion depth; for a document opened from a
  path, Python and the core raise the same `TypeError` (the file object
  cannot be copied);
- pdfplumber decodes the metadata in place, in the document's own
  dictionaries: whatever an `/Info` value reaches (the catalog, a page, an
  annotation, a structure element) is read decoded afterwards, by the
  pages, the structure tree and the annotations. The core does the same.

## Runtime Contract

The package is ESM. Node loads `dist/src/index.js`, browsers
`dist/browser/index.js` (bundlers pick it through the `browser` export
condition); both load `dist/pdfplumber.wasm` and fetch or read
`dist/cmaps/` next to themselves, unless `configure()` says otherwise.
Worker mode uses `dist/browser/worker.js`. The same runtime tests run in
Node, Chromium, Firefox and WebKit ([`js-runtime.md`](js-runtime.md#tests)).

## Errors

An exception raised by the core is a `PDFPlumberError` (an `Error`) whose
`name` is the Python class name and `message` is `str(e)`, as pdfplumber
raises it:

- `PdfminerException`: `open()` on an empty, invalid or unreadable PDF, and
  on an encrypted PDF with a missing or wrong password (pdfplumber wraps
  every exception of pdfminer's `PDFDocument` in it). *Pending the page
  layer*: today `open()` only hands the bytes to the core and resolves; these
  errors will come from `open()` once pdfplumber's `PDF` is ported.
- `MalformedPDFException`, `PDFSyntaxError`, `PSEOF`... and Python builtins
  (`ValueError`, `TypeError`, `KeyError`...) raised later by pdfminer or
  pdfplumber.

Every `PDFPlumberError` carries `pythonModule`, `pythonQualname` and
`pythonMro` (the class's `__mro__`, as `module.qualname` strings);
`error.isInstance("PDFException")` answers like Python's `isinstance`, by
class name or qualified name.

Other errors:

- `NotImplemented` (a `PDFPlumberError`): a feature whose Rust core has not
  landed yet.
- `CoreCrash`: the WebAssembly core stopped on an internal error (a bug);
  its documents are lost, the next `open()` starts a new core.
- `Error` and `TypeError`: input adaptation failures (a missing file, a
  failed HTTP fetch, an unsupported input type, an argument that contains
  itself), and failures to load the module or a CMap file.

Callers should branch on `error.name` (or `isInstance`) and treat the message
as diagnostic text.
