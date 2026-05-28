import { describe, expect, it } from "vitest";

import { glyphTextLikePdfminer, glyphWidthLikePdfminer } from "../../src/font-decoding.js";
import { decodePdfLiteralBytesAsUtf8ThenUtf16, decodePdfStringLikePdfminer } from "../../src/pdf-strings.js";
import { parsePdfObjects } from "../../src/pdf.js";
import {
  annotationStringDecodeErrorLikePdfminer,
  normalizePdfStringLikePdfminer,
  shouldEmulatePdfminerOpenError,
  shouldSuppressPagesLikePdfminer,
  validateStreamsLikePdfminer
} from "../../src/pdfminer-compat.js";
import type { MappedFont } from "../../src/types.js";

function font(overrides: Partial<MappedFont> = {}): MappedFont {
  return {
    fontname: "F1",
    ascent: 0.8,
    descent: -0.2,
    fontMatrix0: 0.001,
    vertical: false,
    cidFallback: false,
    hasToUnicode: true,
    missingFile: false,
    ...overrides
  };
}

describe("low-level pdfminer string, glyph, and compatibility behavior", () => {
  it("decodes PDFDocEncoding and UTF-16BE strings like pdfminer metadata paths", () => {
    expect(decodePdfStringLikePdfminer([0x41, 0x90, 0x93, 0x94, 0xa0])).toBe("A\u2019\uFB01\uFB02\u20AC");
    expect(decodePdfStringLikePdfminer([0xfe, 0xff, 0x00, 0x41, 0x00, 0x78])).toBe("Ax");
  });

  it("prefers UTF-8 literal strings and falls back to UTF-16 when UTF-8 is invalid", () => {
    expect(decodePdfLiteralBytesAsUtf8ThenUtf16([0x48, 0xc3, 0xa9])).toBe("Hé");
    expect(decodePdfLiteralBytesAsUtf8ThenUtf16([0xfe, 0xff, 0x00, 0x41])).toBe("A");
    expect(decodePdfLiteralBytesAsUtf8ThenUtf16([0xff, 0xfe, 0x41, 0x00])).toBe("A");
  });

  it("maps glyph names, CIDs, control characters, and missing-font widths through pdfminer-like rules", () => {
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 33,
        glyphUnicode: "!",
        font: font({ fontRecord: { objectNumber: 1, baseFont: "F", firstChar: 32, widths: [], encodingDifferences: { 33: "quoteright" } } })
      })
    ).toBe("\u2019");

    expect(glyphTextLikePdfminer({ originalCharCode: 321, glyphUnicode: "x", font: font({ cidFallback: true }) })).toBe("(cid:321)");
    expect(glyphTextLikePdfminer({ originalCharCode: 65, glyphUnicode: "\u0001", font: font() })).toBe("A");
    expect(glyphTextLikePdfminer({ originalCharCode: 13, glyphUnicode: "\r", font: font() })).toBe("(cid:13)");
    expect(glyphTextLikePdfminer({ originalCharCode: 121, glyphUnicode: "\r", font: font() })).toBe("y");

    expect(
      glyphWidthLikePdfminer(
        font({
          missingFile: true,
          hasToUnicode: false,
          fontRecord: { objectNumber: 1, baseFont: "F", subtype: "TrueType", firstChar: 32, widths: [250, 333] }
        }),
        { originalCharCode: 33, width: 999 }
      )
    ).toBe(333);
  });

  it("validates pdfminer-compatible ASCII85 stream failures", () => {
    const objects = parsePdfObjects("1 0 obj\n<< /Filter /ASCII85Decode /Length 4 >>\nstream\nabc\u0000~>\nendstream\nendobj");

    expect(() => validateStreamsLikePdfminer({ raw: "", objects })).toThrow(/Non-Ascii85 digit/);
  });

  it("keeps narrow pdfminer open-error and page-suppression emulation scoped", () => {
    const suppressRaw = [
      "1 0 obj << /Type /Pages /Count 1 /Kids [3 0 R] >> endobj",
      "2 0 obj << /Producer (PyPDF2) /Title (IntMetadata) /Copies 0 /Note (%%Postscript (OFF)) >> endobj",
      "3 0 obj << /Type /Page /Parent 1 0 R /MediaBox [0 0 612 792] >> endobj",
      "4 0 obj << /Type /Catalog /Pages 1 0 R >> endobj"
    ].join("\n");
    const suppressObjects = parsePdfObjects(suppressRaw);

    expect(shouldSuppressPagesLikePdfminer({ raw: suppressRaw, objects: suppressObjects })).toBe(true);
    expect(shouldSuppressPagesLikePdfminer({ raw: suppressRaw.replace("/Copies 0", "/Copies 1"), objects: parsePdfObjects(suppressRaw.replace("/Copies 0", "/Copies 1")) })).toBe(false);
    expect(shouldEmulatePdfminerOpenError({ raw: "/Subtype /FreeText /Contents <eda080>", objects: new Map() })).toBeNull();
    expect(annotationStringDecodeErrorLikePdfminer(["/Subtype /FreeText /Contents <eda080>"])?.name).toBe("UnicodeDecodeError");
    expect(annotationStringDecodeErrorLikePdfminer(["/Subtype /FreeText /Contents (valid) /Contents <eda080>"])?.name).toBe("UnicodeDecodeError");
    expect(normalizePdfStringLikePdfminer("")).toBeNull();
    expect(normalizePdfStringLikePdfminer("value")).toBe("value");
  });
});
