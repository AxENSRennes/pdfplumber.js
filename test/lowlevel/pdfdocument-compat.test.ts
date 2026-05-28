import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import { open } from "../../src/index.js";
import { parseInfoMetadata } from "../../src/pdf.js";
import { parsePdfDocument } from "../../src/pdf/document.js";

interface PdfDocumentOracle {
  encryptedNoIdInfo: Array<Record<string, string>>;
  annotationPages: number;
  annotationPagesWithAnnots: number;
}

function pdfminerOracle(): PdfDocumentOracle {
  const code = `
import json
from pdfminer.pdfdocument import PDFDocument
from pdfminer.pdfpage import PDFPage
from pdfminer.pdfparser import PDFParser

with open("pdfminer-six/samples/encryption/encrypted_doc_no_id.pdf", "rb") as fp:
    encrypted_doc = PDFDocument(PDFParser(fp))
    encrypted_info = [{key: value.decode("latin1") if isinstance(value, bytes) else str(value) for key, value in item.items()} for item in encrypted_doc.info]

with open("pdfminer-six/samples/contrib/issue-1082-annotations.pdf", "rb") as fp:
    annotation_doc = PDFDocument(PDFParser(fp))
    annotation_pages = list(PDFPage.create_pages(annotation_doc))

print(json.dumps({
    "encryptedNoIdInfo": encrypted_info,
    "annotationPages": len(annotation_pages),
    "annotationPagesWithAnnots": sum(1 for page in annotation_pages if page.annots is not None),
}))
`;
  return JSON.parse(execFileSync("wsl_venv/bin/python", ["-c", code], { encoding: "utf8" })) as PdfDocumentOracle;
}

describe("low-level pdfminer PDFDocument compatibility", () => {
  it("raises PDFObjectNotFound for object zero like pdfminer", () => {
    const raw = readFileSync("pdfminer-six/samples/simple1.pdf", "latin1");
    const store = parsePdfDocument(raw);

    expect(() => store.getRequiredObject(0)).toThrowError(expect.objectContaining({ name: "PDFObjectNotFound" }));
  });

  it("decrypts metadata strings when an encrypted document has no trailer ID", async () => {
    const expected = pdfminerOracle().encryptedNoIdInfo[0];
    const raw = readFileSync("pdfminer-six/samples/encryption/encrypted_doc_no_id.pdf", "latin1");
    const store = parsePdfDocument(raw);

    expect(parseInfoMetadata(raw, store.rawObjects)).toMatchObject(expected);

    const pdf = await open("pdfminer-six/samples/encryption/encrypted_doc_no_id.pdf");
    try {
      expect(pdf.metadata).toMatchObject(expected);
    } finally {
      await pdf.close();
    }
  });

  it("opens the upstream annotation traversal fixture and exposes page annotations", async () => {
    const expected = pdfminerOracle();
    const pdf = await open("pdfminer-six/samples/contrib/issue-1082-annotations.pdf");
    try {
      expect(pdf.pages).toHaveLength(expected.annotationPages);
      expect(pdf.pages.filter((page) => page.annots.length > 0)).toHaveLength(expected.annotationPagesWithAnnots);
      expect(pdf.annots).toHaveLength(5);
    } finally {
      await pdf.close();
    }
  });
});
