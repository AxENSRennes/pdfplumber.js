// Hand mutants of the JavaScript boundary layer of wave 5 (BOUNDARY): each
// mutant replaces one exact text of a source file, runs the tests that
// cover it (vitest, Node, sources), and is "killed" when they fail. The
// source is restored after each run.
//
//   node scripts/js-boundary-mutants.mjs <checkout> [ids...]   (dist/ built: node scripts/build.mjs)
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const root = process.argv[2];
const only = new Set(process.argv.slice(3));

const STREAMS = "the streams of results are references";
const LAZY = "a CJK document extracts like pdfplumber, its CMaps fetched once";
const HISTORY = "a long history makes the runtime hand over every CMap";
const PAGES = "page operations through the boundary match pdfplumber";
const DIAG = "pdfplumber's and pdfminer's records and warnings reach onWarning";

const mutants = [
  // pdfobjects.ts
  ["js/src/pdfobjects.ts", "if (!this.#ref) return this.#rawdata as unknown as Promise<Uint8Array | null>;", "if (!this.#ref) return null as unknown as Promise<Uint8Array | null>;", STREAMS],
  ["js/src/pdfobjects.ts", 'return this.#read("stream_rawdata");', 'return this.#read("stream_data");', STREAMS],
  ["js/src/pdfobjects.ts", 'return this.#read("stream_data");', 'return this.#read("stream_rawdata");', STREAMS],
  ["js/src/pdfobjects.ts", 'return this.#read("stream_get_data") as Promise<Uint8Array>;', 'return this.#read("stream_data") as Promise<Uint8Array>;', STREAMS],
  ["js/src/pdfobjects.ts", 'return this.#read("stream_get_rawdata");', 'return this.#read("stream_data");', STREAMS],
  ["js/src/pdfobjects.ts", "if (this.#data !== null) return Promise.resolve(this.#data);", "if (true) return Promise.resolve(this.#data);", STREAMS],
  ["js/src/pdfobjects.ts", "if (!this.#ref) return Promise.resolve(this.#rawdata);", "if (!this.#ref) return Promise.resolve(null);", STREAMS],
  ["js/src/pdfobjects.ts", "    s.#ref = { handle, source };\n", "", STREAMS],
  ["js/src/pdfobjects.ts", "if (!ref.source) return Promise.reject(", "if (false) return Promise.reject(", STREAMS],
  ["js/src/pdfobjects.ts", "return [this.#rawdata, this.#data];", "return [this.#data, this.#rawdata];", STREAMS],
  // runtime.ts
  ["js/src/core/runtime.ts", "    if (owner === undefined) return this.#unowned;\n", "", STREAMS],
  ["js/src/core/runtime.ts", "        if (s) return s;\n", "", STREAMS],
  ["js/src/core/runtime.ts", "      const source: StreamSource = owner\n", "      const source: StreamSource = false\n", STREAMS],
  ["js/src/core/runtime.ts", 'responseValue(this.callRaw("set_stream_refs", 0, [true]), "set_stream_refs");', 'responseValue(this.callRaw("set_stream_refs", 0, [false]), "set_stream_refs");', STREAMS],
  ["js/src/core/runtime.ts", "  return (u8[6] & 2) !== 0;", "  return false;", HISTORY],
  ["js/src/core/runtime.ts", "  return (u8[6] & 2) !== 0;", "  return (u8[6] & 1) !== 0;", HISTORY],
  ["js/src/core/runtime.ts", "      if (r.historyLong) await this.loadAll();\n", "", HISTORY],
  ["js/src/core/runtime.ts", "this.cmaps.names.filter((n) => !this.added.has(n))", "this.cmaps.names.filter((n) => this.added.has(n))", HISTORY],
  ["js/src/core/runtime.ts", "        this.voidCalls++;\n        await this.loadMissing(r.value as string[]);\n        continue;", "        await this.loadMissing(r.value as string[]);\n        continue;", LAZY],
  // wire.ts
  ["js/src/core/wire.ts", "return maker ? maker(handle, objid, genno, attrs) : PDFStream.reference(objid, genno, attrs, handle, null);", "return PDFStream.reference(objid, genno, attrs, handle, null);", STREAMS],
  ["js/src/core/wire.ts", "    if (v instanceof PSLiteral) {\n      w.u8(TAG.LITERAL);", "    if (false) {\n      w.u8(TAG.LITERAL);", PAGES],
  ["js/src/core/wire.ts", "    if (v instanceof PSKeyword) {\n      w.u8(TAG.KEYWORD);", "    if (false) {\n      w.u8(TAG.KEYWORD);", PAGES],
  ["js/src/core/wire.ts", "    if (v instanceof PDFObjRef) {\n      w.u8(TAG.REF);", "    if (false) {\n      w.u8(TAG.REF);", PAGES],
  ["js/src/core/wire.ts", "    if (v instanceof PageRef) {\n      if (accept", "    if (false) {\n      if (accept", PAGES],
  ["js/src/core/wire.ts", "      stack.push(v.page_number, v.handle);", "      stack.push(v.handle, v.page_number);", PAGES],
  ["js/src/core/wire.ts", "      stack.push(v.attrs, v.genno, v.objid, v.handle);", "      stack.push(v.handle, v.genno, v.objid, v.attrs);", PAGES],
  ["js/src/core/wire.ts", "    if (v instanceof PDFStream) {\n      if (v.handle === undefined)", "    if (false) {\n      if (v.handle === undefined)", PAGES],
  ["js/src/core/wire.ts", "pending.push([holder, key, (x: Record<string, unknown>) => streamRef([x.handle, x.objid, x.genno, x.attrs])]);", "pending.push([holder, key, (x: Record<string, unknown>) => streamRef([x.objid, x.handle, x.genno, x.attrs])]);", STREAMS],
  // api.ts (owner)
  ["js/src/api.ts", "return { ...diagnosticsPolicy(this.#own, page), owner: this };", "return { ...diagnosticsPolicy(this.#own, page) };", STREAMS],
];

const run = (pattern) => {
  try {
    execFileSync(
      "node",
      ["node_modules/vitest/vitest.mjs", "run", "--config", "js/vitest.config.ts", "js/test/node/runtime.test.ts", "-t", pattern],
      { cwd: root, stdio: "pipe", timeout: 30 * 60_000, env: { ...process.env } },
    );
    return true;
  } catch {
    return false;
  }
};

let survivors = 0;
for (const [i, [file, from, to, test]] of mutants.entries()) {
  if (only.size && !only.has(String(i))) continue;
  const path = join(root, file);
  const original = readFileSync(path, "utf8");
  const n = original.split(from).length - 1;
  if (n !== 1) {
    console.log(`#${i} ${file}: the text occurs ${n} times`);
    continue;
  }
  writeFileSync(path, original.replace(from, to));
  let passed;
  try {
    passed = run(test);
  } finally {
    writeFileSync(path, original);
  }
  if (passed) survivors++;
  console.log(`#${i} ${passed ? "SURVIVED" : "killed"} ${file}: ${from.slice(0, 70).replace(/\n/g, "\\n")}`);
}
console.log(`${mutants.length} mutants, ${survivors} survivors`);
