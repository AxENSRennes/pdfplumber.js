// The depth windows of the adversarial sets (oracle/unit/js_api.py,
// depth.jsonl) through the public API, in Node, on the TypeScript sources:
// where Python's RecursionError happens when each member's statement runs
// at the top level of a script (docs/public-api.md, "Operations"). The
// stage traces through the API cannot show it (their oracle calls
// pdfplumber from nested generators; docs/verification.md).
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { describe, it } from "vitest";

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

describe("public API sessions at the depth windows (oracle/unit/js_api.py, depth.jsonl)", () => {
  const lines = oracleLines("js_api", "depth") as { in: [{ name: string }]; out: unknown[] }[];
  for (const line of lines) {
    it(line.in[0].name, async () => {
      api.configure({});
      await replayApiSession(lib, line, env);
    }, 300_000);
  }
});
