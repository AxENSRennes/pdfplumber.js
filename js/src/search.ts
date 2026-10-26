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

/** The `TypeError` of a method called without a required argument
 * (`Page.search()`: "Page.search() missing 1 required positional argument:
 * 'pattern'"), raised by the call, before its body runs. */
export function missingArgument(qualname: string, name: string): PDFPlumberError {
  return pythonBuiltinError("TypeError", `${qualname}() missing 1 required positional argument: '${name}'`);
}

/** Whether `search` runs a pattern itself (a string or a `RegExp`); any
 * other value goes to the core, where `TextMap.search` raises Python's
 * error for it (`re.compile`'s, `re.escape`'s). */
export function runnablePattern(pattern: unknown): pattern is string | RegExp {
  return typeof pattern === "string" || pattern instanceof RegExp;
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

type Span = [number, number] | null;

/** The matches of `re` over `text`, in `matchAll` order: the spans of each
 * (UTF-16 offsets; `null` for a group that did not take part) and, for a
 * pattern with named groups, the spans of those by name
 * (`indices.groups`), what `m.group(name)` reads; `named` is `null` for a
 * pattern without any (Python then refuses any name). */
export function matchSpans(text: string, re: RegExp): { spans: Span[][]; named: Record<string, Span>[] | null } {
  const spans: Span[][] = [];
  const named: Record<string, Span>[] = [];
  let hasNames = false;
  const span = (s: [number, number] | undefined): Span => (s ? [s[0], s[1]] : null);
  for (const m of text.matchAll(re)) {
    const indices = m.indices;
    if (!indices) throw new Error("RegExp without the d flag");
    spans.push(Array.from(indices, span));
    const groups = indices.groups;
    hasNames ||= groups !== undefined;
    const byName: Record<string, Span> = {};
    for (const name of Object.keys(groups ?? {})) {
      Object.defineProperty(byName, name, { value: span(groups![name]), enumerable: true, writable: true });
    }
    named.push(byName);
  }
  return { spans, named: hasNames ? named : null };
}
