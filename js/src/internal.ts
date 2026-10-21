/**
 * Internal API, for the tests and the benchmark (`pdfplumber-js/internal`):
 * the operations of crates/wasm/src/ops.rs over objects given by
 * JavaScript, before the page layer exposes them through pages. Not part
 * of the public contract.
 */

import { diagnosticsPolicy, dropCore, getCore } from "./api.js";
import { WasmCore, decodeResponse, encodeRequest, type CallOptions, type Core } from "./core/runtime.js";
import { compilePattern, matchSpans, type PatternOptions } from "./search.js";
import type { Bbox, PDFObject, SearchResult, TableAxisGroup, Word } from "./types.js";

export { WasmCore, decodeResponse, encodeRequest, getCore, dropCore, diagnosticsPolicy };
export { decodeBinary, decodeJson, encodeValue, encodeGeneralizedUtf8, decodeGeneralizedUtf8 } from "./core/wire.js";
export { compilePattern, matchSpans, escapeRegExp } from "./search.js";
export { CMAP_NAMES } from "./generated/cmap-names.js";

export interface InternalTable {
  bbox: Bbox;
  cells: Bbox[];
  rows: TableAxisGroup[];
  columns: TableAxisGroup[];
  text: (string | null)[][];
}

/** A `TextMap` kept by the core. */
export class TextMapHandle {
  constructor(
    private readonly core: Core,
    readonly handle: number,
    /** `TextMap.as_string` */
    readonly text: string,
  ) {}

  /** `TextMap.search` with a JavaScript pattern. `main_group` is any
   * value Python's `m.group()` takes (a number, a group name...). */
  async search(
    pattern: string | RegExp,
    options: PatternOptions & { return_groups?: boolean; return_chars?: boolean; main_group?: unknown } = {},
  ): Promise<SearchResult[]> {
    const { spans, named } = matchSpans(this.text, compilePattern(pattern, options));
    return (await this.core.call("textmap_search", 0, [
      this.handle,
      spans,
      options.return_groups ?? true,
      options.return_chars ?? true,
      options.main_group === undefined ? 0 : options.main_group,
      named,
    ])) as SearchResult[];
  }

  /** `TextMap.extract_text_lines(strip, return_chars)` */
  async lines(strip = true, returnChars = true): Promise<SearchResult[]> {
    return (await this.core.call("textmap_lines", 0, [this.handle, strip, returnChars])) as SearchResult[];
  }

  async free(): Promise<void> {
    await this.core.call("free", this.handle, null);
  }
}

/** The internal operations, on the current core. */
export async function internals(options: CallOptions = {}) {
  const core = await getCore();
  const call = (op: string, args: unknown = null, handle = 0, extra: CallOptions = {}) =>
    core.call(op, handle, args, { ...options, ...extra });
  return {
    core,
    call,
    info: () => call("info") as Promise<Record<string, unknown>>,
    setFormat: (format: "binary" | "json") => call("set_format", format),
    echo: (value: unknown) => call("echo", value),
    testValues: () => call("test_values") as Promise<Record<string, unknown>>,
    exceptionKinds: () => call("exception_kinds") as Promise<string[]>,
    raise: (cls: string, message: string) => call("raise", [cls, message]),
    warn: (entries: [string, number, string][], extra: CallOptions = {}) => call("warn", entries, 0, extra),
    cmapDecode: (name: string, code: Uint8Array) => call("cmap_decode", [name, code]) as Promise<number[]>,
    cmapUnichr: (collection: string, vertical: boolean, cids: number[]) =>
      call("cmap_unichr", [collection, vertical, cids]) as Promise<(string | [string, string])[]>,
    extractText: (chars: PDFObject[], kwargs: Record<string, unknown> = {}) =>
      call("extract_text", [chars, kwargs]) as Promise<string>,
    extractWords: (chars: PDFObject[], kwargs: Record<string, unknown> = {}) =>
      call("extract_words", [chars, kwargs]) as Promise<Word[]>,
    dedupeChars: (chars: PDFObject[], tolerance: unknown = 1, extraAttrs: string[] | null = ["fontname", "size"]) =>
      call("dedupe_chars", [chars, tolerance, extraAttrs]) as Promise<PDFObject[]>,
    async textmap(chars: PDFObject[], kwargs: Record<string, unknown> = {}): Promise<TextMapHandle> {
      const [handle, text] = (await call("textmap", [chars, kwargs])) as [number, string];
      return new TextMapHandle(core, handle, text);
    },
    findTables: (
      chars: PDFObject[],
      edges: PDFObject[],
      bbox: unknown,
      settings: Record<string, unknown> | null = null,
      extractKwargs: Record<string, unknown> = {},
    ) => call("find_tables", [chars, edges, bbox, settings, extractKwargs]) as Promise<InternalTable[]>,
    free: (handle: number) => call("free", null, handle),
  };
}
