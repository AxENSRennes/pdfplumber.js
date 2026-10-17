/**
 * The public API (docs/public-api.md): `open()`, `PDFPlumberDocument`,
 * `PDFPlumberPage`. The document is opened by the Rust core; the page layer
 * of the port has not landed yet, so the page-level features reject with
 * `NotImplemented` (docs/port-strategy.md, "État").
 */

import { NotImplemented } from "./errors.js";
import { WasmCore, type Core, type Diagnostic } from "./core/runtime.js";
import { platform } from "./platform/platform.js";
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
  Table,
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

/** The core the next document opens in (a new one after a crash). */
export async function getCore(): Promise<Core> {
  if (corePromise) {
    const core = await corePromise.catch(() => undefined);
    if (core && !core.crashed) return core;
  }
  const options = runtimeOptions;
  corePromise = (async () => {
    const p = platform();
    const module = await p.compileWasm(options.wasm);
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

/** The keyword arguments of `pdfplumber.open` among the options, as they
 * are: pdfplumber checks none of them at `open()`, and the ported code
 * raises Python's errors where Python does (an `unicode_norm` it does not
 * know when the text is normalized, `pages` when the pages load...). */
function openKwargs(options: OpenOptions): Record<string, unknown> {
  const { pages, password, laparams, unicode_norm, raise_unicode_errors, strict_metadata } = options;
  return { pages, password, laparams, unicode_norm, raise_unicode_errors, strict_metadata };
}

const registry =
  typeof FinalizationRegistry === "function"
    ? new FinalizationRegistry<{ core: Core; handle: number }>(({ core, handle }) => {
        if (!core.crashed) core.call("free", handle, null).catch(() => undefined);
      })
    : undefined;

/** Opens a PDF (`pdfplumber.open`). */
export async function open(input: PDFInput, options: OpenOptions = {}): Promise<PDFPlumberDocument> {
  const args = openKwargs(options);
  const bytes = await platform().readInput(input);
  const core = await getCore();
  const policy = diagnosticsPolicy(options);
  const handle = (await core.call("doc_open", 0, args, { data: bytes, ...policy })) as number;
  return new PDFPlumberDocument(core, handle, options);
}

function notImplemented<T>(what: string): Promise<T> {
  return Promise.reject(new NotImplemented(what));
}

/** An open PDF (`pdfplumber.PDF`). */
export class PDFPlumberDocument {
  #closed = false;
  readonly #token = {};

  /** @internal */
  constructor(
    private readonly core: Core,
    private readonly handle: number,
    readonly options: OpenOptions,
  ) {
    registry?.register(this, { core, handle }, this.#token);
  }

  /** The loaded pages (`pdf.pages`). */
  get pages(): PDFPlumberPage[] {
    throw new NotImplemented("PDFPlumberDocument.pages");
  }

  /** The Info dictionary (`pdf.metadata`). */
  get metadata(): Record<string, unknown> {
    throw new NotImplemented("PDFPlumberDocument.metadata");
  }

  /** `pdf.objects`: the objects of every loaded page, by `object_type`. */
  get objects(): Promise<Record<string, PDFObject[]>> {
    return notImplemented("PDFPlumberDocument.objects");
  }

  get annots(): Promise<PDFObject[]> {
    return notImplemented("PDFPlumberDocument.annots");
  }

  get hyperlinks(): Promise<PDFObject[]> {
    return notImplemented("PDFPlumberDocument.hyperlinks");
  }

  get edges(): Promise<PDFObject[]> {
    return notImplemented("PDFPlumberDocument.edges");
  }

  get horizontal_edges(): Promise<PDFObject[]> {
    return notImplemented("PDFPlumberDocument.horizontal_edges");
  }

  get vertical_edges(): Promise<PDFObject[]> {
    return notImplemented("PDFPlumberDocument.vertical_edges");
  }

  /** Releases the document in the core (idempotent). */
  async close(): Promise<void> {
    if (this.#closed) return;
    this.#closed = true;
    registry?.unregister(this.#token);
    if (!this.core.crashed) await this.core.call("free", this.handle, null);
  }

  async [Symbol.asyncDispose](): Promise<void> {
    await this.close();
  }
}

/** A page (`pdfplumber.page.Page`), or a page derived from one (`crop`,
 * `filter`...). Coordinates follow pdfplumber (top-left origin). */
export class PDFPlumberPage {
  /** @internal */
  protected constructor(
    readonly page_number: number,
    readonly width: number,
    readonly height: number,
    readonly bbox: Bbox,
  ) {}

  get pageNumber(): number {
    return this.page_number;
  }

  get objects(): Promise<Record<string, PDFObject[]>> {
    return notImplemented("PDFPlumberPage.objects");
  }
  get chars(): Promise<PDFObject[]> {
    return notImplemented("PDFPlumberPage.chars");
  }
  get rects(): Promise<PDFObject[]> {
    return notImplemented("PDFPlumberPage.rects");
  }
  get lines(): Promise<PDFObject[]> {
    return notImplemented("PDFPlumberPage.lines");
  }
  get curves(): Promise<PDFObject[]> {
    return notImplemented("PDFPlumberPage.curves");
  }
  get images(): Promise<PDFObject[]> {
    return notImplemented("PDFPlumberPage.images");
  }
  get annots(): Promise<PDFObject[]> {
    return notImplemented("PDFPlumberPage.annots");
  }
  get hyperlinks(): Promise<PDFObject[]> {
    return notImplemented("PDFPlumberPage.hyperlinks");
  }
  get edges(): Promise<PDFObject[]> {
    return notImplemented("PDFPlumberPage.edges");
  }

  extractText(_options?: ExtractTextOptions): Promise<string> {
    return notImplemented("PDFPlumberPage.extractText");
  }
  extract_text(options?: ExtractTextOptions): Promise<string> {
    return this.extractText(options);
  }
  extractWords(_options?: WordOptions): Promise<Word[]> {
    return notImplemented("PDFPlumberPage.extractWords");
  }
  extract_words(options?: WordOptions): Promise<Word[]> {
    return this.extractWords(options);
  }
  extractTextLines(_options?: TextLineOptions): Promise<SearchResult[]> {
    return notImplemented("PDFPlumberPage.extractTextLines");
  }
  extract_text_lines(options?: TextLineOptions): Promise<SearchResult[]> {
    return this.extractTextLines(options);
  }
  search(_pattern: string | RegExp, _options?: SearchOptions): Promise<SearchResult[]> {
    return notImplemented("PDFPlumberPage.search");
  }
  crop(_bbox: Bbox, _options?: CropOptions): PDFPlumberPage {
    throw new NotImplemented("PDFPlumberPage.crop");
  }
  withinBbox(_bbox: Bbox, _options?: CropOptions): PDFPlumberPage {
    throw new NotImplemented("PDFPlumberPage.withinBbox");
  }
  within_bbox(bbox: Bbox, options?: CropOptions): PDFPlumberPage {
    return this.withinBbox(bbox, options);
  }
  outsideBbox(_bbox: Bbox, _options?: CropOptions): PDFPlumberPage {
    throw new NotImplemented("PDFPlumberPage.outsideBbox");
  }
  outside_bbox(bbox: Bbox, options?: CropOptions): PDFPlumberPage {
    return this.outsideBbox(bbox, options);
  }
  filter(_test: (obj: PDFObject) => boolean): Promise<PDFPlumberPage> {
    return notImplemented("PDFPlumberPage.filter");
  }
  dedupeChars(_options?: DedupeOptions): PDFPlumberPage {
    throw new NotImplemented("PDFPlumberPage.dedupeChars");
  }
  dedupe_chars(options?: DedupeOptions): PDFPlumberPage {
    return this.dedupeChars(options);
  }
  findTable(_options?: TableOptions): Promise<Table | null> {
    return notImplemented("PDFPlumberPage.findTable");
  }
  find_table(options?: TableOptions): Promise<Table | null> {
    return this.findTable(options);
  }
  findTables(_options?: TableOptions): Promise<Table[]> {
    return notImplemented("PDFPlumberPage.findTables");
  }
  find_tables(options?: TableOptions): Promise<Table[]> {
    return this.findTables(options);
  }
  extractTable(_options?: TableOptions): Promise<(string | null)[][] | null> {
    return notImplemented("PDFPlumberPage.extractTable");
  }
  extract_table(options?: TableOptions): Promise<(string | null)[][] | null> {
    return this.extractTable(options);
  }
  extractTables(_options?: TableOptions): Promise<(string | null)[][][]> {
    return notImplemented("PDFPlumberPage.extractTables");
  }
  extract_tables(options?: TableOptions): Promise<(string | null)[][][]> {
    return this.extractTables(options);
  }
}
