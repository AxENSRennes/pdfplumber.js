// The sessions of the public API (oracle/unit/js_api.py) on the dist build
// of the package (dist/src: what npm ships for Node), as a user imports it.
// Needs `node scripts/build.mjs` (check.sh builds first).
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { describe, it } from "vitest";

import { PDFPLUMBER_ROOT, ROOT, oracleLines } from "../oracle-data.mjs";
import { replayApiSession } from "../shared/api-sessions.mjs";

const env = {
  readFile: async (rel: string) => new Uint8Array(await readFile(join(PDFPLUMBER_ROOT, rel))),
  fileInput: (rel: string) => join(PDFPLUMBER_ROOT, rel),
  fileUrl: (rel: string) => pathToFileURL(join(PDFPLUMBER_ROOT, rel)),
};

describe("public API sessions on the dist build", async () => {
  const api = await import(pathToFileURL(join(ROOT, "dist/src/index.js")).href);
  const internal = await import(pathToFileURL(join(ROOT, "dist/src/internal.js")).href);
  const lib = { ...api, ...internal };
  const lines = oracleLines("js_api", "sessions") as { in: [{ name: string }]; out: unknown[] }[];
  for (const line of lines) {
    it(line.in[0].name, async () => {
      api.configure({});
      await replayApiSession(lib, line, env);
    }, 300_000);
  }
});
