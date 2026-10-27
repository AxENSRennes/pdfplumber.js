// The depth windows of the adversarial sets and the API's own (oracle/unit/js_api.py,
// depth.jsonl and depth-bytes.jsonl) through the public API, in Node, on the TypeScript sources:
// where Python's RecursionError happens when each member's statement runs
// at the top level of a script (docs/js-runtime.md, "API Operations"). The
// stage traces through the API cannot show it (their oracle calls
// pdfplumber from nested generators; docs/verification.md,
// "Parity Through The JavaScript API").
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { describe, expect, it } from "vitest";

import * as api from "../../src/index.js";
import * as internal from "../../src/internal.js";
import { PDFPLUMBER_ROOT, oracleLines } from "../oracle-data.mjs";
import { replayApiSession } from "../shared/api-sessions.mjs";

const lib = { ...api, ...internal };
const env = {
  readFile: async (rel: string) => new Uint8Array(await readFile(join(PDFPLUMBER_ROOT, rel))),
  fileInput: (rel: string) => join(PDFPLUMBER_ROOT, rel),
  fileUrl: (rel: string) => pathToFileURL(join(PDFPLUMBER_ROOT, rel)),
};

// depth.jsonl: the windows opened by path, as the stages open them;
// depth-bytes.jsonl: from their bytes, as every runtime can (the browsers
// replay them too: js/test/browser/api.spec.ts)
describe.each(["depth", "depth-bytes"])("public API sessions at the depth windows (oracle/unit/js_api.py, %s.jsonl)", (name) => {
  const lines = oracleLines("js_api", name) as { in: [{ name: string }]; out: unknown[] }[];
  for (const line of lines) {
    it(line.in[0].name, async () => {
      api.configure({});
      const r = await replayApiSession(lib, line, env);
      // every expectation compared, and the calls Python made made
      expect(r.checked).toBe(line.out.length);
      expect(r.made).toBeGreaterThan(0);
    }, 300_000);
  }
});
