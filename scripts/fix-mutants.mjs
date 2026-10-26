// Hand mutants of the code wave5/fix wrote (the review findings of wave 5),
// in JavaScript (js/src) and in Rust (crates: the core's operations, which
// only the sessions of the public API check through WebAssembly): each
// mutant replaces one exact text of a source file, runs the tests that check
// that code (the sessions of oracle/unit/js_api.py through the API,
// api-lifetime, api-runtime, the runtime tests, the depth windows, the
// core's unit tests), and is "killed" when they fail. A Rust mutant
// rebuilds dist/pdfplumber.wasm first (the JavaScript tests load it). The
// source is restored after each run (and the module rebuilt after a Rust
// mutant). Survivors are listed in docs/architecture.md ("Vague 5",
// "Corrections de la revue"), each with its justification.
//
//   node scripts/fix-mutants.mjs <checkout> [ids...]   (a copy of the checkout:
//   the sources are mutated in place; dist/ built, unit data generated)
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const root = process.argv[2];
const only = new Set(process.argv.slice(3));

const vitest = (...args) => ["vitest", ...args];
const API = vitest("js/test/node/api.test.ts", "js/test/node/api-lifetime.test.ts", "js/test/node/api-runtime.test.ts");
const SESSIONS = (pattern) => vitest("js/test/node/api.test.ts", "-t", pattern);
const LIFETIME = vitest("js/test/node/api-lifetime.test.ts");
const RUNTIME = vitest("js/test/node/api-runtime.test.ts");
const PAGES = vitest("js/test/node/runtime.test.ts", "-t", "page operations through the boundary|the streams of results");
const DEPTH = vitest("js/test/node/api-depth.test.ts", "-t", "structure");
const CMAPS = ["cargo", "test", "-p", "pdfplumber-wasm", "--lib", "cmaps"];

const A = "js/src/api.ts";
const R = "js/src/core/runtime.ts";
const W = "js/src/core/wire.ts";
const C = "js/src/worker/client.ts";
const S = "js/src/search.ts";
const T = "js/src/api/structure.ts";
const PG = "crates/wasm/src/pages.rs";
const TX = "crates/pdfplumber/src/utils/text.rs";
const OP = "crates/wasm/src/ops.rs";
const CM = "crates/wasm/src/cmaps.rs";
const SR = "crates/wasm/src/structs.rs";

/** [file, text, replacement, tests...] */
const mutants = [
  // null is None, undefined not given
  [A, "return v === undefined ? fallback : v;", "return v ?? fallback;", SESSIONS("calls|arguments")],
  // search() and filter() of what is not a pattern or a function
  [A, '    if (pattern === undefined) return Promise.reject(missingArgument("Page.search", "pattern"));\n', "", SESSIONS("calls")],
  [A, "refused = [pattern, given(own.regex, true)];", "refused = [pattern, true];", SESSIONS("calls")],
  [A, "} else if (!runnablePattern(pattern)) {", "} else if (false) {", SESSIONS("calls")],
  [A, '    if (test === undefined) return Promise.reject(missingArgument("Page.filter", "test_function"));\n', "", SESSIONS("calls")],
  [A, '    if (typeof test !== "function") return this.#derive("page_filter", [null, test]);\n', "", SESSIONS("calls")],
  [S, 'return typeof pattern === "string" || pattern instanceof RegExp;', "return pattern instanceof RegExp;", SESSIONS("calls")],
  [S, "`${qualname}() missing 1 required positional argument: '${name}'`", "`${qualname}() missing an argument`", SESSIONS("calls")],
  // the finalizers' turns of the event loop; a crashed worker stopped
  [A, "    if (tracked < YIELD_AFTER) return this.core.exclusive(f);\n", "", LIFETIME],
  [A, "    tracked++;\n", "", LIFETIME],
  [A, 'if (typeof g.setImmediate === "function") return g.setImmediate(resolve);', "", LIFETIME],
  [A, "    core?.terminate?.();\n", "", RUNTIME],
  // pages of a document's own results; references of another core
  [R, "    return origin === undefined || origin === this;", "    return true;", LIFETIME],
  [R, "    return origin === undefined || origin === this;", "    return origin === this;", PAGES],
  [R, "      origins.set(s, this);\n", "", LIFETIME],
  [R, "      origins.set(ref, this);\n", "", LIFETIME],
  [R, "      pageRefOwners.set(ref, owner);\n", "", LIFETIME],
  [W, "  const maker = refMakers?.page;\n  return maker ? maker(handle, pageNumber) : new PageRef(handle, pageNumber);", "  return new PageRef(handle, pageNumber);", LIFETIME],
  [W, '      if (accept && !accept(v)) throw new TypeError(OTHER_CORE("PageRef"));\n', "", LIFETIME],
  [W, '      if (accept && !accept(v)) throw new TypeError(OTHER_CORE("PDFStream"));\n', "", LIFETIME],
  // the streams kept by document
  [R, "    if (owner === undefined) return this.#unowned;\n    let m = this.#byOwner.get(owner);", "    let m = this.#byOwner.get(owner ?? (this as never));", PAGES],
  [R, "      if (live.size >= 1024 && !known) sweep(live);\n", "", LIFETIME],
  [R, "  if (live.size < 2 * sizes) return;\n", "", LIFETIME],
  // wasm32 addresses, unsigned
  [R, "const ptr = this.ex.pp_alloc(bytes.length) >>> 0;", "const ptr = this.ex.pp_alloc(bytes.length);", RUNTIME],
  [R, "this.ex.pp_result_ptr() >>> 0", "this.ex.pp_result_ptr()", RUNTIME],
  [R, "this.ex.pp_result_len() >>> 0", "this.ex.pp_result_len()", RUNTIME],
  // CMaps the loader cannot provide
  [R, '    if (failed.length) responseValue(this.callRaw("cmap_unavailable", 0, failed), "cmap_unavailable");\n', "", RUNTIME],
  [R, "const failed = names.filter((n) => !this.added.has(n));", "const failed = names.filter((n) => this.added.has(n));", RUNTIME],
  // arguments: Maps' undefined values, objects with no Python value
  [W, "else if (v instanceof Map) entries = [...v.entries()].filter(([, x]) => x !== undefined);", "else if (v instanceof Map) entries = [...v.entries()];", SESSIONS("arguments")],
  [W, "else if (isPlainObject(v)) entries = Object.entries(v).filter(([, x]) => x !== undefined);", "else if (true) entries = Object.entries(v as object).filter(([, x]) => x !== undefined);", SESSIONS("arguments")],
  [W, "  return proto === null || Object.getPrototypeOf(proto) === null;", "  return true;", SESSIONS("arguments")],
  // pages read in the order of the calls
  [A, "      if (this.#pages) return this.#pages;\n", "", LIFETIME],
  [A, "      this.#pages = pages;\n", "", LIFETIME],
  // the worker client
  [C, "        this.stop(r.error.message);\n        throw new CoreCrash(r.error.message);", "        this.dead = r.error.message;\n        throw new CoreCrash(r.error.message);", RUNTIME],
  [C, "this.stop(e.message || `the pdfplumber worker ${this.url} failed: it could not be loaded, or it stopped`);", "this.stop(e.message);", RUNTIME],
  [C, "      core.terminate();\n      throw new Error", "      throw new Error", RUNTIME],
  [C, "    const waiting = [...this.waiting.values()];", "    const waiting: ((r: WorkerReply) => void)[] = [];", RUNTIME],
  [C, "    this.worker.terminate();\n    const waiting", "    const waiting", RUNTIME],
  // the structure element's to_dict and iteration
  [T, 'return this.tree.run_("struct_element_to_dict", [this.path])', 'return this.tree.run_("struct_element_to_dict", [[0]])', SESSIONS("structure")],
  [T, "    return this.children[Symbol.iterator]();\n  }\n\n  /** `tree.find", "    return [].values();\n  }\n\n  /** `tree.find", LIFETIME],
  [T, "  [Symbol.iterator](): Iterator<PDFStructElement> {\n    return this.children[Symbol.iterator]();\n  }\n}", "  [Symbol.iterator](): Iterator<PDFStructElement> {\n    return this.tree.children[Symbol.iterator]();\n  }\n}", LIFETIME],
  // the core (Rust): search's refused pattern, filter of a value, to_dict,
  // the history's size, unavailable CMaps
  [PG, "                    if let Some(r) = refused {", "                    if let Some(r) = refused.filter(|_| false) {", SESSIONS("calls")],
  [TX, "        let regex_false = matches!(regex, PyValue::Bool(false));", "        let regex_false = matches!(regex, PyValue::Bool(true));", SESSIONS("calls")],
  [TX, "        check_hashable(pattern)?;\n", "", SESSIONS("calls")],
  [TX, '            PyValue::Bytes(_) => exc::TypeError("cannot use a bytes pattern on a string-like object"),\n', "", SESSIONS("calls")],
  [TX, "            pycompat::recursion::leaf(1)?;\n", "", SESSIONS("calls")],
  [TX, "        pycompat::recursion::leaf(2)?;\n", "", SESSIONS("calls")],
  [PG, "Some(PyValue::None) => kept.extend(objs.iter().map(|o| (o.clone(), !o.is_empty()))),", "Some(PyValue::None) => kept.extend(objs.iter().map(|o| (o.clone(), o.is_empty()))),", SESSIONS("calls")],
  [PG, "                Some(v) if !objs.is_empty() => {", "                Some(v) => {", SESSIONS("calls")],
  [PG, "    let mask = match value {\n        None => Some(kwargs(mask)?),\n        Some(_) => None,\n    };", "    let mask = Some(kwargs(mask)?);", SESSIONS("calls")],
  [SR, "                let file = tree.doc.file_kind()?;", "                let file = { drop(tree.doc.file_kind()?); pdfminer::pdfparser::FileKind::BytesIO };", DEPTH],
  [SR, '            "struct_element_to_dict" => {', '            "struct_element_to_dict_" => {', SESSIONS("structure")],
  [OP, "        if self.history.len() > self.history_limit || self.history_bytes() > self.history_bytes_limit {", "        if self.history.len() > self.history_limit {", RUNTIME],
  [OP, "                bytes: self.request_bytes,", "                bytes: 0,", RUNTIME],
  [OP, "                if let Some(bytes) = a.get(1) {", "                if let Some(bytes) = a.get(9) {", RUNTIME],
  [OP, "        self.request_bytes = request.len();\n", "", RUNTIME],
  [CM, "        s.configured && s.known.iter().all(|n| s.loaded.contains_key(n) || s.unavailable.contains(n))", "        s.configured && s.known.iter().all(|n| s.loaded.contains_key(n))", RUNTIME, CMAPS],
  [CM, "        s.unavailable.remove(name);\n", "", RUNTIME, CMAPS],
  [CM, ".filter(|n| s.known.contains(n) && !s.loaded.contains_key(n))", ".filter(|_| true)", RUNTIME, CMAPS],
  [OP, "                cmaps::mark_unavailable(names);\n", "                drop(names);\n", RUNTIME],
];

const env = { ...process.env, PATH: `${join(homedir(), ".cargo/bin")}:${process.env.PATH}`, CARGO_BUILD_JOBS: process.env.CARGO_BUILD_JOBS ?? "2" };

/** Runs one test command; `true` when it passes. */
function run(spec) {
  const [kind, ...args] = spec;
  const cmd = kind === "vitest" ? ["node", ["node_modules/vitest/vitest.mjs", "run", "--config", "js/vitest.config.ts", ...args]] : [kind, args];
  try {
    execFileSync(cmd[0], cmd[1], { cwd: root, stdio: "pipe", timeout: 60 * 60_000, env });
    return true;
  } catch {
    return false;
  }
}

/** Rebuilds dist/pdfplumber.wasm; `false` when the mutant does not build. */
function buildWasm() {
  try {
    execFileSync("node", ["scripts/build-wasm.mjs", "--no-opt"], { cwd: root, stdio: "pipe", timeout: 30 * 60_000, env });
    return true;
  } catch {
    return false;
  }
}

let survivors = 0;
let skipped = 0;
for (const [i, [file, from, to, ...tests]] of mutants.entries()) {
  if (only.size && !only.has(String(i))) continue;
  const path = join(root, file);
  const original = readFileSync(path, "utf8");
  const n = original.split(from).length - 1;
  if (n !== 1) {
    skipped++;
    console.log(`#${i} SKIPPED ${file}: the text occurs ${n} times: ${from.slice(0, 60)}`);
    continue;
  }
  const rust = file.endsWith(".rs");
  writeFileSync(path, original.replace(from, to));
  let outcome;
  try {
    if (rust && !buildWasm()) outcome = "unviable";
    else outcome = tests.every((t) => run(t)) ? "SURVIVED" : "killed";
  } finally {
    writeFileSync(path, original);
    if (rust) buildWasm();
  }
  if (outcome === "SURVIVED") survivors++;
  console.log(`#${i} ${outcome} ${file}: ${from.slice(0, 90).replace(/\n/g, "\\n")}`);
}
console.log(`${mutants.length} mutants, ${survivors} survivors, ${skipped} skipped`);
