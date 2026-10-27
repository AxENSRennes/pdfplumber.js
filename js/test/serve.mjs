#!/usr/bin/env node
// The server of the browser tests (js/playwright.config.ts): the
// repository (dist/, js/), the corpus and the upstream test files of
// PDFPLUMBER_ROOT under /corpus/ and /upstream/, the generated corpora of
// the cache under /.cache/corpus/, the unit data of
// oracle/unit/js_runtime.py under /oracle/ and the sessions of the public
// API (oracle/unit/js_api.py) under /oracle-api/.
//   node js/test/serve.mjs [PORT]
import { join } from "node:path";
import { CACHE, PDFPLUMBER_ROOT, ROOT, oracleDir } from "./oracle-data.mjs";
import { startServer } from "./static-server.mjs";

const port = Number(process.argv[2] ?? process.env.PORT ?? 4173);
const { url } = await startServer(ROOT, {
  port,
  mounts: {
    "/corpus": join(PDFPLUMBER_ROOT, "corpus"),
    "/upstream": join(PDFPLUMBER_ROOT, "upstream"),
    // the depth windows of the API sessions (depth-bytes.jsonl), files of
    // the adversarial sets and of ApiDepth, named from PDFPLUMBER_ROOT
    "/.cache/corpus": join(CACHE, "corpus"),
    "/oracle": oracleDir("js_runtime"),
    "/oracle-api": oracleDir("js_api"),
  },
});
console.log(`serving ${ROOT} on ${url}`);
