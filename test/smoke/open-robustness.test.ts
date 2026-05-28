import { createServer, type Server } from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { open, type PDFInput } from "../../src/index.js";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "../..");

function fixture(relativePath: string): string {
  return path.join(repoRoot, relativePath);
}

async function withNotFoundServer<T>(fn: (url: string) => Promise<T>): Promise<T> {
  const server = createServer((_request, response) => {
    response.statusCode = 404;
    response.end("not found");
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  try {
    const address = server.address();
    if (!address || typeof address === "string") throw new Error("Failed to start test server");
    return await fn(`http://127.0.0.1:${address.port}/missing.pdf`);
  } finally {
    await new Promise<void>((resolve, reject) => (server as Server).close((error) => error ? reject(error) : resolve()));
  }
}

async function captureOpen(input: PDFInput, options: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
  try {
    const document = await open(input, options);
    try {
      const firstPage = document.pages[0];
      return {
        status: "ok",
        pages: document.pages.length,
        chars: firstPage?.chars.length ?? 0,
        textHead: firstPage ? String(await firstPage.extractText()).slice(0, 80) : ""
      };
    } finally {
      await document.close();
    }
  } catch (error) {
    return {
      status: "error",
      name: error instanceof Error ? error.name : typeof error,
      message: error instanceof Error ? error.message : String(error)
    };
  }
}

function expectStableOpenOutcome(outcome: Record<string, unknown>): void {
  if (outcome.status === "ok") {
    expect(outcome.pages).toEqual(expect.any(Number));
    expect(Number(outcome.pages)).toBeGreaterThanOrEqual(0);
    expect(outcome.chars).toEqual(expect.any(Number));
    expect(outcome.textHead).toEqual(expect.any(String));
    return;
  }
  expect(outcome).toMatchObject({
    status: "error",
    name: expect.stringMatching(/^(InvalidPDFException|PasswordException|UnknownErrorException|FormatError|Error)$/),
    message: expect.any(String)
  });
}

describe("public open() robustness", () => {
  it("raises documented stable errors for invalid, missing, and empty inputs", async () => {
    await expect(captureOpen(fixture("pdfjs/test/pdfs/bug1020226.pdf"))).resolves.toMatchObject({
      status: "error",
      name: "InvalidPDFException",
      message: "Invalid PDF structure."
    });

    await expect(captureOpen(new Uint8Array(0))).resolves.toMatchObject({
      status: "error",
      name: "InvalidPDFException",
      message: "The PDF file is empty, i.e. its size is zero bytes."
    });

    await withNotFoundServer(async (url) => {
      await expect(captureOpen(url)).resolves.toMatchObject({
        status: "error",
        name: "Error",
        message: "Failed to fetch PDF: 404 Not Found"
      });
    });
  });

  it("keeps password protected PDFs on stable public outcomes", async () => {
    for (const relativePath of ["pdfjs/test/pdfs/pr6531_1.pdf", "pdfjs/test/pdfs/pr6531_2.pdf", "pdfjs/test/pdfs/issue3371.pdf"]) {
      await expect(captureOpen(fixture(relativePath))).resolves.toMatchObject({
        status: "error",
        name: "PasswordException",
        message: "No password given"
      });
    }

    await expect(captureOpen(fixture("pdfjs/test/pdfs/pr6531_2.pdf"), { password: "qwerty" })).resolves.toMatchObject({
      status: "error",
      name: "PasswordException",
      message: "Incorrect Password"
    });

    for (const relativePath of ["pdfjs/test/pdfs/pr6531_1.pdf", "pdfjs/test/pdfs/pr6531_2.pdf"]) {
      await expect(captureOpen(fixture(relativePath), { password: "asdfasdf" })).resolves.toMatchObject({
        status: "ok",
        pages: 1
      });
    }
  });

  it("opens or raises a stable error for malformed PDF.js parser fixtures", async () => {
    const cases = [
      "pdfjs/test/pdfs/PDFBOX-4352-0.pdf",
      "pdfjs/test/pdfs/GHOSTSCRIPT-698804-1-fuzzed.pdf",
      "pdfjs/test/pdfs/REDHAT-1531897-0.pdf",
      "pdfjs/test/pdfs/poppler-67295-0.pdf",
      "pdfjs/test/pdfs/poppler-85140-0.pdf",
      "pdfjs/test/pdfs/poppler-91414-0-53.pdf",
      "pdfjs/test/pdfs/poppler-91414-0-54.pdf",
      "pdfjs/test/pdfs/poppler-742-0-fuzzed.pdf",
      "pdfjs/test/pdfs/poppler-937-0-fuzzed.pdf",
      "pdfjs/test/pdfs/issue15150.pdf",
      "pdfjs/test/pdfs/issue15590.pdf"
    ];

    for (const relativePath of cases) {
      expectStableOpenOutcome(await captureOpen(fixture(relativePath)));
    }
  });

  it("opens selected PDF.js load manifest corpus PDFs with stable public outcomes", async () => {
    const cases = [
      "pdfjs/test/pdfs/issue4461.pdf",
      "pdfjs/test/pdfs/issue6069.pdf",
      "pdfjs/test/pdfs/issue1293r.pdf",
      "pdfjs/test/pdfs/issue17554.pdf",
      "pdfjs/test/pdfs/issue6108.pdf",
      "pdfjs/test/pdfs/issue7446.pdf",
      "pdfjs/test/pdfs/issue5599.pdf",
      "pdfjs/test/pdfs/issue18986.pdf",
      "pdfjs/test/pdfs/issue6151.pdf",
      "pdfjs/test/pdfs/bug1020858.pdf",
      "pdfjs/test/pdfs/issue7665.pdf",
      "pdfjs/test/pdfs/pdfjsbad1586.pdf",
      "pdfjs/test/pdfs/issue1985.pdf",
      "pdfjs/test/pdfs/openoffice.pdf",
      "pdfjs/test/pdfs/arial_unicode_ab_cidfont.pdf",
      "pdfjs/test/pdfs/arial_unicode_en_cidfont.pdf",
      "pdfjs/test/pdfs/bug868745.pdf",
      "pdfjs/test/pdfs/scan-bad.pdf",
      "pdfjs/test/pdfs/ArabicCIDTrueType.pdf",
      "pdfjs/test/pdfs/complex_ttf_font.pdf",
      "pdfjs/test/pdfs/issue1249.pdf",
      "pdfjs/test/pdfs/bug886717.pdf",
      "pdfjs/test/pdfs/canvas.pdf",
      "pdfjs/test/pdfs/bug900822.pdf",
      "pdfjs/test/pdfs/issue11922_reduced.pdf",
      "pdfjs/test/pdfs/issue7229.pdf",
      "pdfjs/test/pdfs/bug1978317.pdf",
      "test/fixtures/upstream-contract-pdfs/pdfjs-bug1260585.pdf",
      "test/fixtures/upstream-contract-pdfs/pdfjs-bug867484.pdf",
      "test/fixtures/upstream-contract-pdfs/pdfjs-bug1755201.pdf",
      "test/fixtures/upstream-contract-pdfs/pdfjs-issue14864.pdf",
      "test/fixtures/upstream-contract-pdfs/pdfjs-issue18503.pdf",
      "test/fixtures/upstream-contract-pdfs/pdfjs-issue19835.pdf",
      "test/fixtures/upstream-contract-pdfs/pdfjs-issue10004.pdf",
      "test/fixtures/upstream-contract-pdfs/pdfjs-issue10272.pdf",
      "test/fixtures/upstream-contract-pdfs/pdfjs-issue11518.pdf",
      "test/fixtures/upstream-contract-pdfs/pdfjs-issue14562.pdf",
      "test/fixtures/upstream-contract-pdfs/pdfjs-bug1766987.pdf",
      "test/fixtures/upstream-contract-pdfs/pdfjs-issue16081.pdf",
      "test/fixtures/upstream-contract-pdfs/pdfjs-issue16119.pdf",
      "test/fixtures/upstream-contract-pdfs/pdfjs-bug1823296.pdf",
      "test/fixtures/upstream-contract-pdfs/pdfjs-bug1847733.pdf",
      "test/fixtures/upstream-contract-pdfs/pdfjs-issue16863.pdf",
      "test/fixtures/upstream-contract-pdfs/pdfjs-issue17856.pdf"
    ];

    for (const relativePath of cases) {
      expectStableOpenOutcome(await captureOpen(fixture(relativePath)));
    }
  }, 30_000);

  it("opens the large linked PDF.js load manifest corpus PDF with a stable public outcome", async () => {
    expectStableOpenOutcome(await captureOpen(fixture("test/fixtures/upstream-contract-pdfs/pdfjs-issue19281.pdf")));
  }, 90_000);

  it("opens OSS-Fuzz corpus PDFs or raises the documented stable error", async () => {
    const cases = [
      "5452007745323008.pdf",
      "4833695495684096.pdf",
      "6515565732102144.pdf",
      "4646567755972608.pdf",
      "5177159198507008.pdf",
      "6400141380878336.pdf",
      "6085913544818688.pdf",
      "4591020179783680.pdf",
      "4691742750474240.pdf",
      "6013812888633344.pdf",
      "4736668896133120.pdf",
      "5809779695484928.pdf",
      "4652594248613888.pdf",
      "5592736912179200.pdf",
      "5317294594523136.pdf",
      "4927662560968704.pdf",
      "5903429863538688.pdf",
      "5914823472250880.pdf",
      "4715311080734720.pdf"
    ];

    for (const name of cases) {
      expectStableOpenOutcome(await captureOpen(fixture(`pdfplumber-python/tests/pdfs/from-oss-fuzz/load/${name}`)));
    }
  });
});
