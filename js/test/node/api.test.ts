// The sessions of the public API (oracle/unit/js_api.py) in Node, on the
// TypeScript sources and dist/pdfplumber.wasm + dist/cmaps/: every call
// through the public API, compared with Python's results for the
// statements the API runs (js/test/shared/api-sessions.mjs).
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

describe("public API sessions (oracle/unit/js_api.py)", () => {
  const lines = oracleLines("js_api", "sessions") as { in: [{ name: string; node_only: boolean }]; out: unknown[] }[];
  for (const line of lines) {
    it(line.in[0].name, async () => {
      api.configure({});
      await replayApiSession(lib, line, env);
    }, 300_000);
  }
});
