import { describe, expect, it } from "vitest";

import {
  fontCharWidthLikePdfminer,
  getWidthsLikePdfminer,
  glyphNameToUnicodeLikePdfminer,
  glyphTextLikePdfminer,
  glyphWidthLikePdfminer
} from "../../src/font-decoding.js";
import { decodePdfLiteralBytesAsUtf8ThenUtf16, decodePdfStringLikePdfminer } from "../../src/pdf-strings.js";
import { parsePdfObjects } from "../../src/pdf.js";
import {
  annotationStringDecodeErrorLikePdfminer,
  decodeIdentityCMapByteLikePdfminer,
  decodeIdentityCMapLikePdfminer,
  isVerticalCMapNameLikePdfminer,
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
    expect(glyphNameToUnicodeLikePdfminer("hyphen")).toBe("-");
    expect(glyphNameToUnicodeLikePdfminer("minus")).toBe("\u2212");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 33,
        glyphUnicode: "!",
        font: font({ fontRecord: { objectNumber: 1, baseFont: "F", firstChar: 32, widths: [], encodingDifferences: { 33: "quoteright" } } })
      })
    ).toBe("\u2019");

    expect(glyphTextLikePdfminer({ originalCharCode: 321, glyphUnicode: "x", font: font({ cidFallback: true }) })).toBe("(cid:321)");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 32,
        glyphUnicode: " ",
        font: font({ hasToUnicode: false, fontRecord: { objectNumber: 1, baseFont: "LSXICB+CMR9", subtype: "Type1", hasToUnicode: false, symbolic: true, charSet: ["A"], firstChar: 11, widths: [] } })
      })
    ).toBe("(cid:32)");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 33,
        glyphUnicode: "\u00ed",
        font: font({
          hasToUnicode: false,
          fontRecord: { objectNumber: 1, baseFont: "DBCFPF+MSTT31c64e", subtype: "Type1", hasToUnicode: false, symbolic: true, encodingDifferences: { 33: "GED", 58: "GFA" }, firstChar: 1, widths: [] }
        })
      })
    ).toBe("!");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 39,
        glyphUnicode: "i",
        font: font({
          hasToUnicode: true,
          fontRecord: { objectNumber: 1, baseFont: "KXINQC+AdvT905", subtype: "Type1", hasToUnicode: true, symbolic: true, encodingDifferences: { 39: "C105", 47: "C71" }, firstChar: 33, widths: [] }
        })
      })
    ).toBe("\u2019");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 47,
        glyphUnicode: "G",
        font: font({
          hasToUnicode: true,
          fontRecord: { objectNumber: 1, baseFont: "KXINQC+AdvT905", subtype: "Type1", hasToUnicode: true, symbolic: true, encodingDifferences: { 39: "C105", 47: "C71" }, firstChar: 33, widths: [] }
        })
      })
    ).toBe("/");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 241,
        glyphUnicode: "\u00e6",
        font: font({
          hasToUnicode: false,
          fontRecord: { objectNumber: 1, baseFont: "ArialMT", subtype: "TrueType", hasToUnicode: false, symbolic: false, firstChar: 32, widths: [] }
        })
      })
    ).toBe("\u00f1");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 136,
        glyphUnicode: "\u00e0",
        font: font({
          hasToUnicode: false,
          fontRecord: { objectNumber: 1, baseFont: "HEJHEO+NewCenturySchlbk-Roman+2", subtype: "TrueType", hasToUnicode: false, symbolic: true, embeddedUnicodeMap: { 136: "\uf008" }, firstChar: 0, widths: [] }
        })
      })
    ).toBe("\u02c6");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 141,
        glyphUnicode: "\u00e7",
        font: font({
          hasToUnicode: false,
          fontRecord: { objectNumber: 1, baseFont: "HEJHEO+NewCenturySchlbk-Roman+2", subtype: "TrueType", hasToUnicode: false, symbolic: true, embeddedUnicodeMap: { 141: "\uf00d" }, firstChar: 0, widths: [] }
        })
      })
    ).toBe("(cid:141)");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 160,
        glyphUnicode: "\u00a0",
        font: font({
          hasToUnicode: false,
          fontRecord: { objectNumber: 1, baseFont: "Helvetica", subtype: "Type1", hasToUnicode: false, encodingDifferences: { 160: ".notdef" }, firstChar: 32, widths: [] }
        })
      })
    ).toBe("(cid:160)");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 3,
        glyphUnicode: "\u2217",
        font: font({
          hasToUnicode: false,
          fontRecord: { objectNumber: 1, baseFont: "SJLUHL+CMSY6", subtype: "Type1", hasToUnicode: false, symbolic: true, charSet: ["asteriskmath"], firstChar: 3, widths: [639] }
        })
      })
    ).toBe("\u2217");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 15,
        glyphUnicode: "\u000f",
        font: font({
          hasToUnicode: false,
          fontRecord: { objectNumber: 1, baseFont: "HJCXGN+CMSY7", subtype: "Type1", hasToUnicode: false, symbolic: true, charSet: ["bullet"], firstChar: 15, widths: [585.3] }
        })
      })
    ).toBe("\u2022");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 163,
        glyphUnicode: "\u00d7",
        font: font({
          hasToUnicode: false,
          fontRecord: { objectNumber: 1, baseFont: "KHPFLE+MTSY", subtype: "Type1", hasToUnicode: false, symbolic: true, charSet: ["multiply"], firstChar: 0, widths: [] }
        })
      })
    ).toBe("\u00a3");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 102,
        glyphUnicode: "{",
        font: font({
          hasToUnicode: false,
          fontRecord: { objectNumber: 1, baseFont: "KHPFLE+MTSY", subtype: "Type1", hasToUnicode: false, symbolic: true, charSet: ["braceleft"], firstChar: 0, widths: [] }
        })
      })
    ).toBe("f");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 176,
        glyphUnicode: "\u00a9",
        font: font({
          hasToUnicode: false,
          fontRecord: { objectNumber: 1, baseFont: "KHPFLE+MTSY", subtype: "Type1", hasToUnicode: false, symbolic: true, charSet: ["copyright"], firstChar: 0, widths: [] }
        })
      })
    ).toBe("(cid:176)");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 126,
        glyphUnicode: "\u210f",
        font: font({ hasToUnicode: false, fontRecord: { objectNumber: 1, baseFont: "YSIEBR+MSBM10", subtype: "Type1", hasToUnicode: false, symbolic: true, charSet: ["planckover2pi1"], firstChar: 126, widths: [540.3] } })
      })
    ).toBe("(cid:126)");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 32,
        glyphUnicode: " ",
        font: font({ hasToUnicode: false, fontRecord: { objectNumber: 1, baseFont: "F", subtype: "Type1", hasToUnicode: false, symbolic: true, charSet: ["space"], firstChar: 32, widths: [] } })
      })
    ).toBe(" ");
    expect(glyphTextLikePdfminer({ originalCharCode: 65, glyphUnicode: "\u0001", font: font() })).toBe("A");
    expect(glyphTextLikePdfminer({ originalCharCode: 0, glyphUnicode: "\u0000", font: font() })).toBe("(cid:0)");
    expect(glyphTextLikePdfminer({ originalCharCode: 65, glyphUnicode: "\u0000", font: font() })).toBe("A");
    expect(glyphTextLikePdfminer({ originalCharCode: 10, glyphUnicode: "\n", font: font() })).toBe("(cid:10)");
    expect(glyphTextLikePdfminer({ originalCharCode: 2, glyphUnicode: "\n", font: font() })).toBe("\n");
    expect(glyphTextLikePdfminer({ originalCharCode: 99, glyphUnicode: "\r", font: font({ fontname: "unknown", fontRecord: { objectNumber: 1, baseFont: "F", subtype: "Type3", firstChar: 45, widths: [] } }) })).toBe("\r");
    expect(glyphTextLikePdfminer({ originalCharCode: 13, glyphUnicode: "\r", font: font() })).toBe("(cid:13)");
    expect(glyphTextLikePdfminer({ originalCharCode: 121, glyphUnicode: "\r", font: font() })).toBe("y");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 72,
        glyphUnicode: "H",
        font: font({
          hasToUnicode: false,
          fontRecord: { objectNumber: 1, baseFont: "AIGDT", subtype: "Type0", hasToUnicode: false, firstChar: 0, widths: [], embeddedUnicodeMap: { 72: "\uf066" } }
        })
      })
    ).toBe("(cid:72)");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 142,
        glyphUnicode: "\u008e",
        font: font({ hasToUnicode: false, fontRecord: { objectNumber: 1, baseFont: "ISOCPEUR", subtype: "Type0", hasToUnicode: false, firstChar: 0, widths: [] } })
      })
    ).toBe("(cid:142)");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 0xa14b,
        glyphUnicode: "\u22ef",
        font: font({
          hasToUnicode: false,
          fontRecord: {
            objectNumber: 1,
            baseFont: "DFKaiShu-Md-HK-BF",
            subtype: "Type0",
            cidCoding: "Adobe-CNS1",
            encodingName: "B5pc-H",
            hasToUnicode: false,
            symbolic: true,
            firstChar: 0,
            widths: []
          }
        })
      })
    ).toBe("\u2026");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 0xa14b,
        glyphUnicode: "\u22ef",
        font: font({
          hasToUnicode: false,
          fontRecord: {
            objectNumber: 1,
            baseFont: "DFMing-Md-HK-BF",
            subtype: "Type0",
            cidCoding: "Adobe-CNS1",
            encodingName: "ETen-B5-H",
            hasToUnicode: false,
            symbolic: true,
            firstChar: 0,
            widths: []
          }
        })
      })
    ).toBe("\u2026");

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
    expect(
      glyphWidthLikePdfminer(
        font({
          fontname: "Helvetica",
          hasToUnicode: false,
          fontRecord: { objectNumber: 1, baseFont: "Arial", subtype: "TrueType", encodingName: "WinAnsiEncoding", hasToUnicode: false, firstChar: 32, widths: [] }
        }),
        { originalCharCode: 149, unicode: "\u2022", width: 750 }
      )
    ).toBe(350);
    expect(
      glyphWidthLikePdfminer(
        font({
          fontname: "Helvetica",
          hasToUnicode: false,
          fontRecord: { objectNumber: 1, baseFont: "Arial", subtype: "TrueType", encodingName: "WinAnsiEncoding", hasToUnicode: false, firstChar: 32, widths: [] }
        }),
        { originalCharCode: 150, unicode: "\u2013", width: 750 }
      )
    ).toBe(556);
    expect(
      glyphWidthLikePdfminer(
        font({
          fontname: "Helvetica",
          hasToUnicode: false,
          fontRecord: { objectNumber: 1, baseFont: "Arial", subtype: "TrueType", encodingName: "WinAnsiEncoding", hasToUnicode: false, firstChar: 32, widths: [] }
        }),
        { originalCharCode: 128, unicode: "\u20ac", width: 750 }
      )
    ).toBe(0);
  });

  it("recovers TeX symbolic Type1 text through pdfminer-style glyph-name and CID fallbacks", () => {
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 121,
        glyphUnicode: "y",
        font: font({ fontRecord: { objectNumber: 1, baseFont: "VBZNJT+txsys", subtype: "Type1", hasToUnicode: true, symbolic: true, charSet: ["bar", "dagger", "slash"], firstChar: 0, widths: [] } })
      })
    ).toBe("\u2020");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 106,
        glyphUnicode: "j",
        font: font({ fontRecord: { objectNumber: 1, baseFont: "VBZNJT+txsys", subtype: "Type1", hasToUnicode: true, symbolic: true, charSet: ["bar", "dagger", "slash"], firstChar: 0, widths: [] } })
      })
    ).toBe("|");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 157,
        glyphUnicode: "\u009d",
        font: font({ fontRecord: { objectNumber: 1, baseFont: "VBZNJT+txsys", subtype: "Type1", hasToUnicode: true, symbolic: true, charSet: ["bar", "dagger", "slash"], firstChar: 0, widths: [] } })
      })
    ).toBe("/");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 12,
        glyphUnicode: "\f",
        font: font({ fontRecord: { objectNumber: 1, baseFont: "VBZNJT+txsys", subtype: "Type1", hasToUnicode: true, symbolic: true, charSet: ["bar", "dagger", "slash"], firstChar: 0, widths: [] } })
      })
    ).toBe("(cid:12)");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 149,
        glyphUnicode: "\u0095",
        font: font({ fontRecord: { objectNumber: 1, baseFont: "VPSDBU+NewTXMI", subtype: "Type1", hasToUnicode: true, symbolic: true, charSet: ["period", "greater"], firstChar: 17, widths: [] } })
      })
    ).toBe(".");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 161,
        glyphUnicode: "\u00a1",
        font: font({ fontRecord: { objectNumber: 1, baseFont: "VPSDBU+NewTXMI", subtype: "Type1", hasToUnicode: true, symbolic: true, charSet: ["period", "greater"], firstChar: 17, widths: [] } })
      })
    ).toBe(">");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 38,
        glyphUnicode: "&",
        font: font({ fontRecord: { objectNumber: 1, baseFont: "MBVNDY+txsya", subtype: "Type1", hasToUnicode: true, symbolic: true, charSet: ["greaterorsimilar", "lessorsimilar"], firstChar: 38, widths: [] } })
      })
    ).toBe("(cid:38)");
    expect(
      glyphTextLikePdfminer({
        originalCharCode: 46,
        glyphUnicode: ".",
        font: font({ fontRecord: { objectNumber: 1, baseFont: "MBVNDY+txsya", subtype: "Type1", hasToUnicode: true, symbolic: true, charSet: ["greaterorsimilar", "lessorsimilar"], firstChar: 38, widths: [] } })
      })
    ).toBe("(cid:46)");
  });

  it("handles PDFFont width defaults and get_widths lists like pdfminer", () => {
    expect(fontCharWidthLikePdfminer({}, 0, 100)).toBe(0.1);
    expect(fontCharWidthLikePdfminer({ 0: 50 }, 0, 100)).toBe(0.05);
    expect(fontCharWidthLikePdfminer({ 0: 200 }, 0, 100)).toBe(0.2);
    expect(fontCharWidthLikePdfminer({ 0: null }, 0, 100)).toBe(0.1);

    expect(getWidthsLikePdfminer([0, [1, 2, 3, 4]])).toEqual({ 0: 1, 1: 2, 2: 3, 3: 4 });
    expect(getWidthsLikePdfminer([0, 4, 3])).toEqual({ 0: 3, 1: 3, 2: 3, 3: 3, 4: 3 });

    const ref = { kind: "ref", objectNumber: 121 };
    expect(getWidthsLikePdfminer([0, ref], (value) => (value === ref ? [1, 2, 3, 4] : value))).toEqual({ 0: 1, 1: 2, 2: 3, 3: 4 });
  });

  it("decodes Adobe glyph names like pdfminer.encodingdb.name2unicode", () => {
    const highPlane = String.fromCodePoint(0x1040c);
    expect(glyphNameToUnicodeLikePdfminer("Lcommaaccent")).toBe("\u013b");
    expect(glyphNameToUnicodeLikePdfminer("uni013B")).toBe("\u013b");
    expect(glyphNameToUnicodeLikePdfminer("uni013b")).toBe("\u013b");
    expect(glyphNameToUnicodeLikePdfminer("uni20AC0308")).toBe("\u20ac\u0308");
    expect(glyphNameToUnicodeLikePdfminer("uni20ac0308")).toBe("\u20ac\u0308");
    expect(glyphNameToUnicodeLikePdfminer("uni20ac")).toBe("\u20ac");
    expect(glyphNameToUnicodeLikePdfminer("uniD801DC0C")).toBeNull();
    expect(glyphNameToUnicodeLikePdfminer("uniF6FB")).toBe("\uf6fb");
    expect(glyphNameToUnicodeLikePdfminer("unif6fb")).toBe("\uf6fb");
    expect(glyphNameToUnicodeLikePdfminer("u013B")).toBe("\u013b");
    expect(glyphNameToUnicodeLikePdfminer("u013b")).toBe("\u013b");
    expect(glyphNameToUnicodeLikePdfminer("u1040C")).toBe(highPlane);
    expect(glyphNameToUnicodeLikePdfminer("u1040c")).toBe(highPlane);
    expect(glyphNameToUnicodeLikePdfminer("Lcommaaccent_uni20AC0308_u1040C.alternate")).toBe(`\u013b\u20ac\u0308${highPlane}`);
    expect(glyphNameToUnicodeLikePdfminer("Lcommaaccent_uni20ac0308_u1040c.alternate")).toBe(`\u013b\u20ac\u0308${highPlane}`);
    expect(glyphNameToUnicodeLikePdfminer("foo")).toBeNull();
    expect(glyphNameToUnicodeLikePdfminer(".notdef")).toBeNull();
    expect(glyphNameToUnicodeLikePdfminer("Ogoneksmall")).toBe("\uf6fb");
    expect(glyphNameToUnicodeLikePdfminer("226215240241240240240240")).toBeNull();
  });

  it("decodes Identity CMaps and CMap writing-mode aliases like pdfminer", () => {
    const pairs = (values: number[]) => values.flatMap((value) => [(value >> 8) & 0xff, value & 0xff]);

    expect(decodeIdentityCMapLikePdfminer([])).toEqual([]);
    expect(decodeIdentityCMapLikePdfminer([0])).toEqual([]);
    expect(decodeIdentityCMapLikePdfminer([...pairs([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]), 0])).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    expect(decodeIdentityCMapLikePdfminer(pairs([65535, 65534, 65533, 65532, 65531]))).toEqual([65535, 65534, 65533, 65532, 65531]);
    expect(decodeIdentityCMapByteLikePdfminer([0, 1, 2, 3, 4])).toEqual([0, 1, 2, 3, 4]);
    expect(decodeIdentityCMapByteLikePdfminer(Array.from({ length: 255 }, (_value, index) => index)).slice(-5)).toEqual([250, 251, 252, 253, 254]);
    expect(isVerticalCMapNameLikePdfminer("Identity-H")).toBe(false);
    expect(isVerticalCMapNameLikePdfminer("Identity-V")).toBe(true);
    expect(isVerticalCMapNameLikePdfminer("DLIdent-H")).toBe(false);
    expect(isVerticalCMapNameLikePdfminer("DLIdent-V")).toBe(true);
    expect(isVerticalCMapNameLikePdfminer("OneByteIdentityV")).toBe(true);
    expect(isVerticalCMapNameLikePdfminer("OneByteIdentityH")).toBe(false);
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
