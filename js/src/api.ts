/**
 * The public API (docs/public-api.md): `open()`, `PDFPlumberDocument`,
 * `PDFPlumberPage`, `Table`.
 *
 * Every member is one job of the core (`Core.exclusive`: the FIFO of the
 * core's requests): the operations of crates/wasm/src/pages.rs it runs,
 * each a statement of oracle/unit/js_pages_run.py, and the JavaScript
 * between them (a `RegExp` for `search`, the test function of `filter`).
 * The table of members, operations and statements is docs/public-api.md,
 * "Operations"; docs/architecture.md, "Vague 5", has the contract this
 * file implements (wave 5 base: the spine; the API workstream completes
 * it).
 */

import { NotImplemented } from "./errors.js";
import { WasmCore, type CallOptions, type Core, type CoreCall, type Diagnostic } from "./core/runtime.js";
import { platform } from "./platform/platform.js";
import { ORACLE, TYPED, type OracleOpen, type OracleSearch } from "./hooks.js";
import { compilePattern, matchSpans } from "./search.js";
import { PageRef } from "./pdfobjects.js";
import type {
  Bbox,
  CropOptions,
  DedupeOptions,
  ExtractTextOptions,
  OpenOptions,
  PDFInput,
  PDFObject,
  PDFPlumberWarning,
  RuntimeOptions,
  SearchOptions,
  SearchResult,
  SerializeOptions,
  TableAxisGroup,
  TableOptions,
  TextLineOptions,
  Word,
  WordOptions,
} from "./types.js";

let runtimeOptions: RuntimeOptions = {};
let corePromise: Promise<Core> | undefined;

/** Sets the runtime options (where the WebAssembly module and the CMaps
 * are, worker mode). Applies to the documents opened afterwards. */
export function configure(options: RuntimeOptions): void {
  runtimeOptions = { ...options };
  corePromise = undefined;
}

/** @internal The next document opens in a new core, with the same
 * runtime options (a new Python process: the tests replaying sessions of
 * oracle/unit/js_pages_run.py). */
export function dropCore(): void {
  const previous = corePromise;
  corePromise = undefined;
  // the documents of the dropped process are never used again: stop its
  // worker, which would otherwise keep its WebAssembly memory alive (one
  // per session: browsers then fail to allocate a new instance)
  previous?.then(
    (core) => core.terminate?.(),
    () => {},
  );
}

/** The compiled module of the last fetched WebAssembly source (the
 * default URL or a URL given as a string or `URL`): a new core (after a
 * crash, `configure()` or `dropCore()`) instantiates it again instead of
 * fetching and compiling the module once more. */
let compiled: { key: string | undefined; module: Promise<WebAssembly.Module> } | undefined;

function compileWasm(source: RuntimeOptions["wasm"]): Promise<WebAssembly.Module> {
  const p = platform();
  if (source !== undefined && typeof source !== "string" && !(source instanceof URL)) return p.compileWasm(source);
  const key = source instanceof URL ? source.href : source;
  if (compiled && compiled.key === key) return compiled.module;
  const entry = { key, module: p.compileWasm(source) };
  compiled = entry;
  entry.module.catch(() => {
    if (compiled === entry) compiled = undefined;
  });
  return entry.module;
}

/** The core the next document opens in (a new one after a crash). */
export async function getCore(): Promise<Core> {
  if (corePromise) {
    const core = await corePromise.catch(() => undefined);
    if (core && !core.crashed) return core;
  }
  const options = runtimeOptions;
  corePromise = (async () => {
    const p = platform();
    const module = await compileWasm(options.wasm);
    if (options.worker) {
      if (!p.workerCore) throw new Error("worker mode is available in browsers only");
      return p.workerCore(module, options);
    }
    return WasmCore.create(module, p.cmapSource(options.cmaps));
  })();
  corePromise.catch(() => (corePromise = undefined));
  return corePromise;
}

const LEVELS: Record<number, PDFPlumberWarning["level"]> = {
  10: "debug",
  20: "info",
  30: "warning",
  40: "error",
  50: "critical",
};

/** How a document's calls collect and forward diagnostics. */
export interface DiagnosticsPolicy {
  minLevel: number;
  onDiagnostics?: (entries: Diagnostic[], dropped: number) => void;
}

export function diagnosticsPolicy(options: OpenOptions, page?: number): DiagnosticsPolicy {
  const logLevel = options.logLevel ?? "silent";
  const minLevel = logLevel === "debug" ? 10 : logLevel === "warning" || options.onWarning ? 30 : 0;
  if (!minLevel) return { minLevel };
  return {
    minLevel,
    onDiagnostics(entries, dropped) {
      for (const d of entries) {
        const w: PDFPlumberWarning = { source: d.source, level: LEVELS[d.level] ?? "warning", message: d.message };
        if (page !== undefined) w.page = page;
        options.onWarning?.(w);
        if (logLevel === "debug" || (logLevel === "warning" && d.level >= 30)) {
          const log = d.level >= 30 ? console.warn : console.debug;
          log(`pdfplumber-js: ${d.source}: ${d.message}`);
        }
      }
      if (dropped) options.onWarning?.({ source: "pdfplumber-js", level: "warning", message: `${dropped} more diagnostics dropped` });
    },
  };
}

/** The keyword arguments of `pdfplumber.open`: every option but the API's
 * own (`logLevel`, `onWarning`), as they are: pdfplumber checks none of
 * them at `open()` but their names (an unknown one is Python's
 * `TypeError`), and the ported code raises Python's errors where Python
 * does (an `unicode_norm` it does not know when the text is normalized,
 * `pages` when the pages load...). */
function openKwargs(options: OpenOptions): Record<string, unknown> {
  const kwargs: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(options)) {
    if (k === "logLevel" || k === "onWarning") continue;
    Object.defineProperty(kwargs, k, { value: v, enumerable: true, writable: true, configurable: true });
  }
  return kwargs;
}

/** A document released (`release()`, `await using`) is no longer in the
 * core: a JavaScript lifetime, which Python has no counterpart of. */
export class DocumentReleased extends Error {
  constructor() {
    super("the document was released (release() or await using): open it again");
    this.name = "DocumentReleased";
  }
}

const registry =
  typeof FinalizationRegistry === "function"
    ? new FinalizationRegistry<{ core: Core; handle: number }>(({ core, handle }) => {
        if (!core.crashed) core.call("free", handle, null).catch(() => undefined);
      })
    : undefined;

/** Opens a PDF (`pdfplumber.open`): a filesystem path (Node) as
 * `pdfplumber.open(path)`, any other input as
 * `pdfplumber.open(io.BytesIO(bytes))`. */
export async function open(input: PDFInput, options: OpenOptions = {}): Promise<PDFPlumberDocument> {
  const args = openKwargs(options);
  const p = platform();
  const path = p.filePath?.(input);
  const bytes = await p.readInput(input);
  const core = await getCore();
  const policy = diagnosticsPolicy(options);
  const oracle = (options as { [ORACLE]?: OracleOpen })[ORACLE];
  let handle: number;
  if (oracle) handle = (await core.call("oracle_doc_open", 0, [args, oracle.ids, path ?? null], { data: bytes, ...policy })) as number;
  else if (path !== undefined) handle = (await core.call("doc_open_file", 0, [path, args], { data: bytes, ...policy })) as number;
  else handle = (await core.call("doc_open", 0, args, { data: bytes, ...policy })) as number;
  return new PDFPlumberDocument(core, handle, options);
}

/** The geometry of a page as the core gives it (`doc_pages`, `page_info`). */
interface Geometry {
  handle?: number;
  page_number: number;
  initial_doctop: number;
  rotation: number;
  mediabox: Bbox;
  cropbox: Bbox;
  trimbox?: Bbox;
  bleedbox?: Bbox;
  artbox?: Bbox;
  bbox: Bbox;
  width: number;
  height: number;
}

/** The names `getattr(container, name)` reads (`doc_attr`, `page_attr`):
 * the object lists of `Container` (pdfplumber/container.py), whose names
 * end with "s". */
type ListName =
  | "chars"
  | "rects"
  | "lines"
  | "curves"
  | "images"
  | "annots"
  | "hyperlinks"
  | "edges"
  | "rect_edges"
  | "curve_edges"
  | "horizontal_edges"
  | "vertical_edges"
  | "textboxhorizontals"
  | "textboxverticals"
  | "textlinehorizontals"
  | "textlineverticals";

/** An open PDF (`pdfplumber.PDF`). */
export class PDFPlumberDocument {
  #released = false;
  #pages: Promise<PDFPlumberPage[]> | undefined;
  /** The pages by handle (`PageRef` resolution). */
  readonly #byHandle = new Map<number, PDFPlumberPage>();
  readonly #token = {};

  /** @internal */
  constructor(
    /** @internal */ readonly core: Core,
    /** @internal */ readonly handle: number,
    readonly options: OpenOptions,
  ) {
    registry?.register(this, { core, handle }, this.#token);
  }

  /** @internal A job on this document: rejects once it is released. */
  job<T>(f: (call: CoreCall) => Promise<T>): Promise<T> {
    if (this.#released) return Promise.reject(new DocumentReleased());
    return this.core.exclusive((call) => {
      if (this.#released) throw new DocumentReleased();
      return f(call);
    });
  }

  /** @internal The call options of a request on this document (its
   * diagnostics, for page `page`). */
  callOptions(page?: number): CallOptions {
    return diagnosticsPolicy(this.options, page);
  }

  /** @internal A page of this document from its geometry. */
  page_(geometry: Geometry, handle: number, typed: unknown, parent?: PDFPlumberPage, geometryError?: unknown): PDFPlumberPage {
    const known = this.#byHandle.get(handle);
    if (known) return known;
    const page = new PDFPlumberPage(this, handle, geometry, typed, parent, geometryError);
    this.#byHandle.set(handle, page);
    return page;
  }

  /** The pages (`pdf.pages`): computed at the first read, as pdfplumber
   * computes them lazily (an error of the page tree or of a page's boxes
   * rejects this promise, not `open()`); the same array until `close()`,
   * which flushes them, as `PDF.close()` does. A rejected read is not
   * kept: the next read runs `pdf.pages` again, as Python does. */
  get pages(): Promise<PDFPlumberPage[]> {
    if (this.#pages) return this.#pages;
    const p = this.job(async (call) => {
      const list = (await call("doc_pages", this.handle, null, this.callOptions())) as Geometry[];
      const typed = this.core.lastTyped as unknown[] | undefined;
      return list.map((g, i) => this.page_(g, g.handle!, typed?.[i]));
    });
    this.#pages = p;
    p.catch(() => {
      if (this.#pages === p) this.#pages = undefined;
    });
    return p;
  }

  /** The page of a `PageRef` (a `Page` held in a result: an annotation's
   * `data.P`...): the same `PDFPlumberPage` as `pdf.pages` gives for it. */
  page(ref: PageRef): Promise<PDFPlumberPage> {
    const known = this.#byHandle.get(ref.handle);
    if (known) return Promise.resolve(known);
    return this.job(async (call) => {
      const g = (await call("page_info", ref.handle, null, this.callOptions())) as Geometry;
      return this.page_(g, ref.handle, this.core.lastTyped);
    });
  }

  #get<T>(op: string, args: unknown = null): Promise<T> {
    return this.job(async (call) => (await call(op, this.handle, args, this.callOptions())) as T);
  }

  /** The Info dictionary (`pdf.metadata`). */
  get metadata(): Promise<Record<string, unknown>> {
    return this.#get("doc_metadata");
  }

  /** `pdf.objects`: the objects of every loaded page, by `object_type`. */
  get objects(): Promise<Record<string, PDFObject[]>> {
    return this.#get("doc_objects");
  }

  #list(name: ListName): Promise<PDFObject[]> {
    return this.#get("doc_attr", [name]);
  }

  get chars(): Promise<PDFObject[]> {
    return this.#list("chars");
  }
  get rects(): Promise<PDFObject[]> {
    return this.#list("rects");
  }
  get lines(): Promise<PDFObject[]> {
    return this.#list("lines");
  }
  get curves(): Promise<PDFObject[]> {
    return this.#list("curves");
  }
  get images(): Promise<PDFObject[]> {
    return this.#list("images");
  }
  get edges(): Promise<PDFObject[]> {
    return this.#list("edges");
  }
  get rect_edges(): Promise<PDFObject[]> {
    return this.#list("rect_edges");
  }
  get rectEdges(): Promise<PDFObject[]> {
    return this.rect_edges;
  }
  get curve_edges(): Promise<PDFObject[]> {
    return this.#list("curve_edges");
  }
  get curveEdges(): Promise<PDFObject[]> {
    return this.curve_edges;
  }
  get horizontal_edges(): Promise<PDFObject[]> {
    return this.#list("horizontal_edges");
  }
  get horizontalEdges(): Promise<PDFObject[]> {
    return this.horizontal_edges;
  }
  get vertical_edges(): Promise<PDFObject[]> {
    return this.#list("vertical_edges");
  }
  get verticalEdges(): Promise<PDFObject[]> {
    return this.vertical_edges;
  }
  get textboxhorizontals(): Promise<PDFObject[]> {
    return this.#list("textboxhorizontals");
  }
  get textboxverticals(): Promise<PDFObject[]> {
    return this.#list("textboxverticals");
  }
  get textlinehorizontals(): Promise<PDFObject[]> {
    return this.#list("textlinehorizontals");
  }
  get textlineverticals(): Promise<PDFObject[]> {
    return this.#list("textlineverticals");
  }

  get annots(): Promise<PDFObject[]> {
    return this.#get("doc_annots");
  }

  get hyperlinks(): Promise<PDFObject[]> {
    return this.#get("doc_hyperlinks");
  }

  /** `pdf.structure_tree`: the elements' `to_dict()`. */
  get structure_tree(): Promise<Record<string, unknown>[]> {
    return this.#get("doc_structure_tree");
  }
  get structureTree(): Promise<Record<string, unknown>[]> {
    return this.structure_tree;
  }

  /** `PDFStructTree(pdf, page)`: the structure tree of the document, or of
   * one of its pages, with `findAll`, `find`, `elementBbox` and its
   * elements' `allMcids`; waits for the boundary operations
   * (docs/architecture.md, "Vague 5", BOUNDARY). */
  structTree(_page?: PDFPlumberPage): Promise<never> {
    return Promise.reject(new NotImplemented("PDFPlumberDocument.structTree"));
  }

  /** `pdf.to_dict(object_types)` */
  toDict(objectTypes?: string[] | null): Promise<Record<string, unknown>> {
    return this.#get("doc_to_dict", [objectTypes === undefined ? {} : { object_types: objectTypes }]);
  }
  to_dict(objectTypes?: string[] | null): Promise<Record<string, unknown>> {
    return this.toDict(objectTypes);
  }

  /** `pdf.to_json(**options)` (`stream: null`: the text). */
  toJson(options: SerializeOptions = {}): Promise<string> {
    return this.#get("doc_to_json", [options]);
  }
  to_json(options?: SerializeOptions): Promise<string> {
    return this.toJson(options);
  }

  /** `pdf.to_csv(**options)` */
  toCsv(options: SerializeOptions = {}): Promise<string> {
    return this.#get("doc_to_csv", [options]);
  }
  to_csv(options?: SerializeOptions): Promise<string> {
    return this.toCsv(options);
  }

  /** `PDF.close()`: flushes the caches of the document and its pages, and
   * closes a file it opened (a path): the document stays usable, as in
   * Python (its pages are made again at the next `pages`, a file closed
   * raises `ValueError` where an object not read before is needed). The
   * core keeps it until `release()`. */
  close(): Promise<void> {
    return this.job(async (call) => {
      await call("doc_close", this.handle, [], this.callOptions());
      this.#pages = undefined;
      this.#byHandle.clear();
    });
  }

  /** Releases the document in the core, with its pages and tables
   * (idempotent); any later call rejects with `DocumentReleased`. */
  async release(): Promise<void> {
    if (this.#released) return;
    this.#released = true;
    this.#pages = undefined;
    this.#byHandle.clear();
    registry?.unregister(this.#token);
    if (!this.core.crashed) await this.core.call("free", this.handle, null);
  }

  /** `await using pdf = await open(...)`: `close()`, then `release()`, as
   * `with pdfplumber.open(...) as pdf:` closes it. */
  async [Symbol.asyncDispose](): Promise<void> {
    try {
      if (!this.#released && !this.core.crashed) await this.close();
    } finally {
      await this.release();
    }
  }
}

/** A table a page found (`pdfplumber.table.Table`). */
export class Table {
  /** @internal */
  constructor(
    /** The page it was found on (`Table.page`). */
    readonly page: PDFPlumberPage,
    /** @internal */ readonly handle: number,
    readonly bbox: Bbox,
    readonly cells: Bbox[],
    readonly rows: TableAxisGroup[],
    readonly columns: TableAxisGroup[],
  ) {}

  /** `Table.extract(**options)`: the text of each cell, `null` where
   * there is no cell. */
  extract(options: ExtractTextOptions = {}): Promise<(string | null)[][]> {
    const doc = this.page.pdf;
    return doc.job(
      async (call) => (await call("table_extract", this.handle, [options], doc.callOptions(this.page.page_number))) as (string | null)[][],
    );
  }
}

interface TableValue {
  handle: number;
  bbox: Bbox;
  cells: Bbox[];
  rows: TableAxisGroup[];
  columns: TableAxisGroup[];
}

/** The options of `search` that `Page.search` takes itself (the others
 * go to the text map). */
const SEARCH_ARGS = new Set(["regex", "case", "main_group", "return_chars", "return_groups"]);

/** A page (`pdfplumber.page.Page`), or a page derived from one (`crop`,
 * `filter`...). Coordinates follow pdfplumber (top-left origin).
 *
 * Its geometry is read when the page is made (`pdf.pages`, or the call
 * that derives it), in the same job; a geometry Python cannot compute (a
 * crop box kept as given with `strict: false`) throws when `width` or
 * `height` is read, as `page.width` raises in Python. */
export class PDFPlumberPage {
  readonly page_number: number;
  readonly initial_doctop: number;
  readonly rotation: number;
  readonly mediabox: Bbox;
  readonly cropbox: Bbox;
  readonly trimbox?: Bbox;
  readonly bleedbox?: Bbox;
  readonly artbox?: Bbox;
  readonly bbox: Bbox;
  readonly #width: number | undefined;
  readonly #height: number | undefined;
  readonly #geometryError: unknown;
  /** @internal the typed geometry (hooks.ts) */
  readonly [TYPED]: unknown;

  /** @internal */
  constructor(
    /** The document (`page.pdf`). */
    readonly pdf: PDFPlumberDocument,
    /** @internal */ readonly handle: number,
    geometry: Geometry,
    typed: unknown,
    /** The page it derives from (`DerivedPage.parent_page`), for a derived page. */
    readonly parent?: PDFPlumberPage,
    geometryError?: unknown,
  ) {
    this.page_number = geometry.page_number;
    this.initial_doctop = geometry.initial_doctop;
    this.rotation = geometry.rotation;
    this.mediabox = geometry.mediabox;
    this.cropbox = geometry.cropbox;
    if ("trimbox" in geometry) this.trimbox = geometry.trimbox;
    if ("bleedbox" in geometry) this.bleedbox = geometry.bleedbox;
    if ("artbox" in geometry) this.artbox = geometry.artbox;
    this.bbox = geometry.bbox;
    this.#width = geometry.width;
    this.#height = geometry.height;
    this.#geometryError = geometryError;
    this[TYPED] = typed;
  }

  get pageNumber(): number {
    return this.page_number;
  }
  get initialDoctop(): number {
    return this.initial_doctop;
  }

  /** `page.width` (throws Python's error for a box it cannot subtract). */
  get width(): number {
    if (this.#geometryError !== undefined) throw this.#geometryError;
    return this.#width!;
  }

  /** `page.height` */
  get height(): number {
    if (this.#geometryError !== undefined) throw this.#geometryError;
    return this.#height!;
  }

  /** `repr(page)` */
  toString(): string {
    return `<Page:${this.page_number}>`;
  }

  #run<T>(op: string, args: unknown): Promise<T> {
    return this.pdf.job(async (call) => (await call(op, this.handle, args, this.pdf.callOptions(this.page_number))) as T);
  }

  get objects(): Promise<Record<string, PDFObject[]>> {
    return this.#run("page_objects", null);
  }

  #list(name: ListName): Promise<PDFObject[]> {
    return this.#run("page_attr", [name]);
  }

  get chars(): Promise<PDFObject[]> {
    return this.#list("chars");
  }
  get rects(): Promise<PDFObject[]> {
    return this.#list("rects");
  }
  get lines(): Promise<PDFObject[]> {
    return this.#list("lines");
  }
  get curves(): Promise<PDFObject[]> {
    return this.#list("curves");
  }
  get images(): Promise<PDFObject[]> {
    return this.#list("images");
  }
  get annots(): Promise<PDFObject[]> {
    return this.#list("annots");
  }
  get hyperlinks(): Promise<PDFObject[]> {
    return this.#list("hyperlinks");
  }
  get edges(): Promise<PDFObject[]> {
    return this.#list("edges");
  }
  get rect_edges(): Promise<PDFObject[]> {
    return this.#list("rect_edges");
  }
  get rectEdges(): Promise<PDFObject[]> {
    return this.rect_edges;
  }
  get curve_edges(): Promise<PDFObject[]> {
    return this.#list("curve_edges");
  }
  get curveEdges(): Promise<PDFObject[]> {
    return this.curve_edges;
  }
  get horizontal_edges(): Promise<PDFObject[]> {
    return this.#list("horizontal_edges");
  }
  get horizontalEdges(): Promise<PDFObject[]> {
    return this.horizontal_edges;
  }
  get vertical_edges(): Promise<PDFObject[]> {
    return this.#list("vertical_edges");
  }
  get verticalEdges(): Promise<PDFObject[]> {
    return this.vertical_edges;
  }
  get textboxhorizontals(): Promise<PDFObject[]> {
    return this.#list("textboxhorizontals");
  }
  get textboxverticals(): Promise<PDFObject[]> {
    return this.#list("textboxverticals");
  }
  get textlinehorizontals(): Promise<PDFObject[]> {
    return this.#list("textlinehorizontals");
  }
  get textlineverticals(): Promise<PDFObject[]> {
    return this.#list("textlineverticals");
  }

  /** `page.structure_tree` */
  get structure_tree(): Promise<Record<string, unknown>[]> {
    return this.#run("page_structure_tree", null);
  }
  get structureTree(): Promise<Record<string, unknown>[]> {
    return this.structure_tree;
  }

  extractText(options: ExtractTextOptions = {}): Promise<string> {
    return this.#run("page_extract_text", [options]);
  }
  extract_text(options?: ExtractTextOptions): Promise<string> {
    return this.extractText(options);
  }
  extractTextSimple(options: ExtractTextOptions = {}): Promise<string> {
    return this.#run("page_extract_text_simple", [options]);
  }
  extract_text_simple(options?: ExtractTextOptions): Promise<string> {
    return this.extractTextSimple(options);
  }
  extractWords(options: WordOptions = {}): Promise<Word[]> {
    return this.#run("page_extract_words", [options]);
  }
  extract_words(options?: WordOptions): Promise<Word[]> {
    return this.extractWords(options);
  }
  /** `page.extract_text_lines(strip, return_chars, **options)` */
  extractTextLines(options: TextLineOptions = {}): Promise<SearchResult[]> {
    const { strip, return_chars, ...kw } = options;
    return this.#run("page_extract_text_lines", [strip ?? true, return_chars ?? true, kw]);
  }
  extract_text_lines(options?: TextLineOptions): Promise<SearchResult[]> {
    return this.extractTextLines(options);
  }

  /** `page.search(pattern, **options)` with JavaScript `RegExp` semantics
   * (docs/public-api.md, "Search"): the text the pattern runs over
   * (`page_search_text`), the pattern run over it here, then the results
   * of the spans (`page_search`), in one job. */
  search(pattern: string | RegExp, options: SearchOptions = {}): Promise<SearchResult[]> {
    const oracle = (options as { [ORACLE]?: OracleSearch })[ORACLE];
    const kw: Record<string, unknown> = {};
    const own: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(options)) (SEARCH_ARGS.has(k) ? own : kw)[k] = v;
    return this.pdf.job(async (call) => {
      const opts = this.pdf.callOptions(this.page_number);
      // the text map first (`page.get_textmap(...)`), then the pattern:
      // pdfplumber raises its errors (a RegExp with `regex: false`...) in
      // `TextMap.search`, once the text map is computed
      const text = (await call("page_search_text", this.handle, [kw], opts)) as string;
      let spans: ([number, number] | null)[][];
      let named: Record<string, [number, number] | null>[] | null;
      if (oracle) {
        // Python's matches, in code points: the core takes them as they are
        spans = oracle.spans;
        named = oracle.named ?? null;
      } else {
        ({ spans, named } = matchSpans(text, compilePattern(pattern, { regex: own.regex as boolean, case: own.case as boolean })));
      }
      return (await call(
        "page_search",
        this.handle,
        [kw, spans, own.return_groups ?? true, own.return_chars ?? true, own.main_group ?? 0, named, oracle !== undefined],
        opts,
      )) as SearchResult[];
    });
  }

  /** A derived page: the operation `op` (`page_crop`...), then its
   * geometry (`page_info`), in one job. */
  #derive(op: string, args: unknown, before?: (call: CoreCall) => Promise<unknown>): Promise<PDFPlumberPage> {
    return this.pdf.job(async (call) => {
      const opts = this.pdf.callOptions(this.page_number);
      const a = before ? await before(call) : args;
      const made = (await call(op, this.handle, a, opts)) as { handle: number; page_number: number; bbox: Bbox };
      let geometry: Geometry;
      let typed: unknown;
      let error: unknown;
      try {
        geometry = (await call("page_info", made.handle, null, opts)) as Geometry;
        typed = this.pdf.core.lastTyped;
      } catch (e) {
        // `page.width` raises: the geometry made by the derivation, its
        // width and height throwing
        geometry = { ...(this as unknown as Geometry), page_number: made.page_number, bbox: made.bbox, width: NaN, height: NaN };
        error = e;
      }
      return this.pdf.page_(geometry, made.handle, typed, this, error);
    });
  }

  crop(bbox: Bbox, options: CropOptions = {}): Promise<PDFPlumberPage> {
    return this.#derive("page_crop", [bbox, options]);
  }
  withinBbox(bbox: Bbox, options: CropOptions = {}): Promise<PDFPlumberPage> {
    return this.#derive("page_within_bbox", [bbox, options]);
  }
  within_bbox(bbox: Bbox, options?: CropOptions): Promise<PDFPlumberPage> {
    return this.withinBbox(bbox, options);
  }
  outsideBbox(bbox: Bbox, options: CropOptions = {}): Promise<PDFPlumberPage> {
    return this.#derive("page_outside_bbox", [bbox, options]);
  }
  outside_bbox(bbox: Bbox, options?: CropOptions): Promise<PDFPlumberPage> {
    return this.outsideBbox(bbox, options);
  }

  /** `page.filter(test)`: `test` runs here on the parent's objects
   * (`page_objects`), the truth of each result goes to the core
   * (`page_filter`), in one job (docs/public-api.md). */
  filter(test: (obj: PDFObject) => unknown): Promise<PDFPlumberPage> {
    return this.#derive("page_filter", null, async (call) => {
      const objects = (await call("page_objects", this.handle, null, this.pdf.callOptions(this.page_number))) as Record<
        string,
        PDFObject[]
      >;
      const mask: Record<string, boolean[]> = {};
      for (const [kind, objs] of Object.entries(objects)) mask[kind] = objs.map((o) => Boolean(test(o)));
      return [mask];
    });
  }

  dedupeChars(options: DedupeOptions = {}): Promise<PDFPlumberPage> {
    return this.#derive("page_dedupe_chars", [options]);
  }
  dedupe_chars(options?: DedupeOptions): Promise<PDFPlumberPage> {
    return this.dedupeChars(options);
  }

  #tables(values: TableValue[]): Table[] {
    return values.map((t) => new Table(this, t.handle, t.bbox, t.cells, t.rows, t.columns));
  }

  /** `page.find_tables(table_settings)` */
  findTables(settings?: TableOptions | null): Promise<Table[]> {
    return this.#run<TableValue[]>("page_find_tables", settings === undefined ? [] : [settings]).then((v) => this.#tables(v));
  }
  find_tables(settings?: TableOptions | null): Promise<Table[]> {
    return this.findTables(settings);
  }
  /** `page.find_table(table_settings)` */
  findTable(settings?: TableOptions | null): Promise<Table | null> {
    return this.#run<TableValue | null>("page_find_table", settings === undefined ? [] : [settings]).then((v) =>
      v === null ? null : this.#tables([v])[0],
    );
  }
  find_table(settings?: TableOptions | null): Promise<Table | null> {
    return this.findTable(settings);
  }
  extractTables(settings?: TableOptions | null): Promise<(string | null)[][][]> {
    return this.#run("page_extract_tables", settings === undefined ? [] : [settings]);
  }
  extract_tables(settings?: TableOptions | null): Promise<(string | null)[][][]> {
    return this.extractTables(settings);
  }
  extractTable(settings?: TableOptions | null): Promise<(string | null)[][] | null> {
    return this.#run("page_extract_table", settings === undefined ? [] : [settings]);
  }
  extract_table(settings?: TableOptions | null): Promise<(string | null)[][] | null> {
    return this.extractTable(settings);
  }

  /** `page.debug_tablefinder(table_settings)`: waits for the boundary
   * operation (docs/architecture.md, "Vague 5", BOUNDARY). */
  debugTablefinder(_settings?: TableOptions | null): Promise<never> {
    return Promise.reject(new NotImplemented("PDFPlumberPage.debugTablefinder"));
  }
  debug_tablefinder(settings?: TableOptions | null): Promise<never> {
    return this.debugTablefinder(settings);
  }

  /** `page.to_dict(object_types)` */
  toDict(objectTypes?: string[] | null): Promise<Record<string, unknown>> {
    return this.#run("page_to_dict", [objectTypes === undefined ? {} : { object_types: objectTypes }]);
  }
  to_dict(objectTypes?: string[] | null): Promise<Record<string, unknown>> {
    return this.toDict(objectTypes);
  }
  toJson(options: SerializeOptions = {}): Promise<string> {
    return this.#run("page_to_json", [options]);
  }
  to_json(options?: SerializeOptions): Promise<string> {
    return this.toJson(options);
  }
  toCsv(options: SerializeOptions = {}): Promise<string> {
    return this.#run("page_to_csv", [options]);
  }
  to_csv(options?: SerializeOptions): Promise<string> {
    return this.toCsv(options);
  }

  /** `Page.close()`: flushes the page's caches (it stays usable). */
  close(): Promise<void> {
    return this.#run("page_close", []);
  }
}
