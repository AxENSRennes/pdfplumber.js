// Hand mutants of the public API layer (js/src/api.ts,
// js/src/api/structure.ts, js/src/search.ts, js/src/errors.ts), Stryker not
// being available offline: each mutant replaces one exact text of a source
// file (a branch removed or inverted, a constant, an operation, an
// argument), runs the tests that check the API against Python through the
// API itself (the sessions of oracle/unit/js_api.py, js/test/node/api.test.ts,
// and js/test/node/api-lifetime.test.ts; the diagnostics test of the runtime
// for the diagnostics policy), and is "killed" when they fail. The source is
// restored after each run. Accepted survivors are listed in
// docs/verification.md, "Mutation Testing" ("Hand Mutants"), each with its
// justification.
//
//   node scripts/js-api-mutants.mjs <checkout> [ids...]    (a copy of the checkout:
//   the sources are mutated in place; dist/ built, unit data generated)
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const root = process.argv[2];
const only = new Set(process.argv.slice(3));

const API = ["js/test/node/api.test.ts", "js/test/node/api-lifetime.test.ts", "js/test/node/api-runtime.test.ts"];
const DIAG = ["js/test/node/runtime.test.ts", "-t", "onWarning|diagnostics reach"];
const API_OPTS = ["js/test/node/api-runtime.test.ts"];

const A = "js/src/api.ts";
const S = "js/src/api/structure.ts";
const R = "js/src/search.ts";
const E = "js/src/errors.ts";

/** [file, text, replacement, tests?] */
const mutants = [
  // the finalizers
  [A, "if (life.released || core.crashed) return;", "if (core.crashed) return;"],
  [A, "if (document) life.released = true;", ""],
  [A, '        core.call("free", handle, null).catch(() => undefined);', ""],
  // options
  [A, "if (!(options instanceof Map) && !isPlainObject(options)) return [{}, options];", "if (options === null || typeof options !== \"object\" || Array.isArray(options)) return [{}, options];"],
  [A, "if (!(options instanceof Map) && !isPlainObject(options)) return [{}, options];", "if (options === null) return [{}, options];"],
  [A, "const entries = options instanceof Map ? [...options.entries()] : Object.entries(options);", "const entries = Object.entries(options);"],
  [A, "setOwn(own.has(k) ? mine : kw, k, v);", "setOwn(kw, k, v);"],
  [A, "setOwn(own.has(k) ? mine : kw, k, v);", "setOwn(own.has(k) ? kw : mine, k, v);"],
  [A, "Object.defineProperty(o, k, { value: v, enumerable: true, writable: true, configurable: true });", "Object.defineProperty(o, k, { value: v, enumerable: false, writable: true, configurable: true });"],
  [A, 'const OPEN_OWN = new Set(["logLevel", "onWarning"]);', 'const OPEN_OWN = new Set(["logLevel"]);'],
  [A, "  options ??= {};\n", ""],
  [A, "const policy = diagnosticsPolicy(apiOptions(options));", "const policy = diagnosticsPolicy({});", DIAG],
  [A, "this.#own = apiOptions(options);", "this.#own = {};", DIAG],
  [A, 'const minLevel = logLevel === "debug" ? 10 : logLevel === "warning" || options.onWarning ? 30 : 0;', 'const minLevel = logLevel === "debug" ? 10 : logLevel === "warning" ? 30 : 0;', DIAG],
  [A, "if (page !== undefined) w.page = page;", "", DIAG],
  [A, "options.onWarning?.(w);", "", DIAG],
  [A, "else if (path !== undefined) handle", "else if (false) handle"],
  // the document
  [A, "    if (this.#life.released) return Promise.reject(new DocumentReleased());\n    if (tracked", "    if (tracked"],
  [A, "if (page.parent === undefined || this.#byHandle.get(page.handle) === page) return;", "return;"],
  [A, "    registry?.unregister(page);\n", ""],
  [A, "      this.#weak.set(handle, new WeakRef(page));\n      this.track(page, handle);", "      this.#weak.set(handle, new WeakRef(page));"],
  [A, "    const weak = this.#weak.get(handle)?.deref();\n    if (weak) return weak;\n", ""],
  [A, "const page = this.#current.get(g.page_number) ?? this.page_(g, g.handle!, typed?.[i]);", "const page = this.page_(g, g.handle!, typed?.[i]);"],
  [A, "        this.#current.set(g.page_number, page);\n", ""],
  [A, "      this.#pages = pages;\n", ""],
  [A, "      if (this.#pages) return this.#pages;\n", ""],
  [A, "if (!(ref instanceof PageRef) || pageRefOwner(ref) !== this)", "if (!(ref instanceof PageRef))"],
  [A, "const current = this.#refs.get(ref) === this.#epoch;", "const current = true;"],
  [A, "const current = this.#refs.get(ref) === this.#epoch;", "const current = false;"],
  [A, "const known = this.#known(ref.handle) ?? (current ? this.#current.get(n) : undefined);", "const known = this.#known(ref.handle);"],
  [A, "if (current && !this.#current.has(n)) this.#current.set(n, page);", ""],
  [A, "if (p instanceof PageRef) this.#refs.set(p, this.#epoch);", ""],
  [A, 'if (kind === "annots") annots(value);', 'if (kind === "annots") page(value);'],
  [A, 'else if (kind === "page_dict") page(value);', ""],
  [A, "else for (const d of ((value as { pages?: unknown } | null)?.pages as unknown[] | undefined) ?? []) page(d);", ""],
  [A, "      if (refs) this.readRefs(value, refs);", ""],
  [A, "      if (refs) this.pdf.readRefs(value, refs);", ""],
  [A, 'return this.#get("doc_annots", null, "annots");', 'return this.#get("doc_annots");'],
  [A, 'return this.#run("page_attr", [name], name === "annots" || name === "hyperlinks" ? "annots" : undefined);', 'return this.#run("page_attr", [name]);'],
  [A, "if (page !== undefined && page !== null && !(page instanceof PDFPlumberPage && page.pdf === this))", "if (page !== undefined && page !== null && !(page instanceof PDFPlumberPage))"],
  [A, "[page ? page.handle : null]", "[null]"],
  [A, "return new PDFStructTree(this, page ?? null, v.handle, v.children);", "return new PDFStructTree(this, null, v.handle, v.children);"],
  [A, "      this.#pages = undefined;\n      for (const [h, page] of this.#byHandle) {", "      for (const [h, page] of this.#byHandle) {"],
  [A, "        if (page.parent !== undefined) continue;\n", ""],
  [A, "        this.#weak.set(h, new WeakRef(page));\n", ""],
  [A, "      this.#current.clear();\n      this.#epoch++;", "      this.#epoch++;"],
  [A, "      this.#current.clear();\n      this.#epoch++;", "      this.#current.clear();"],
  [A, "    if (this.#life.released) return;\n    this.#life.released = true;", "    this.#life.released = true;"],
  [A, "    registry?.unregister(this.#token);\n", ""],
  [A, 'if (!this.core.crashed) await this.core.call("free", this.handle, null);', ""],
  [A, "if (!this.#life.released && !this.core.crashed) await this.close();", ""],
  // the page
  [A, 'if ("trimbox" in geometry) this.trimbox = geometry.trimbox;', ""],
  [A, "this.#widthError = geometry.width_error ? fromPython(geometry.width_error) : undefined;", "this.#widthError = undefined;"],
  [A, "this.#heightError = geometry.height_error ? fromPython(geometry.height_error) : undefined;", "this.#heightError = geometry.width_error ? fromPython(geometry.width_error) : undefined;"],
  [A, "return `<Page:${this.page_number}>`;", "return `<Page ${this.page_number}>`;"],
  [A, "    this.pdf.pin(this);\n    return this.#list(\"annots\");", "    return this.#list(\"annots\");"],
  [A, "[given(own.strip, true), given(own.return_chars, true), kw]", "[given(own.strip, false), given(own.return_chars, true), kw]"],
  [A, "[given(own.strip, true), given(own.return_chars, true), kw]", "[given(own.strip, true), given(own.return_chars, false), kw]"],
  [A, 'const SEARCH_ARGS = new Set(["regex", "case", "main_group", "return_chars", "return_groups"]);', 'const SEARCH_ARGS = new Set(["regex", "case", "main_group", "return_chars"]);'],
  [A, "given(own.return_groups, true), given(own.return_chars, true), given(own.main_group, 0)", "given(own.return_groups, true), given(own.return_chars, true), given(own.main_group, 1)"],
  [A, "given(own.return_groups, true), given(own.return_chars, true), given(own.main_group, 0)", "given(own.return_groups, false), given(own.return_chars, true), given(own.main_group, 0)"],
  [A, "compilePattern(pattern, { regex: own.regex as boolean, case: own.case as boolean })", "compilePattern(pattern, { regex: own.regex as boolean })"],
  [A, "const a = before ? await before(call) : args;", "const a = args;"],
  [A, "return this.pdf.page_(geometry, made.handle, this.pdf.core.lastTyped, this);", "return this.pdf.page_(geometry, made.handle, this.pdf.core.lastTyped, undefined);"],
  [A, "mask[kind] = objs.map((o) => Boolean(test(o)));", "mask[kind] = objs.map((o) => !test(o));"],
  [A, "      this.pdf.track(table, t.handle);\n", ""],
  [A, 'return this.#run<TableValue[]>("page_find_tables", [settings ?? null])', 'return this.#run<TableValue[]>("page_find_tables", [null])'],
  [A, "v === null ? null : this.tables_([v])[0],", "v === null ? null : this.tables_([v])[1],"],
  [A, "      tables: this.tables_(v.tables),\n", "      tables: [],\n"],
  [A, "    // the annotations it may hold name this page (`annots`)\n    this.pdf.pin(this);\n", ""],
  [A, 'return this.#run("page_close", []);', 'return this.#run("page_to_dict", [{}]);'],
  // the structure tree
  [S, 'const re = new RegExp(matcher.source, matcher.flags.replace(/[gy]/g, "") + "y");', 'const re = new RegExp(matcher.source, matcher.flags.replace(/[gy]/g, ""));'],
  [S, "        re.lastIndex = 0;\n", ""],
  [S, "return [null, preorder(tree.children).map((el) => Boolean(matcher(el)))];", "return [null, preorder(tree.children).map((el) => !matcher(el))];"],
  [S, "    for (let i = el.children.length - 1; i >= 0; i--) stack.push(el.children[i]);", "    for (let i = 0; i < el.children.length; i++) stack.push(el.children[i]);"],
  [S, "      for (let i = value.children.length - 1; i >= 0; i--) stack.push([value.children[i], [...path, i], el.children]);", "      for (let i = value.children.length - 1; i >= 0; i--) stack.push([value.children[i], [i], el.children]);"],
  [S, "    stack.reverse();\n", ""],
  [S, "    pdf.track(this, handle);\n", ""],
  [S, "if (all) return (found as Path[]).map((p) => this.#at(p));", "if (all) return (found as Path[]).map((p) => this.#at(p.slice(0, 1)));"],
  [S, "return found === null ? null : this.#at(found as Path);", "return found === null ? null : this.#at([0]);"],
  [S, "if (!(el instanceof PDFStructElement) || el.tree !== this)", "if (!(el instanceof PDFStructElement))"],
  [S, 'return this.tree.run_("struct_all_mcids", [this.path])', 'return this.tree.run_("struct_all_mcids", [[0]])'],
  [S, 'return this.tree.run_("struct_element_repr", [this.path])', 'return this.tree.run_("struct_element_repr", [this.path.slice(0, 1)])'],
  [S, "return this.tree.find_(this.path, matcher, false)", "return this.tree.find_(null, matcher, false)"],
  [S, "this.alt_text = value.alt_text;", "this.alt_text = value.actual_text;"],
  [S, "pdf.callOptions(this.page?.page_number)));\n  }", "pdf.callOptions()));\n  }"],
  // search patterns
  [R, 'const flags = new Set([...pattern.flags, "g", "d"]);', 'const flags = new Set([...pattern.flags, "g"]);'],
  [R, '    flags.delete("y");\n', ""],
  [R, 'return new RegExp(source, options.case === false ? "gdui" : "gdu");', 'return new RegExp(source, options.case === false ? "gdi" : "gd");'],
  [R, "const source = options.regex === false ? escapeRegExp(pattern) : pattern;", "const source = pattern;"],
  [R, "if (options.regex === false)\n      throw", "if (false)\n      throw"],
  [R, "if (options.case === false)\n      throw", "if (false)\n      throw"],
  [R, "hasNames ||= groups !== undefined;", "hasNames = true;"],
  [R, "return { spans, named: hasNames ? named : null };", "return { spans, named: null };"],
  [R, "const span = (s: [number, number] | undefined): Span => (s ? [s[0], s[1]] : null);", "const span = (s: [number, number] | undefined): Span => (s ? [s[0], s[1]] : [0, 0]);"],
  // errors
  [E, "super(info.message, info.cause ? { cause: fromPython(info.cause) } : undefined);", "super(info.message);"],
  [E, "this.name = info.name;", 'this.name = "PDFPlumberError";'],
  [E, 'if (info.module === "pycompat" && info.name === "NotPorted") return new NotImplemented(info);', ""],
  [E, "return this.pythonMro.some((q) => q === cls || q.slice(q.lastIndexOf(\".\") + 1) === cls);", "return this.pythonMro.some((q) => q === cls);"],
  // added on resuming: branches the first list left out
  [A, 'if (logLevel === "debug" || (logLevel === "warning" && d.level >= 30)) {', "if (false) {", API_OPTS],
  [A, "const log = d.level >= 30 ? console.warn : console.debug;", "const log = console.debug;", API_OPTS],
  [A, '      if (dropped) options.onWarning?.({ source: "pdfplumber-js", level: "warning", message: `${dropped} more diagnostics dropped` });\n', "", API_OPTS],
  [A, "if (core && !core.crashed) return core;", "if (core) return core;", API_OPTS],
  [A, "if (compiled && compiled.key === key) return compiled.module;", "", API_OPTS],
  [A, 'return this.#derive("page_within_bbox", [bbox, options]);', 'return this.#derive("page_outside_bbox", [bbox, options]);'],
  [A, 'return this.#derive("page_dedupe_chars", [options]);', 'return this.#derive("page_dedupe_chars", [{}]);'],
  [A, 'return this.#run("page_extract_text_simple", [options]);', 'return this.#run("page_extract_text", [options]);'],
  [A, 'async (call) => (await call("table_extract", this.handle, [options], doc.callOptions(this.page.page_number))) as (string | null)[][],', 'async (call) => (await call("table_extract", this.handle, [{}], doc.callOptions(this.page.page_number))) as (string | null)[][],'],
  [A, "const value = await call(op, this.handle, args, this.pdf.callOptions(this.page_number));", "const value = await call(op, this.handle, args, this.pdf.callOptions());", DIAG],
  [A, "    } finally {\n      await this.release();\n    }", "    } finally {\n    }"],
  [A, "const known = this.#known(ref.handle) ?? (current ? this.#current.get(n) : undefined);\n      if (known) return known;", "const known = undefined;\n      if (known) return known;"],
  [A, '    this.pdf.pin(this);\n    return this.#list("hyperlinks");', '    return this.#list("hyperlinks");'],
  [S, "if (matcher instanceof RegExp) {", "if (false) {"],
  [S, 'if (typeof matcher === "function") return', 'if (false) return'],
  // the structure matcher: the mask in its own argument, never taken from a caller's value
  [S, "  return [matcher];\n}", "  return matcher !== null && typeof matcher === \"object\" && \"mask\" in matcher ? [null, matcher.mask] : [matcher];\n}"],
  [S, "  return [matcher];\n}", "  return [null, matcher];\n}"],
  [S, "    return [\n      null,\n", "    return [\n      matcher,\n"],
  [S, "const args = [path, ...matcherArgs(this, matcher)];", "const args = [path, matcherArgs(this, matcher).at(-1)];"],
];

const run = (tests) => {
  try {
    execFileSync("node", ["node_modules/vitest/vitest.mjs", "run", "--config", "js/vitest.config.ts", ...tests], {
      cwd: root,
      stdio: "pipe",
      timeout: 30 * 60_000,
      env: { ...process.env },
    });
    return true;
  } catch {
    return false;
  }
};

let survivors = 0;
let skipped = 0;
for (const [i, [file, from, to, tests]] of mutants.entries()) {
  if (only.size && !only.has(String(i))) continue;
  const path = join(root, file);
  const original = readFileSync(path, "utf8");
  const n = original.split(from).length - 1;
  if (n !== 1) {
    skipped++;
    console.log(`#${i} SKIPPED ${file}: the text occurs ${n} times: ${from.slice(0, 60)}`);
    continue;
  }
  writeFileSync(path, original.replace(from, to));
  let passed;
  try {
    passed = run(tests ?? API) && (tests ? run(API) : true);
  } finally {
    writeFileSync(path, original);
  }
  if (passed) survivors++;
  console.log(`#${i} ${passed ? "SURVIVED" : "killed"} ${file}: ${from.slice(0, 90).replace(/\n/g, "\\n")}`);
}
console.log(`${mutants.length} mutants, ${survivors} survivors, ${skipped} skipped`);
