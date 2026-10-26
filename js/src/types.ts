/**
 * Public types (docs/public-api.md). Option names and object keys follow
 * pdfplumber's (snake_case); methods have camelCase and snake_case names.
 */

/** What `open()` reads. Node: a filesystem path, a `file:` or HTTP(S) URL
 * (string or `URL`), bytes or a `Blob`. Browser: bytes, a `Blob`, a URL
 * (string, resolved against the document, or `URL`). */
export type PDFInput = string | URL | ArrayBuffer | ArrayBufferView | Blob;

/** A diagnostic forwarded to `onWarning`: the text of a pdfminer `logging`
 * call or a pdfplumber `warnings.warn`. It never changes a result. */
export interface PDFPlumberWarning {
  /** Logger name (`"pdfminer.pdfdocument"`) or warning category. */
  source: string;
  /** `"debug"`, `"info"`, `"warning"`, `"error"` or `"critical"`. */
  level: "debug" | "info" | "warning" | "error" | "critical";
  message: string;
  /** One-based number of the page being processed, when there is one. */
  page?: number;
}

/** `pdfplumber.open(...)` options. */
export interface OpenOptions {
  /** One-based page numbers to load (`pages=`). */
  pages?: number[];
  /** Password of an encrypted PDF (`password=`: `str` or `bytes`). */
  password?: string | Uint8Array;
  /** `laparams=`: layout analysis parameters (`LAParams` keyword
   * arguments), or `null`/absent for none. */
  laparams?: LAParams | null;
  /** `unicode_norm=` */
  unicode_norm?: "NFC" | "NFKC" | "NFD" | "NFKD" | null;
  /** `raise_unicode_errors=` */
  raise_unicode_errors?: boolean;
  /** `strict_metadata=` */
  strict_metadata?: boolean;
  /** `repair=`: `true` is not ported (Ghostscript; `NotImplemented`). */
  repair?: boolean;
  /** `gs_path=`, `repair_setting=`: read with `repair` only. */
  gs_path?: string | null;
  repair_setting?: string;
  /** Where diagnostics go besides `onWarning`: `"silent"` (default),
   * `"warning"` (warnings and above to `console.warn`) or `"debug"`
   * (everything to `console.debug`/`console.warn`). */
  logLevel?: "silent" | "warning" | "debug";
  /** Receives the diagnostics at warning level and above (all of them with
   * `logLevel: "debug"`). */
  onWarning?: (warning: PDFPlumberWarning) => void;
  /** Any other option is a keyword argument of `pdfplumber.open` too
   * (an unknown name: Python's `TypeError`). */
  [keyword: string]: unknown;
}

/** `pdfminer.layout.LAParams` keyword arguments. */
export interface LAParams {
  line_overlap?: number;
  char_margin?: number;
  line_margin?: number;
  word_margin?: number;
  boxes_flow?: number | null;
  detect_vertical?: boolean;
  all_texts?: boolean;
}

/** A page object (`char`, `rect`, `line`, `curve`, `image`, `annot`...):
 * pdfplumber's dict, keys in pdfplumber's order (see docs/js-runtime.md,
 * "Values", for how Python values map to JavaScript). */
export interface PDFObject {
  object_type: string;
  page_number: number;
  x0: number;
  x1: number;
  top: number;
  bottom: number;
  doctop?: number;
  y0?: number;
  y1?: number;
  width?: number;
  height?: number;
  [key: string]: unknown;
}

/** A word of `extractWords`. */
export interface Word {
  text: string;
  x0: number;
  x1: number;
  top: number;
  bottom: number;
  doctop: number;
  upright: boolean;
  height: number;
  width: number;
  direction: "ltr" | "rtl" | "ttb" | "btt";
  chars?: PDFObject[];
  [key: string]: unknown;
}

export type TextDirection = "ltr" | "rtl" | "ttb" | "btt";

/** `extract_text` / `chars_to_textmap` keyword arguments. */
export interface ExtractTextOptions extends WordOptions {
  layout?: boolean;
  layout_width?: number;
  layout_height?: number;
  layout_width_chars?: number;
  layout_height_chars?: number;
  layout_bbox?: [number, number, number, number];
  x_density?: number;
  y_density?: number;
  line_dir_render?: TextDirection | null;
  char_dir_render?: TextDirection | null;
}

/** `extract_words` keyword arguments. */
export interface WordOptions {
  x_tolerance?: number;
  y_tolerance?: number;
  x_tolerance_ratio?: number | null;
  y_tolerance_ratio?: number | null;
  keep_blank_chars?: boolean;
  use_text_flow?: boolean;
  line_dir?: TextDirection;
  char_dir?: TextDirection;
  line_dir_rotated?: TextDirection | null;
  char_dir_rotated?: TextDirection | null;
  split_at_punctuation?: boolean | string;
  extra_attrs?: string[] | null;
  expand_ligatures?: boolean;
  return_chars?: boolean;
}

/** `search` options: pdfplumber's, plus the `extract_text` ones. The
 * pattern is a JavaScript `RegExp` or string (docs/public-api.md,
 * "Search"). */
export interface SearchOptions extends Omit<ExtractTextOptions, "return_chars"> {
  regex?: boolean | null;
  case?: boolean | null;
  return_groups?: boolean | null;
  return_chars?: boolean | null;
  /** The group whose text and bbox a result has: a number, or the name
   * of a named group (`(?<name>…)`), read for each match as Python's
   * `m.group(main_group)` reads it. */
  main_group?: number | string | null;
}

/** A match of `search` (pdfplumber's `match_to_dict`). */
export interface SearchResult {
  text: string;
  x0: number;
  top: number;
  x1: number;
  bottom: number;
  /** The groups' texts (`m.groups()`), `null` for a group that did not
   * take part. */
  groups?: (string | null)[];
  chars?: PDFObject[];
}

/** `extract_text_lines` options. */
export interface TextLineOptions extends Omit<ExtractTextOptions, "return_chars"> {
  strip?: boolean | null;
  return_chars?: boolean | null;
}

export type Bbox = [number, number, number, number];

/** `crop` / `within_bbox` / `outside_bbox` options. */
export interface CropOptions {
  relative?: boolean;
  strict?: boolean;
}

/** `dedupe_chars` options. */
export interface DedupeOptions {
  tolerance?: number;
  extra_attrs?: string[] | null;
}

export type TableStrategy = "lines" | "lines_strict" | "text" | "explicit";

/** An explicit table line: a coordinate, or an object whose edges are used. */
export type ExplicitTableLine = number | PDFObject;

/** pdfplumber's `table_settings`. */
export interface TableOptions {
  vertical_strategy?: TableStrategy;
  horizontal_strategy?: TableStrategy;
  explicit_vertical_lines?: ExplicitTableLine[];
  explicit_horizontal_lines?: ExplicitTableLine[];
  snap_tolerance?: number;
  snap_x_tolerance?: number;
  snap_y_tolerance?: number;
  join_tolerance?: number;
  join_x_tolerance?: number;
  join_y_tolerance?: number;
  edge_min_length?: number;
  edge_min_length_prefilter?: number;
  min_words_vertical?: number;
  min_words_horizontal?: number;
  intersection_tolerance?: number;
  intersection_x_tolerance?: number;
  intersection_y_tolerance?: number;
  text_tolerance?: number;
  text_x_tolerance?: number;
  text_y_tolerance?: number;
  [textOption: `text_${string}`]: unknown;
}

/** A row or column of a table (`pdfplumber.table.Row`/`Column`). */
export interface TableAxisGroup {
  bbox: Bbox;
  /** The cells, `null` where the row (column) has none. */
  cells: (Bbox | null)[];
}

/** `to_json` / `to_csv` keyword arguments (pdfplumber/container.py);
 * `to_dict` takes `object_types` only. `stream`: `null` (the default)
 * returns the text; any other stream is not modelled (`NotImplemented`). */
export interface SerializeOptions {
  stream?: null;
  object_types?: string[] | null;
  include_attrs?: string[] | null;
  exclude_attrs?: string[] | null;
  precision?: number | null;
  indent?: number | null;
}

/** `pdfplumber.table.TableFinder` (`page.debugTablefinder()`): the edges
 * the finder kept, the intersections (`[point, {v, h}]` items of Python's
 * dict, keyed by `(x, top)`), the cells, the tables, and the settings it
 * resolved (`TableSettings`, its fields as a dict). */
export interface TableFinderResult<T> {
  edges: PDFObject[];
  intersections: [[number, number], { v: PDFObject[]; h: PDFObject[] }][];
  cells: Bbox[];
  tables: T[];
  settings: Record<string, unknown>;
}

/** What `PDFStructTree.findAll` and `find` match (pdfplumber/structure.py
 * `_find_all`): an element type, a `RegExp` (JavaScript semantics,
 * anchored at the start of the type, as `re.match`), or a function of an
 * element (run on every element of the tree, in Python's order). */
export type StructMatcher<E> = string | RegExp | ((element: E) => unknown);

/** A compiled `WebAssembly.Module`, where the program's libraries declare
 * it (the DOM library, a worker's); else any object: the package's
 * declarations type-check without the DOM library (a Node project with
 * `@types/node` alone). */
export type WasmModule = typeof globalThis extends { WebAssembly: { Module: { prototype: infer M } } } ? M : object;

/** Runtime settings (`configure`). */
export interface RuntimeOptions {
  /** The WebAssembly module: a URL, a path (Node), bytes or a compiled
   * module. Default: `pdfplumber.wasm` next to the package's `dist/`. */
  wasm?: string | URL | ArrayBuffer | Uint8Array | WasmModule;
  /** Predefined CMaps: `false` to go without them (CJK documents then lose
   * their text, as pdfminer without its `cmap` directory), a base URL or
   * directory, or a loader. Default: `cmaps/` next to the package's
   * `dist/`. */
  cmaps?: false | { baseUrl?: string | URL; load?: (name: string) => Promise<Uint8Array> };
  /** Browser only: run the core in a dedicated worker (`true`, or the URL
   * of `dist/browser/worker.js`). */
  worker?: boolean | string | URL;
}
