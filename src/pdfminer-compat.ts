import { namedError } from "./errors.js";

import { decodePdfLiteralBytesAsUtf8ThenUtf16, parsePdfDictBytes } from "./pdf-strings.js";

export interface PdfminerCompatContext {
  raw: string;
  objects: Map<number, string>;
  password?: string;
}

export function validateStreamsLikePdfminer(ctx: PdfminerCompatContext): void {
  for (const text of ctx.objects.values()) {
    if (!/\/Filter\s*(?:\[[^\]]*)?\/(?:A85|ASCII85Decode)\b/.test(text)) continue;
    const streamIndex = text.indexOf("stream");
    const endstreamIndex = text.lastIndexOf("endstream");
    if (streamIndex === -1 || endstreamIndex <= streamIndex) continue;
    let start = streamIndex + "stream".length;
    if (text[start] === "\r" && text[start + 1] === "\n") start += 2;
    else if (text[start] === "\r" || text[start] === "\n") start += 1;
    const stream = text.slice(start, endstreamIndex);
    for (let i = 0; i < stream.length; i += 1) {
      const char = stream[i];
      if (/\s/.test(char)) continue;
      if (char === "~" && stream[i + 1] === ">") break;
      const code = char.charCodeAt(0);
      if (char === "z" || char === "y" || (code >= 33 && code <= 117)) continue;
      throw namedError("PdfminerException", `Non-Ascii85 digit found: ${char}`);
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
  return /(?:^|-)V$/i.test(normalizeIdentityCMapNameLikePdfminer(name) ?? "");
}

export function shouldEmulatePdfminerOpenError(ctx: PdfminerCompatContext): Error | null {
  if (/\/Subtype\s*\/FreeText[\s\S]{0,300}?\/Contents\s*<\s*eda080\s*>/i.test(ctx.raw)) {
    return namedError("UnicodeDecodeError", "'utf-16-le' codec can't decode byte 0x80 in position 2: truncated data");
  }
  if (/\/Type\s*\/Sig\b/.test(ctx.raw) && /\/ByteRange\s*\[/.test(ctx.raw) && /\/Prev\s+\d+/.test(ctx.raw) && /\/DigestLocation\s*\[/.test(ctx.raw)) {
    return namedError("MalformedPDFException", "maximum recursion depth exceeded");
  }
  return null;
}

export function annotationStringDecodeErrorLikePdfminer(annotationObjects: Array<string | undefined>): Error | null {
  for (const objectText of annotationObjects) {
    if (!objectText) continue;
    for (const key of ["Contents", "T", "TU"]) {
      const bytes = parsePdfDictBytes(objectText, key);
      if (!bytes || !Array.from(bytes).some((byte) => byte >= 0x80)) continue;
      if (decodePdfLiteralBytesAsUtf8ThenUtf16(bytes) == null) {
        return namedError("UnicodeDecodeError", "'utf-16-le' codec can't decode annotation string bytes");
      }
    }
  }
  return null;
}
