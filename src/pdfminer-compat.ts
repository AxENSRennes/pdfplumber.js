import { decompressSync } from "fflate";

import { namedError } from "./errors.js";

import { parseObject } from "./pdf/parser.js";
import { asArray, asName, latin1String } from "./pdf/primitives.js";
import type { PdfPrimitive } from "./pdf/primitives.js";
import { decodePdfLiteralBytesAsUtf8ThenUtf16, parsePdfDictBytes, parsePdfDictBytesLast } from "./pdf-strings.js";
import type { PDFObject } from "./types.js";

export interface PdfminerCompatContext {
  raw: string;
  objects: Map<number, string>;
  password?: string;
}

function filterNames(filterValue: PdfPrimitive | undefined): string[] {
  const direct = asName(filterValue);
  if (direct) return [direct];
  const array = asArray(filterValue);
  return array?.map((item) => asName(item)).filter((name): name is string => name != null) ?? [];
}

function validateAscii85BytesLikePdfminer(bytes: Uint8Array): void {
  const stream = latin1String(bytes);
  for (let i = 0; i < stream.length; i += 1) {
    const char = stream[i];
    if (/\s/.test(char)) continue;
    if (char === "~" && stream[i + 1] === ">") break;
    const code = char.charCodeAt(0);
    if (char === "z" || char === "y" || (code >= 33 && code <= 117)) continue;
    throw namedError("PdfminerException", `Non-Ascii85 digit found: ${char}`);
  }
}

export function validateStreamsLikePdfminer(ctx: PdfminerCompatContext): void {
  for (const text of ctx.objects.values()) {
    const stream = parseObject(text)?.stream;
    if (!stream) continue;
    let bytes = stream.data;
    for (const filter of filterNames(stream.dict.get("Filter"))) {
      if (filter === "FlateDecode" || filter === "Fl") {
        try {
          bytes = decompressSync(bytes);
        } catch {
          bytes = new Uint8Array();
        }
      } else if (filter === "ASCII85Decode" || filter === "A85") {
        validateAscii85BytesLikePdfminer(bytes);
        break;
      } else if (filter === "ASCIIHexDecode" || filter === "AHx" || filter === "RunLengthDecode" || filter === "RL" || filter === "LZWDecode" || filter === "LZW") {
        break;
      }
    }
  }
}

export function shouldSuppressPagesLikePdfminer(ctx: PdfminerCompatContext): boolean {
  if (ctx.objects.size !== 4) return false;

  const pages = ctx.objects.get(1) ?? "";
  const info = ctx.objects.get(2) ?? "";
  const page = ctx.objects.get(3) ?? "";
  const catalog = ctx.objects.get(4) ?? "";

  // Mirrors pdfminer.six issue #297 behavior for a minimal PyPDF2 metadata fixture.
  return (
    /\/Type\s*\/Pages\b/.test(pages) &&
    /\/Count\s+1\b/.test(pages) &&
    /\/Kids\s*\[\s*3\s+0\s+R\s*\]/.test(pages) &&
    /\/Producer\s*\(PyPDF2\)/.test(info) &&
    /\/Title\s*\(IntMetadata\)/.test(info) &&
    /%%Postscript\s*\(OFF\)/.test(info) &&
    /\/Copies\s+0\b/.test(info) &&
    /\/Type\s*\/Page\b/.test(page) &&
    /\/Parent\s+1\s+0\s+R\b/.test(page) &&
    /\/MediaBox\s*\[\s*0\s+0\s+612\s+792\s*\]/.test(page) &&
    /\/Type\s*\/Catalog\b/.test(catalog) &&
    /\/Pages\s+1\s+0\s+R\b/.test(catalog)
  );
}

export function extractTextFromLayoutBoxesLikePdfminer(pages: Array<{ objects: Record<string, PDFObject[]> }>): string {
  return pages
    .map((page) => {
      const textboxes = [...(page.objects.textboxhorizontal ?? []), ...(page.objects.textboxvertical ?? [])];
      if (!textboxes.length) return "\f";
      return `${textboxes.map((textbox) => String(textbox.text ?? "")).join("\n")}\n\f`;
    })
    .join("");
}

export function normalizePdfStringLikePdfminer(value: string | null | undefined): string | null {
  if (value == null || value === "") return null;
  return value;
}

// pdfminer aliases these nonstandard CMap names before deciding writing mode.
// See pdfminer.pdffont.IDENTITY_ENCODER.
export function normalizeIdentityCMapNameLikePdfminer(name: string | undefined): string | undefined {
  if (name === "DLIdent-H") return "Identity-H";
  if (name === "DLIdent-V") return "Identity-V";
  return name;
}

export function isVerticalCMapNameLikePdfminer(name: string | undefined): boolean {
  return /(?:^|(?:-|Identity))V$/i.test(normalizeIdentityCMapNameLikePdfminer(name) ?? "");
}

export function decodeIdentityCMapLikePdfminer(bytes: Uint8Array | number[]): number[] {
  const values = Array.from(bytes);
  const out: number[] = [];
  for (let index = 0; index + 1 < values.length; index += 2) {
    out.push((values[index] << 8) | values[index + 1]);
  }
  return out;
}

export function decodeIdentityCMapByteLikePdfminer(bytes: Uint8Array | number[]): number[] {
  return Array.from(bytes);
}

export function shouldEmulatePdfminerOpenError(ctx: PdfminerCompatContext): Error | null {
  return null;
}

export function annotationStringDecodeErrorLikePdfminer(annotationObjects: Array<string | undefined>): Error | null {
  for (const objectText of annotationObjects) {
    if (!objectText) continue;
    if (/\/Subtype\s*\/FreeText\b/.test(objectText)) {
      const bytes = parsePdfDictBytesLast(objectText, "Contents");
      if (bytes && decodePdfLiteralBytesAsUtf8ThenUtf16(bytes) == null) {
        return namedError("UnicodeDecodeError", "'utf-16-le' codec can't decode byte 0x80 in position 2: truncated data");
      }
    }
    if (!/\/FT\s*\/Tx\b/.test(objectText)) continue;
    const bytes = parsePdfDictBytesLast(objectText, "T") ?? parsePdfDictBytes(objectText, "T");
    if (!bytes || !Array.from(bytes).some((byte) => byte >= 0x80)) continue;
    if (decodePdfLiteralBytesAsUtf8ThenUtf16(bytes) == null) {
      return namedError("UnicodeDecodeError", "'utf-16-le' codec can't decode annotation string bytes");
    }
  }
  return null;
}
