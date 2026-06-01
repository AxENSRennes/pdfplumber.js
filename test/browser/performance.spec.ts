import { createServer, type Server } from "node:http";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { expect, test } from "@playwright/test";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "../..");
const baseline = JSON.parse(fs.readFileSync(path.join(repoRoot, "test/fixtures/performance/baseline-browser.json"), "utf8")) as {
  version: 1;
  thresholds: { timeRegressionRatio: number };
  browsers: Record<string, Record<string, { totalMs: number; pageCount: number; textLength: number; wordCount: number }>>;
};

test.skip(!process.env.PDFPLUMBER_JS_RUN_BROWSER_PERF, "browser performance benchmarks run through npm run perf:browser");

function contentType(filePath: string): string {
  if (filePath.endsWith(".js")) return "text/javascript";
  if (filePath.endsWith(".map")) return "application/json";
  if (filePath.endsWith(".pdf")) return "application/pdf";
  return "application/octet-stream";
}

async function startServer(): Promise<{ server: Server; baseUrl: string }> {
  const server = createServer((request, response) => {
    const pathname = decodeURIComponent(new URL(request.url ?? "/", "http://127.0.0.1").pathname);
    const filePath = path.join(repoRoot, pathname);
    if (!filePath.startsWith(repoRoot) || !fs.existsSync(filePath) || !fs.statSync(filePath).isFile()) {
      response.statusCode = 404;
      response.end("not found");
      return;
    }
    response.setHeader("content-type", contentType(filePath));
    fs.createReadStream(filePath).pipe(response);
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("Failed to start benchmark server");
  return { server, baseUrl: `http://127.0.0.1:${address.port}` };
}

test.describe("browser performance baseline", () => {
  let server: Server;
  let baseUrl: string;

  test.beforeAll(async () => {
    const started = await startServer();
    server = started.server;
    baseUrl = started.baseUrl;
  });

  test.afterAll(async () => {
    if (server) await new Promise<void>((resolve, reject) => server.close((error) => (error ? reject(error) : resolve())));
  });

  test("keeps browser extraction within the relative baseline", async ({ page, browserName }) => {
    test.setTimeout(90_000);
    await page.goto(`${baseUrl}/dist/browser/index.js`);
    const actual = await page.evaluate(async ({ moduleUrl, baseUrl }) => {
      const { open } = await import(moduleUrl);
      async function measure(name: string, pdfPath: string, pages: number[], inputKind: "arrayBuffer" | "blob" | "url") {
        const url = `${baseUrl}/${pdfPath}`;
        const response = await fetch(url);
        const bytes = await response.arrayBuffer();
        const input = inputKind === "arrayBuffer" ? bytes.slice(0) : inputKind === "blob" ? new Blob([bytes], { type: "application/pdf" }) : url;
        const start = performance.now();
        const document = await open(input, { pages, logLevel: "silent" });
        try {
          const firstPage = document.pages[0];
          const text = String(await firstPage.extractText());
          const words = await firstPage.extractWords();
          return { name, totalMs: Math.round((performance.now() - start) * 100) / 100, pageCount: document.pages.length, textLength: text.length, wordCount: words.length };
        } finally {
          await document.close();
        }
      }
      return [
        await measure("small-arraybuffer", "pdfplumber-python/tests/pdfs/scotus-transcript-p1.pdf", [1], "arrayBuffer"),
        await measure("large-url-page-1", "test/fixtures/upstream-contract-pdfs/pdfjs-pdf-reference-1-7.pdf", [1], "url"),
        await measure("large-blob-page-100", "test/fixtures/upstream-contract-pdfs/pdfjs-pdf-reference-1-7.pdf", [100], "blob")
      ];
    }, { moduleUrl: `${baseUrl}/dist/browser/index.js`, baseUrl });

    console.log(JSON.stringify({ browserName, actual }));
    const browserBaseline = baseline.browsers[browserName];
    expect(browserBaseline, `missing browser baseline for ${browserName}`).toBeTruthy();
    for (const result of actual) {
      const expected = browserBaseline[result.name];
      expect(expected, `missing browser baseline for ${browserName}.${result.name}`).toBeTruthy();
      expect(result.pageCount).toBe(expected.pageCount);
      expect(result.textLength).toBe(expected.textLength);
      expect(result.wordCount).toBe(expected.wordCount);
      expect(result.totalMs).toBeLessThanOrEqual(expected.totalMs * (1 + baseline.thresholds.timeRegressionRatio));
    }
  });
});
