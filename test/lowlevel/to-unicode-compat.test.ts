import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { open } from "../../src/index.js";

class PdfBuilder {
  private objects: Uint8Array[] = [];

  add(body: string | Uint8Array): number {
    const bytes = typeof body === "string" ? latin1Bytes(body) : body;
    this.objects.push(bytes);
    return this.objects.length;
  }

  stream(dictionary: string, data: string | Uint8Array): number {
    const bytes = typeof data === "string" ? latin1Bytes(data) : data;
    return this.add(concatBytes(latin1Bytes(`<< ${dictionary} /Length ${bytes.length} >>\nstream\n`), bytes, latin1Bytes("\nendstream")));
  }

  write(rootObject: number): Uint8Array {
    const chunks: Uint8Array[] = [latin1Bytes("%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")];
    const offsets = [0];
    let length = chunks[0].length;
    for (let index = 0; index < this.objects.length; index += 1) {
      offsets.push(length);
      const prefix = latin1Bytes(`${index + 1} 0 obj\n`);
      const suffix = latin1Bytes("\nendobj\n");
      chunks.push(prefix, this.objects[index], suffix);
      length += prefix.length + this.objects[index].length + suffix.length;
    }
    const xrefOffset = length;
    chunks.push(latin1Bytes(`xref\n0 ${this.objects.length + 1}\n0000000000 65535 f \n`));
    for (const offset of offsets.slice(1)) chunks.push(latin1Bytes(`${String(offset).padStart(10, "0")} 00000 n \n`));
    chunks.push(latin1Bytes(`trailer\n<< /Size ${this.objects.length + 1} /Root ${rootObject} 0 R >>\nstartxref\n${xrefOffset}\n%%EOF\n`));
    return concatBytes(...chunks);
  }
}

function latin1Bytes(value: string): Uint8Array {
  const bytes = new Uint8Array(value.length);
  for (let index = 0; index < value.length; index += 1) bytes[index] = value.charCodeAt(index) & 0xff;
  return bytes;
}

function concatBytes(...chunks: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(chunks.reduce((sum, chunk) => sum + chunk.length, 0));
  let offset = 0;
  for (const chunk of chunks) {
    out.set(chunk, offset);
    offset += chunk.length;
  }
  return out;
}

function utf16beHex(value: string): string {
  let out = "";
  for (let index = 0; index < value.length; index += 1) out += value.charCodeAt(index).toString(16).toUpperCase().padStart(4, "0");
  return out;
}

function toUnicodeCMap(code: number, text: string): string {
  return [
    "/CIDInit /ProcSet findresource begin",
    "12 dict begin",
    "begincmap",
    "/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def",
    "/CMapName /HighPlaneToUnicode def",
    "/CMapType 2 def",
    "1 begincodespacerange",
    "<0000> <FFFF>",
    "endcodespacerange",
    "1 beginbfchar",
    `<${code.toString(16).toUpperCase().padStart(4, "0")}> <${utf16beHex(text)}>`,
    "endbfchar",
    "endcmap",
    "CMapName currentdict /CMap defineresource pop",
    "end",
    "end"
  ].join("\n");
}

function highPlaneToUnicodePdf(text: string): Uint8Array {
  const pdf = new PdfBuilder();
  const toUnicode = pdf.stream("", toUnicodeCMap(0x20, text));
  const descriptor = "<< /Type /FontDescriptor /FontName /HighPlaneCID /Flags 32 /Ascent 718 /Descent -207 /CapHeight 718 /ItalicAngle 0 /StemV 80 >>";
  const descendant = pdf.add(`<< /Type /Font /Subtype /CIDFontType2 /BaseFont /HighPlaneCID /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor ${descriptor} /DW 600 >>`);
  const font = pdf.add(`<< /Type /Font /Subtype /Type0 /BaseFont /HighPlaneCID /Encoding /Identity-H /DescendantFonts [${descendant} 0 R] /ToUnicode ${toUnicode} 0 R >>`);
  const content = pdf.stream("", "BT /F1 24 Tf 40 100 Td <0020> Tj ET");
  const page = pdf.add(`<< /Type /Page /MediaBox [0 0 180 140] /Resources << /Font << /F1 ${font} 0 R >> >> /Contents ${content} 0 R >>`);
  const pages = pdf.add(`<< /Type /Pages /Kids [${page} 0 R] /Count 1 >>`);
  const catalog = pdf.add(`<< /Type /Catalog /Pages ${pages} 0 R >>`);
  return pdf.write(catalog);
}

function pdfplumberTextOracle(pdfBytes: Uint8Array): string {
  const dir = mkdtempSync(join(tmpdir(), "pdfplumber-js-to-unicode-"));
  const path = join(dir, "high-plane.pdf");
  try {
    writeFileSync(path, pdfBytes);
    const code = `
import json
import pdfplumber
with pdfplumber.open(${JSON.stringify(path)}) as pdf:
    print(json.dumps(pdf.pages[0].extract_text()))
`;
    return JSON.parse(execFileSync("wsl_venv/bin/python", ["-c", code], { encoding: "utf8" })) as string;
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

describe("low-level ToUnicode compatibility", () => {
  it("extracts Extension B ToUnicode mappings like pdfplumber", async () => {
    const text = String.fromCodePoint(0x20000);
    const pdfBytes = highPlaneToUnicodePdf(text);
    const expected = pdfplumberTextOracle(pdfBytes);
    const pdf = await open(pdfBytes);
    try {
      expect(await Promise.resolve(pdf.pages[0].extractText())).toBe(expected);
      expect(pdf.pages[0].chars.map((char) => char.text).join("")).toBe(expected);
    } finally {
      await pdf.close();
    }
  });
});
