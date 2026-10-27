/**
 * The internal hooks of the public API (never documented as public): what
 * the harness that writes stage traces through the public API (js/trace/,
 * docs/verification.md, "Parity Through The JavaScript API") gives the API
 * where the public semantics intentionally differ from Python's, and how it
 * reads typed values. Reached through `pdfplumber-js/internal` only.
 *
 * - `ORACLE` in the options of `open()`: `{ ids }`, Python's `id()`
 *   ranking of each page's layout analysis (the `ids` lines of the
 *   `plumber_objects` oracle trace), given to the core's oracle hooks
 *   (`oracle_doc_open`, crates/wasm/src/oracle.rs: only in
 *   dist/pdfplumber-oracle.wasm); the public API ranks by creation order.
 * - `ORACLE` in the options of `search()`: `{ spans, named }`, the matches
 *   of Python's `re` (code point offsets, as `plumber_text` writes them),
 *   used in place of the JavaScript `RegExp`'s.
 * - `TYPED` on a page: the typed value (core/typed.ts) of the geometry the
 *   page was made from, when the core writes typed results.
 */

export const ORACLE: unique symbol = Symbol("pdfplumber-js oracle hooks");
export const TYPED: unique symbol = Symbol("pdfplumber-js typed value");

/** `ORACLE` of `open()`. */
export interface OracleOpen {
  /** page number -> `[[key, rank], ...]` */
  ids: Record<string, [string, number][]>;
}

/** `ORACLE` of `search()`. */
export interface OracleSearch {
  /** For each match, the span of each group in code points (`null`: the
   * group did not take part), as `re.finditer` gives them. */
  spans: ([number, number] | null)[][];
  /** For each match, its named groups' spans, or `null` (no named group). */
  named?: Record<string, [number, number] | null>[] | null;
}
