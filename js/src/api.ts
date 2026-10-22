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
  const [, kwargs] = splitOptions(options, OPEN_OWN);
  return kwargs as Record<string, unknown>;
}

/** The API's own options of `open()` (`logLevel`, `onWarning`), from a
 * plain object or a `Map`. */
function apiOptions(options: OpenOptions): OpenOptions {
  return splitOptions(options, OPEN_OWN)[0] as OpenOptions;
}

/** The options of `open()` that are the API's own. */
const OPEN_OWN = new Set(["logLevel", "onWarning"]);

function setOwn(o: Record<string, unknown>, k: string, v: unknown): void {
  // a key such as `__proto__` is an own property, as in a Python dict
  Object.defineProperty(o, k, { value: v, enumerable: true, writable: true, configurable: true });
}

/** An options argument split into the options named in `own` and the
 * others (`**kwargs`). Options are a dict: a plain object (its own
 * enumerable properties) or a `Map` with string keys (docs/public-api.md,
 * "Arguments"). Any other value is not split: it goes to the core as the
 * `**kwargs` it is, where Python's `TypeError` ("argument after ** must be
 * a mapping") is raised. */
function splitOptions(options: unknown, own: ReadonlySet<string>): [Record<string, unknown>, unknown] {
  if (options === null || typeof options !== "object" || Array.isArray(options)) return [{}, options];
  const entries = options instanceof Map ? [...options.entries()] : Object.entries(options);
  const mine: Record<string, unknown> = {};
  const kw: Record<string, unknown> = {};
  for (const [k, v] of entries) {
    if (typeof k !== "string") throw new TypeError("dict keys must be strings");
    setOwn(own.has(k) ? mine : kw, k, v);
  }
  return [mine, kw];
}

/** A document released (`release()`, `await using`) is no longer in the
 * core: a JavaScript lifetime, which Python has no counterpart of. */
export class DocumentReleased extends Error {
  constructor() {
    super("the document was released (release() or await using): open it again");
    this.name = "DocumentReleased";
  }
}

/** Whether a document is still in its core: shared by the document and the
 * finalizers of its pages and tables (a finalizer must not free a handle
 * of a released document, whose number the core may have given to
 * another object since). */
interface Life {
  released: boolean;
}

/** What a finalizer frees: a handle of a core, while its document lives. */
interface Held {
  core: Core;
  handle: number;
  life: Life;
}

/** Frees the handle of an object that became unreachable (a document, a
 * derived page, a table): `free`, queued after the calls made before it.
 * Freeing never changes a result (the core keeps no Python-visible state
 * of a freed object; docs/js-runtime.md, "Handles libérés"). A document's
 * entry marks its life released first: the entries of its pages and tables
 * that run after it free nothing (the document took them along). */
const registry =
  typeof FinalizationRegistry === "function"
    ? new FinalizationRegistry<Held & { document?: true }>(({ core, handle, life, document }) => {
        if (life.released || core.crashed) return;
        if (document) life.released = true;
        core.call("free", handle, null).catch(() => undefined);
      })
    : undefined;

/** Opens a PDF (`pdfplumber.open`): a filesystem path (Node) as
 * `pdfplumber.open(path)`, any other input as
 * `pdfplumber.open(io.BytesIO(bytes))`. */
export async function open(input: PDFInput, options: OpenOptions = {}): Promise<PDFPlumberDocument> {
  // `null` is no option (`pdfplumber.open(x)`)
  options ??= {};
  const args = openKwargs(options);
  const p = platform();
  const path = p.filePath?.(input);
  const bytes = await p.readInput(input);
  const core = await getCore();
  const policy = diagnosticsPolicy(apiOptions(options));
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
  readonly #life: Life = { released: false };
  #pages: Promise<PDFPlumberPage[]> | undefined;
  /** The pages by handle (`PageRef` resolution): the pages of `pdf.pages`,
   * of `page(ref)`, and the derived pages a value may name (`pin`). */
  readonly #byHandle = new Map<number, PDFPlumberPage>();
  /** The other derived pages, which the document does not keep: freed in
   * the core once unreachable (`track`). */
  readonly #weak = new Map<number, WeakRef<PDFPlumberPage>>();
  /** The pages of Python's current `pdf.pages` list (`PDF._pages`, made
   * again after each `close()`) the API has objects of, by page number:
   * from `pdf.pages`, or from `page(ref)` of a reference read since the
   * last `close()` (`#refs`). */
  readonly #current = new Map<number, PDFPlumberPage>();
  /** How many times `close()` flushed the pages. */
  #epoch = 0;
  /** The epoch each `PageRef` of a result was read in: a reference read
   * since the last `close()` names a page of the current list. */
  readonly #refs = new WeakMap<PageRef, number>();
  readonly #token = {};
  /** The API's own options (`logLevel`, `onWarning`). */
  readonly #own: OpenOptions;

  /** @internal */
  constructor(
    /** @internal */ readonly core: Core,
    /** @internal */ readonly handle: number,
    /** The options of `open()`, as given. */
    readonly options: OpenOptions,
  ) {
    this.#own = apiOptions(options);
    registry?.register(this, { core, handle, life: this.#life, document: true }, this.#token);
  }

  /** @internal A job on this document: rejects once it is released. */
  job<T>(f: (call: CoreCall) => Promise<T>): Promise<T> {
    if (this.#life.released) return Promise.reject(new DocumentReleased());
    return this.core.exclusive((call) => {
      if (this.#life.released) throw new DocumentReleased();
      return f(call);
    });
  }

  /** @internal Frees the core's handle of `obj` (a derived page, a table)
   * once it is unreachable, while this document lives. */
  track(obj: object, handle: number): void {
    registry?.register(obj, { core: this.core, handle, life: this.#life }, obj);
  }

  /** @internal A derived page whose handle a value may hold (a `PageRef`
   * of its annotations, `CroppedPage.annots` naming the cropped page):
   * kept by the document, as the pages of `pdf.pages` are, so that
   * `page(ref)` gives it back and its handle is never freed while the
   * reference can be resolved. */
  pin(page: PDFPlumberPage): void {
    if (page.parent === undefined || this.#byHandle.get(page.handle) === page) return;
    registry?.unregister(page);
    this.#weak.delete(page.handle);
    this.#byHandle.set(page.handle, page);
  }

  /** @internal The call options of a request on this document (its
   * diagnostics, for page `page`). */
  callOptions(page?: number): CallOptions {
    return diagnosticsPolicy(this.#own, page);
  }

  /** @internal A page of this document from its geometry. */
  page_(geometry: Geometry, handle: number, typed: unknown, parent?: PDFPlumberPage, geometryError?: unknown): PDFPlumberPage {
    const known = this.#known(handle);
    if (known) return known;
    const page = new PDFPlumberPage(this, handle, geometry, typed, parent, geometryError);
    if (parent === undefined) {
      this.#byHandle.set(handle, page);
    } else {
      // a derived page: the caller keeps it (or its tables, or the pages
      // derived from it); its handle is freed once it is unreachable
      this.#weak.set(handle, new WeakRef(page));
      this.track(page, handle);
    }
    return page;
  }

  /** The live page of a handle, if this document gave one. */
  #known(handle: number): PDFPlumberPage | undefined {
    const strong = this.#byHandle.get(handle);
    if (strong) return strong;
    const weak = this.#weak.get(handle)?.deref();
    if (weak) return weak;
    this.#weak.delete(handle);
    return undefined;
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
      // a page `page(ref)` made of a reference read since the last
      // `close()` is one of these: the same object (as Python's
      // `annot["data"]["P"] is pdf.pages[i]`), called through the handle
      // it was made with
      return list.map((g, i) => {
        const page = this.#current.get(g.page_number) ?? this.page_(g, g.handle!, typed?.[i]);
        this.#current.set(g.page_number, page);
        return page;
      });
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
    if (!(ref instanceof PageRef)) return Promise.reject(new TypeError("page() takes a PageRef of this document"));
    // decided when the job runs, in the order of the calls (a `pages` or a
    // `close()` called before runs before)
    return this.job(async (call) => {
      const current = this.#refs.get(ref) === this.#epoch;
      const n = Number(ref.page_number);
      const known = this.#known(ref.handle) ?? (current ? this.#current.get(n) : undefined);
      if (known) return known;
      const g = (await call("page_info", ref.handle, null, this.callOptions())) as Geometry;
      const page = this.page_(g, ref.handle, this.core.lastTyped);
      if (current && !this.#current.has(n)) this.#current.set(n, page);
      return page;
    });
  }

  /** @internal Notes the `PageRef`s of a value that holds annotations (a
   * `Page` is only ever an annotation's `data.P`: `Page.annots` sets it)
   * as read now: `annots` a list of annotations, or a `to_dict` (`annots`,
   * and each page's for a document's). */
  readRefs(value: unknown, kind: "annots" | "page_dict" | "doc_dict"): void {
    const annots = (list: unknown): void => {
      if (!Array.isArray(list)) return;
      for (const a of list) {
        const p = (a as { data?: { P?: unknown } } | null)?.data?.P;
        if (p instanceof PageRef) this.#refs.set(p, this.#epoch);
      }
    };
    const page = (d: unknown): void => annots((d as { annots?: unknown } | null)?.annots);
    if (kind === "annots") annots(value);
    else if (kind === "page_dict") page(value);
    else for (const d of ((value as { pages?: unknown } | null)?.pages as unknown[] | undefined) ?? []) page(d);
  }

  #get<T>(op: string, args: unknown = null, refs?: "annots" | "doc_dict"): Promise<T> {
    return this.job(async (call) => {
      const value = await call(op, this.handle, args, this.callOptions());
      if (refs) this.readRefs(value, refs);
      return value as T;
    });
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
    return this.#get("doc_annots", null, "annots");
  }

  get hyperlinks(): Promise<PDFObject[]> {
    return this.#get("doc_hyperlinks", null, "annots");
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
    return this.#get("doc_to_dict", [objectTypes === undefined ? {} : { object_types: objectTypes }], "doc_dict");
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
      // the pages are made again at the next read (`PDF.close()` flushes
      // `_pages`); a derived page keeps the page it derives from, as in
      // Python, and stays what a value naming it resolves to
      this.#pages = undefined;
      for (const [h, page] of this.#byHandle) if (page.parent === undefined) this.#byHandle.delete(h);
      this.#current.clear();
      this.#epoch++;
    });
  }

  /** Releases the document in the core, with its pages and tables
   * (idempotent); any later call rejects with `DocumentReleased`. */
  async release(): Promise<void> {
    if (this.#life.released) return;
    this.#life.released = true;
    this.#pages = undefined;
    this.#byHandle.clear();
    this.#weak.clear();
    this.#current.clear();
    registry?.unregister(this.#token);
    if (!this.core.crashed) await this.core.call("free", this.handle, null);
  }

  /** `await using pdf = await open(...)`: `close()`, then `release()`, as
   * `with pdfplumber.open(...) as pdf:` closes it. */
  async [Symbol.asyncDispose](): Promise<void> {
    try {
      if (!this.#life.released && !this.core.crashed) await this.close();
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

/** The arguments of `extract_text_lines` before `**kwargs`. */
const TEXT_LINE_ARGS = new Set(["strip", "return_chars"]);

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

  #run<T>(op: string, args: unknown, refs?: "annots" | "page_dict"): Promise<T> {
    return this.pdf.job(async (call) => {
      const value = await call(op, this.handle, args, this.pdf.callOptions(this.page_number));
      if (refs) this.pdf.readRefs(value, refs);
      return value as T;
    });
  }

  get objects(): Promise<Record<string, PDFObject[]>> {
    return this.#run("page_objects", null);
  }

  #list(name: ListName): Promise<PDFObject[]> {
    return this.#run("page_attr", [name], name === "annots" || name === "hyperlinks" ? "annots" : undefined);
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
  /** `page.annots`: each annotation's `data.P` is this page (a `PageRef`;
   * for a derived page, the derived page itself: `pin`). */
  get annots(): Promise<PDFObject[]> {
    this.pdf.pin(this);
    return this.#list("annots");
  }
  get hyperlinks(): Promise<PDFObject[]> {
    this.pdf.pin(this);
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
    let own: Record<string, unknown>, kw: unknown;
    try {
      [own, kw] = splitOptions(options, TEXT_LINE_ARGS);
    } catch (e) {
      return Promise.reject(e);
    }
    // (an option `undefined` is not given: its default)
    return this.#run("page_extract_text_lines", [own.strip ?? true, own.return_chars ?? true, kw]);
  }
  extract_text_lines(options?: TextLineOptions): Promise<SearchResult[]> {
    return this.extractTextLines(options);
  }

  /** `page.search(pattern, **options)` with JavaScript `RegExp` semantics
   * (docs/public-api.md, "Search"): the text the pattern runs over
   * (`page_search_text`), the pattern run over it here, then the results
   * of the spans (`page_search`), in one job. */
  search(pattern: string | RegExp, options: SearchOptions = {}): Promise<SearchResult[]> {
    const oracle = (options as { [ORACLE]?: OracleSearch } | null)?.[ORACLE];
    let own: Record<string, unknown>, kw: unknown;
    try {
      [own, kw] = splitOptions(options, SEARCH_ARGS);
    } catch (e) {
      return Promise.reject(e);
    }
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

  /** @internal The `Table` objects of the tables the core found, their
   * handles freed once they are unreachable. */
  tables_(values: TableValue[]): Table[] {
    return values.map((t) => {
      const table = new Table(this, t.handle, t.bbox, t.cells, t.rows, t.columns);
      this.pdf.track(table, t.handle);
      return table;
    });
  }

  /** `page.find_tables(table_settings)` */
  findTables(settings?: TableOptions | null): Promise<Table[]> {
    return this.#run<TableValue[]>("page_find_tables", settings === undefined ? [] : [settings]).then((v) => this.tables_(v));
  }
  find_tables(settings?: TableOptions | null): Promise<Table[]> {
    return this.findTables(settings);
  }
  /** `page.find_table(table_settings)` */
  findTable(settings?: TableOptions | null): Promise<Table | null> {
    return this.#run<TableValue | null>("page_find_table", settings === undefined ? [] : [settings]).then((v) =>
      v === null ? null : this.tables_([v])[0],
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
    // the annotations it may hold name this page (`annots`)
    this.pdf.pin(this);
    return this.#run("page_to_dict", [objectTypes === undefined ? {} : { object_types: objectTypes }], "page_dict");
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
