// The shared runtime tests (js/test/shared/tests.mjs) in Node, on the
// TypeScript sources and dist/pdfplumber.wasm + dist/cmaps/ (run
// `node scripts/build.mjs` first, or check.sh does).
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { describe, it } from "vitest";

import * as api from "../../src/index.js";
import * as internal from "../../src/internal.js";
import { setCodegen } from "../../src/core/wire.js";
import { PDFPLUMBER_ROOT, ROOT, oracleLines } from "../oracle-data.mjs";
import { tests } from "../shared/tests.mjs";

const lib = { ...api, ...internal };
const ctx = {
  lib,
  runtime: "node",
  oracle: async (name: string) => oracleLines("js_runtime", name),
  readFile: async (rel: string) => new Uint8Array(await readFile(join(PDFPLUMBER_ROOT, rel))),
  fileInput: (rel: string) => join(PDFPLUMBER_ROOT, rel),
  missingInput: () => join(ROOT, "does-not-exist.pdf"),
  readCMap: async (name: string) =>
    new Uint8Array(await readFile(join(ROOT, "dist/cmaps", `${encodeURIComponent(name)}.pdfcmap.zlib`))),
};

for (const codegen of [true, false]) {
  describe(codegen ? "runtime" : "runtime, rows built without code generation", () => {
    for (const t of tests) {
      if (t.todo) {
        it.todo(`${t.name} (${t.todo})`);
        continue;
      }
      it(t.name, async () => {
        setCodegen(codegen ? undefined : false);
        api.configure({});
        try {
          await t.run(ctx);
        } finally {
          setCodegen(undefined);
        }
      }, 120_000);
    }
  });
}
