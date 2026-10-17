#!/usr/bin/env node
// The server of the browser tests (js/playwright.config.ts): the
// repository (dist/, js/), the corpus of PDFPLUMBER_ROOT under /corpus/, and
// the unit data of oracle/unit/js_runtime.py under /oracle/.
//   node js/test/serve.mjs [PORT]
import { join } from "node:path";
import { PDFPLUMBER_ROOT, ROOT, oracleDir } from "./oracle-data.mjs";
import { startServer } from "./static-server.mjs";

const port = Number(process.argv[2] ?? process.env.PORT ?? 4173);
const { url } = await startServer(ROOT, {
  port,
  mounts: { "/corpus": join(PDFPLUMBER_ROOT, "corpus"), "/oracle": oracleDir("js_runtime") },
});
console.log(`serving ${ROOT} on ${url}`);
