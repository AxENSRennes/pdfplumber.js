import { createServer, type Server } from "node:http";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { describe, expect, it } from "vitest";

import { open, type PDFInput } from "../../src/index.js";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "../..");
const fixturePath = path.join(repoRoot, "pdfplumber-python/tests/pdfs/scotus-transcript-p1.pdf");

interface Summary {
  pageCount: number;
  bbox: readonly number[];
  chars: number;
  words: string[];
  search: string[];
  textHead: string;
}

async function summarize(input: PDFInput): Promise<Summary> {
  const document = await open(input);
  try {
    const page = document.pages[0];
    return {
      pageCount: document.pages.length,
      bbox: page.bbox,
      chars: page.chars.length,
      words: (await page.extractWords()).slice(0, 8).map((word) => String(word.text)),
      search: (await page.search("Final")).map((match) => match.text),
      textHead: String(await page.extractText()).slice(0, 120)
    };
  } finally {
    await document.close();
  }
}

async function withPdfServer<T>(fn: (url: string) => Promise<T>): Promise<T> {
  const bytes = await readFile(fixturePath);
  const server = createServer((_request, response) => {
    response.setHeader("content-type", "application/pdf");
    response.end(bytes);
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  try {
    const address = server.address();
    if (!address || typeof address === "string") throw new Error("Failed to start test server");
    return await fn(`http://127.0.0.1:${address.port}/fixture.pdf`);
  } finally {
    await new Promise<void>((resolve, reject) => (server as Server).close((error) => error ? reject(error) : resolve()));
  }
}

function exactArrayBuffer(bytes: Uint8Array): ArrayBuffer {
  const copy = new ArrayBuffer(bytes.byteLength);
  new Uint8Array(copy).set(bytes);
  return copy;
}

describe("public open() input adapters", () => {
  it("extracts the same Node public data from path, file URL, bytes, Blob, and HTTP URL inputs", async () => {
    const bytes = new Uint8Array(await readFile(fixturePath));
    const arrayBuffer = exactArrayBuffer(bytes);
    const expected = await summarize(fixturePath);

    const cases: Array<[string, PDFInput]> = [
      ["file URL object", pathToFileURL(fixturePath)],
      ["file URL string", pathToFileURL(fixturePath).href],
      ["ArrayBuffer", arrayBuffer.slice(0)],
      ["Uint8Array", new Uint8Array(bytes)],
      ["Blob", new Blob([arrayBuffer], { type: "application/pdf" })]
    ];

    for (const [name, input] of cases) {
      expect(await summarize(input), name).toEqual(expected);
    }

    await withPdfServer(async (url) => {
      expect(await summarize(url), "HTTP URL string").toEqual(expected);
      expect(await summarize(new URL(url)), "HTTP URL object").toEqual(expected);
    });
  });
});
