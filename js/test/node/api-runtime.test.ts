// What the public API layer (js/src/api.ts) does around the core that no
// Python value shows: where diagnostics go besides onWarning (logLevel:
// console.warn and console.debug, the count of dropped records), a new
// core after a crash, the compiled module reused by a new core. The
// diagnostics themselves (their text, as Python shows it) are checked
// against Python by the runtime test "pdfplumber's and pdfminer's records
// and warnings reach onWarning" (js/test/shared/tests.mjs).
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";

import * as api from "../../src/index.js";
import * as internal from "../../src/internal.js";
import { platform } from "../../src/platform/platform.js";
import { PDFPLUMBER_ROOT, oracleLines } from "../oracle-data.mjs";
import { fromTyped } from "../shared/typed.mjs";

const lib = { ...api, ...internal };

const ENTRIES = [
  { source: "pdfminer.psparser", level: 10, message: "a debug record" },
  { source: "pdfminer.pdfinterp", level: 30, message: "a warning" },
  { source: "pdfplumber.pdf", level: 40, message: "an error" },
];

afterEach(() => {
  vi.restoreAllMocks();
});

describe("public API diagnostics policy", () => {
  it("logLevel 'warning' writes warnings and above to console.warn, nothing to console.debug", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const debug = vi.spyOn(console, "debug").mockImplementation(() => {});
    const policy = internal.diagnosticsPolicy({ logLevel: "warning" });
    expect(policy.minLevel).toBe(30);
    policy.onDiagnostics!(ENTRIES.slice(1), 0);
    expect(warn.mock.calls).toEqual([["pdfplumber-js: pdfminer.pdfinterp: a warning"], ["pdfplumber-js: pdfplumber.pdf: an error"]]);
    expect(debug).not.toHaveBeenCalled();
  });

  it("logLevel 'debug' writes every record: below warning to console.debug, the others to console.warn", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const debug = vi.spyOn(console, "debug").mockImplementation(() => {});
    const seen: unknown[] = [];
    const policy = internal.diagnosticsPolicy({ logLevel: "debug", onWarning: (w) => seen.push(w) }, 2);
    expect(policy.minLevel).toBe(10);
    policy.onDiagnostics!(ENTRIES, 0);
    expect(debug.mock.calls).toEqual([["pdfplumber-js: pdfminer.psparser: a debug record"]]);
    expect(warn.mock.calls.length).toBe(2);
    expect(seen).toEqual([
      { source: "pdfminer.psparser", level: "debug", message: "a debug record", page: 2 },
      { source: "pdfminer.pdfinterp", level: "warning", message: "a warning", page: 2 },
      { source: "pdfplumber.pdf", level: "error", message: "an error", page: 2 },
    ]);
  });

  it("onWarning alone writes nothing to the console; dropped records are counted in one more warning", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const debug = vi.spyOn(console, "debug").mockImplementation(() => {});
    const seen: unknown[] = [];
    const policy = internal.diagnosticsPolicy({ onWarning: (w) => seen.push(w) });
    policy.onDiagnostics!(ENTRIES.slice(1, 2), 7);
    expect(seen).toEqual([
      { source: "pdfminer.pdfinterp", level: "warning", message: "a warning" },
      { source: "pdfplumber-js", level: "warning", message: "7 more diagnostics dropped" },
    ]);
    expect(warn).not.toHaveBeenCalled();
    expect(debug).not.toHaveBeenCalled();
  });

  it("open(..., { logLevel: 'warning' }) writes Python's warnings of a document to console.warn", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    let checked = 0;
    for (const { in: input, out } of oracleLines("js_runtime", "diagnostics") as { in: unknown[]; out: unknown }[]) {
      const [, pdfT, optionsT, opsT] = input;
      const expected = (fromTyped(out, lib) as [string, number, string][]).filter(([, level]) => level >= 30);
      const ops = fromTyped(opsT, lib) as string[];
      if (!expected.length) continue;
      // a new core: a new Python process (the registry of warnings.warn)
      lib.dropCore();
      warn.mockClear();
      const pdf = await api.open(fromTyped(pdfT, lib) as Uint8Array, { ...(fromTyped(optionsT, lib, true) as object), logLevel: "warning" });
      const settle = (x: Promise<unknown>) => x.catch(() => undefined);
      for (const op of ops) {
        if (op === "page_annots" || op === "page_objects") {
          const pages = (await settle(pdf.pages)) as api.PDFPlumberPage[] | undefined;
          for (const page of pages ?? []) await settle(op === "page_annots" ? page.annots : page.objects);
        } else if (op === "pages") await settle(pdf.pages);
        else if (op === "doc_annots") await settle(pdf.annots);
        else if (op === "doc_structure_tree") await settle(pdf.structure_tree);
        else throw new Error(`unknown operation ${op}`);
      }
      await pdf.release();
      expect(warn.mock.calls).toEqual(expected.map(([source, , message]) => [`pdfplumber-js: ${source}: ${message}`]));
      checked++;
    }
    lib.dropCore();
    expect(checked).toBeGreaterThan(0);
  });
});

describe("public API cores", () => {
  it("a new core after a crash; the documents of the crashed core release without a call", async () => {
    api.configure({});
    const core = await lib.getCore();
    expect(await lib.getCore()).toBe(core);
    const pdf = await api.open(new Uint8Array(await readFile(join(PDFPLUMBER_ROOT, "corpus/micro/text-operators.pdf"))));
    Object.defineProperty(core, "crashed", { get: () => true, configurable: true });
    const next = await lib.getCore();
    expect(next).not.toBe(core);
    expect(next.crashed).toBe(false);
    const call = vi.spyOn(core, "call");
    await pdf.release();
    expect(call).not.toHaveBeenCalled();
  });

  it("a new core instantiates the module compiled before (the default source)", async () => {
    api.configure({});
    await lib.getCore();
    const p = platform();
    const compile = vi.spyOn(p, "compileWasm");
    lib.dropCore();
    await lib.getCore();
    lib.dropCore();
    await lib.getCore();
    expect(compile).not.toHaveBeenCalled();
    compile.mockRestore();
    api.configure({});
  });
});
