import { createServer, type Server } from "node:http";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { expect, test } from "@playwright/test";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "../..");
const fixturePath = path.join(repoRoot, "pdfplumber-python/tests/pdfs/scotus-transcript-p1.pdf");

interface Summary {
  metadata: Record<string, unknown>;
  pageCount: number;
  page: {
    page_number: number;
    bbox: readonly number[];
    cropbox: readonly number[];
    chars: number;
    words: Array<Record<string, unknown>>;
    search: string[];
    textHead: string;
    objectCounts: Record<string, number>;
  };
}

async function summarizeDocument(document: any): Promise<Summary> {
  try {
    const page = document.pages[0];
    return {
      metadata: Object.fromEntries(Object.entries(document.metadata).filter(([, value]) => value != null)),
      pageCount: document.pages.length,
      page: {
        page_number: page.page_number,
        bbox: page.bbox,
        cropbox: page.cropbox,
        chars: page.chars.length,
        words: page.extractWords().slice(0, 8).map((word: Record<string, unknown>) => ({
          text: word.text,
          x0: word.x0,
          top: word.top,
          x1: word.x1,
          bottom: word.bottom
        })),
        search: page.search("Final").map((match: Record<string, unknown>) => match.text),
        textHead: page.extractText().slice(0, 240),
        objectCounts: Object.fromEntries(Object.entries(page.objects).map(([key, value]) => [key, (value as unknown[]).length]))
      }
    };
  } finally {
    await document.close();
  }
}

async function nodeExpected(): Promise<Summary> {
  const { open } = await import(pathToFileURL(path.join(repoRoot, "dist/src/index.js")).href);
  return summarizeDocument(await open(fixturePath));
}

async function startServer(): Promise<{ server: Server; baseUrl: string }> {
  const server = createServer(async (request, response) => {
    try {
      const url = new URL(request.url ?? "/", "http://127.0.0.1");
      if (url.pathname === "/dist/browser/index.js") {
        response.setHeader("content-type", "text/javascript");
        response.end(await readFile(path.join(repoRoot, "dist/browser/index.js")));
        return;
      }
      if (url.pathname === "/dist/browser/pdf.worker.js") {
        response.setHeader("content-type", "text/javascript");
        response.end(await readFile(path.join(repoRoot, "dist/browser/pdf.worker.js")));
        return;
      }
      if (url.pathname === "/fixture.pdf") {
        response.setHeader("content-type", "application/pdf");
        response.end(await readFile(fixturePath));
        return;
      }
      response.statusCode = 404;
      response.end("not found");
    } catch (error) {
      response.statusCode = 500;
      response.end(String(error));
    }
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("Failed to start browser test server");
  return { server, baseUrl: `http://127.0.0.1:${address.port}` };
}

test.describe("browser ESM package", () => {
  let server: Server;
  let baseUrl: string;
  let expected: Summary;

  test.beforeAll(async () => {
    expected = await nodeExpected();
    const started = await startServer();
    server = started.server;
    baseUrl = started.baseUrl;
  });

  test.afterAll(async () => {
    if (server) await new Promise<void>((resolve, reject) => server.close((error) => (error ? reject(error) : resolve())));
  });

  test("matches Node public extraction behavior for ArrayBuffer, Blob, and URL inputs", async ({ page }) => {
    await page.goto(`${baseUrl}/dist/browser/index.js`);
    const actual = await page.evaluate(async ({ moduleUrl, pdfUrl }) => {
      const { open } = await import(moduleUrl);

      async function summarize(input: ArrayBuffer | Blob | string) {
        const document = await open(input);
        try {
          const page = document.pages[0];
          return {
            metadata: Object.fromEntries(Object.entries(document.metadata).filter(([, value]) => value != null)),
            pageCount: document.pages.length,
            page: {
              page_number: page.page_number,
              bbox: page.bbox,
              cropbox: page.cropbox,
              chars: page.chars.length,
              words: page.extractWords().slice(0, 8).map((word: Record<string, unknown>) => ({
                text: word.text,
                x0: word.x0,
                top: word.top,
                x1: word.x1,
                bottom: word.bottom
              })),
              search: page.search("Final").map((match: Record<string, unknown>) => match.text),
              textHead: page.extractText().slice(0, 240),
              objectCounts: Object.fromEntries(Object.entries(page.objects).map(([key, value]) => [key, (value as unknown[]).length]))
            }
          };
        } finally {
          await document.close();
        }
      }

      const bytes = await (await fetch(pdfUrl)).arrayBuffer();
      return {
        arrayBuffer: await summarize(bytes.slice(0)),
        blob: await summarize(new Blob([bytes], { type: "application/pdf" })),
        url: await summarize(pdfUrl)
      };
    }, { moduleUrl: `${baseUrl}/dist/browser/index.js`, pdfUrl: `${baseUrl}/fixture.pdf` });

    expect(actual.arrayBuffer).toEqual(expected);
    expect(actual.blob).toEqual(expected);
    expect(actual.url).toEqual(expected);
  });
});
