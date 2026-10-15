/**
 * `search()` patterns: JavaScript `RegExp` semantics (docs/public-api.md,
 * "Search"). The core receives the match spans (UTF-16 offsets, converted
 * to code points by the core) and builds pdfplumber's results
 * (`TextMap.search` with the matches given).
 */

import { PDFPlumberError } from "./errors.js";

/** A builtin Python exception raised on the JavaScript side, as the core
 * would report it. */
export function pythonBuiltinError(name: "ValueError" | "TypeError", message: string): PDFPlumberError {
  return new PDFPlumberError({
    name,
    module: "builtins",
    qualname: name,
    mro: [`builtins.${name}`, "builtins.Exception", "builtins.BaseException", "builtins.object"],
    message,
  });
}

/** `re.escape` for a JavaScript pattern in `u` mode: escapes the syntax
 * characters. */
export function escapeRegExp(s: string): string {
  return s.replace(/[\\^$.*+?()[\]{}|/]/g, "\\$&");
}

export interface PatternOptions {
  regex?: boolean;
  case?: boolean;
}

/** The regular expression `search` runs: a `RegExp` with the flags it
 * has plus `g` and `d` (sticky dropped: `finditer` scans), or a string
 * compiled with `u` (and `i` when `case` is false), escaped when `regex` is
 * false. The errors are pdfplumber's (utils/text.py:172 TextMap.search). */
export function compilePattern(pattern: string | RegExp, options: PatternOptions = {}): RegExp {
  if (pattern instanceof RegExp) {
    if (options.regex === false)
      throw pythonBuiltinError("ValueError", "Cannot pass a compiled search pattern *and* regex=False together.");
    if (options.case === false)
      throw pythonBuiltinError("ValueError", "Cannot pass a compiled search pattern *and* case=False together.");
    const flags = new Set([...pattern.flags, "g", "d"]);
    flags.delete("y");
    return new RegExp(pattern.source, [...flags].join(""));
  }
  if (typeof pattern !== "string") throw new TypeError("search pattern must be a string or a RegExp");
  const source = options.regex === false ? escapeRegExp(pattern) : pattern;
  return new RegExp(source, options.case === false ? "gdui" : "gdu");
}

/** The spans of every match of `re` over `text` (UTF-16 offsets; `null`
 * for a group that did not take part), in `matchAll` order. */
export function matchSpans(text: string, re: RegExp): ([number, number] | null)[][] {
  const out: ([number, number] | null)[][] = [];
  for (const m of text.matchAll(re)) {
    const indices = m.indices;
    if (!indices) throw new Error("RegExp without the d flag");
    out.push(Array.from(indices, (s) => (s ? [s[0], s[1]] : null)));
  }
  return out;
}
