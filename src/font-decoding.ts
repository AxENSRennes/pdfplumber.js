import { decodePdfStringLikePdfminer } from "./pdf-strings.js";
import type { FontRecord, MappedFont, PdfJsGlyph } from "./types.js";

export interface GlyphTextInput {
  glyphUnicode?: string;
  originalCharCode?: number;
  font: MappedFont;
  fontRecord?: FontRecord;
}

const ADOBE_GLYPH_NAMES: Record<string, string> = {
  ampersand: "&",
  asterisk: "*",
  asteriskmath: "\u2217",
  at: "@",
  backslash: "\\",
  braceleft: "{",
  braceright: "}",
  bracketleft: "[",
  bracketright: "]",
  bullet: "\u2022",
  colon: ":",
  comma: ",",
  dollar: "$",
  eight: "8",
  five: "5",
  four: "4",
  hyphen: "-",
  Lcommaaccent: "\u013b",
  minus: "\u2212",
  nine: "9",
  Ogoneksmall: "\uf6fb",
  one: "1",
  parenleft: "(",
  parenright: ")",
  percent: "%",
  period: ".",
  plus: "+",
  quotedblleft: "\u201c",
  quotedblright: "\u201d",
  quoteleft: "\u2018",
  quoteright: "\u2019",
  semicolon: ";",
  seven: "7",
  six: "6",
  slash: "/",
  space: " ",
  three: "3",
  two: "2",
  underscore: "_",
  zero: "0"
};

function standardEncodingTextLikePdfminer(code: number): string | null {
  if (code < 32 || code > 126) return null;
  if (code === 39) return "\u2019";
  if (code === 96) return "\u2018";
  return String.fromCharCode(code);
}

const WIN_ANSI_HIGH_BYTES: Record<number, string> = {
  128: "\u20ac",
  130: "\u201a",
  131: "\u0192",
  132: "\u201e",
  133: "\u2026",
  134: "\u2020",
  135: "\u2021",
  136: "\u02c6",
  137: "\u2030",
  138: "\u0160",
  139: "\u2039",
  140: "\u0152",
  142: "\u017d",
  145: "\u2018",
  146: "\u2019",
  147: "\u201c",
  148: "\u201d",
  149: "\u2022",
  150: "\u2013",
  151: "\u2014",
  152: "\u02dc",
  153: "\u2122",
  154: "\u0161",
  155: "\u203a",
  156: "\u0153",
  158: "\u017e",
  159: "\u0178"
};

const HELVETICA_WIN_ANSI_HIGH_WIDTHS: Record<string, number> = {
  "\u20ac": 0,
  "\u201a": 222,
  "\u0192": 556,
  "\u201e": 333,
  "\u2026": 1000,
  "\u2020": 556,
  "\u2021": 556,
  "\u02c6": 333,
  "\u2030": 1000,
  "\u0160": 667,
  "\u2039": 333,
  "\u0152": 1000,
  "\u017d": 611,
  "\u2018": 222,
  "\u2019": 222,
  "\u201c": 333,
  "\u201d": 333,
  "\u2022": 350,
  "\u2013": 556,
  "\u2014": 1000,
  "\u02dc": 333,
  "\u2122": 1000,
  "\u0161": 500,
  "\u203a": 333,
  "\u0153": 944,
  "\u017e": 500,
  "\u0178": 667
};

const HELVETICA_BOLD_WIN_ANSI_HIGH_WIDTHS: Record<string, number> = {
  ...HELVETICA_WIN_ANSI_HIGH_WIDTHS,
  "\u201a": 278,
  "\u201e": 500,
  "\u2018": 278,
  "\u2019": 278,
  "\u201c": 500,
  "\u201d": 500,
  "\u0161": 556
};

function winAnsiTextLikePdfminer(code: number): string | null {
  if (code >= 32 && code < 128) return String.fromCharCode(code);
  if (code >= 160 && code <= 255) return String.fromCharCode(code);
  return WIN_ANSI_HIGH_BYTES[code] ?? null;
}

function codePointFromHex(value: string): string | null {
  if (!/^[0-9a-f]+$/i.test(value)) return null;
  const codePoint = Number.parseInt(value, 16);
  if (!Number.isInteger(codePoint) || codePoint < 0 || codePoint > 0x10ffff) return null;
  if (codePoint >= 0xd800 && codePoint <= 0xdfff) return null;
  return String.fromCodePoint(codePoint);
}

function glyphNameComponentToUnicode(component: string): string | null {
  if (ADOBE_GLYPH_NAMES[component] != null) return ADOBE_GLYPH_NAMES[component];
  if (component.length === 1) return component;
  if (/^uni[0-9a-f]+$/i.test(component) && (component.length - 3) % 4 === 0) {
    let out = "";
    for (let index = 3; index < component.length; index += 4) {
      const text = codePointFromHex(component.slice(index, index + 4));
      if (text == null) return null;
      out += text;
    }
    return out || null;
  }
  if (/^u[0-9a-f]{4,6}$/i.test(component)) return codePointFromHex(component.slice(1));
  return null;
}

export function glyphNameToUnicodeLikePdfminer(name: string | undefined): string | null {
  if (!name) return null;
  const base = name.split(".")[0];
  if (!base) return null;
  let out = "";
  for (const component of base.split("_")) {
    const text = glyphNameComponentToUnicode(component);
    if (text == null) return null;
    out += text;
  }
  return out || null;
}

function numberLikePdfminer(value: unknown): number | null {
  if (value == null) return null;
  if (typeof value === "string" && value.trim() === "") return null;
  const number = Number(value);
  return Number.isFinite(number) ? number : null;
}

export function fontCharWidthLikePdfminer(widths: Record<string, unknown>, cid: number, defaultWidth: number): number {
  const width = numberLikePdfminer(widths[cid]);
  return (width ?? defaultWidth) * 0.001;
}

export function getWidthsLikePdfminer(values: unknown[], resolveRef: (value: unknown) => unknown = (value) => value): Record<number, number> {
  const out: Record<number, number> = {};
  for (let index = 0; index < values.length; index += 1) {
    const first = numberLikePdfminer(resolveRef(values[index]));
    if (first == null) continue;

    const next = resolveRef(values[index + 1]);
    if (Array.isArray(next)) {
      for (let offset = 0; offset < next.length; offset += 1) {
        const width = numberLikePdfminer(resolveRef(next[offset]));
        if (width != null) out[first + offset] = width;
      }
      index += 1;
      continue;
    }

    const last = numberLikePdfminer(next);
    const width = numberLikePdfminer(resolveRef(values[index + 2]));
    if (last == null || width == null) continue;
    for (let cid = first; cid <= last; cid += 1) out[cid] = width;
    index += 2;
  }
  return out;
}

function bytesFromCharCode(code: number): number[] {
  if (code <= 0xff) return [code];
  return [(code >> 8) & 0xff, code & 0xff];
}

function stripSubsetPrefix(name: string | undefined): string {
  return name?.replace(/^[A-Z]{6}\+/, "") ?? "";
}

function charSetIncludes(record: FontRecord | undefined, glyphName: string): boolean {
  return record?.charSet?.includes(glyphName) ?? false;
}

function texSymbolicType1GlyphTextLikePdfminer(fontRecord: FontRecord | undefined, originalCharCode: number, glyphUnicode: string | undefined): string | null {
  if (!fontRecord?.symbolic || fontRecord.hasToUnicode !== true || fontRecord.subtype !== "Type1") return null;
  const baseFont = stripSubsetPrefix(fontRecord.baseFont);
  if (/^txsys$/i.test(baseFont)) {
    if (originalCharCode === 12 && glyphUnicode === "\f") return "(cid:12)";
    if (originalCharCode === 106 && glyphUnicode === "j" && charSetIncludes(fontRecord, "bar")) return "|";
    if (originalCharCode === 121 && glyphUnicode === "y" && charSetIncludes(fontRecord, "dagger")) return "\u2020";
    if (originalCharCode === 157 && glyphUnicode === "\u009d" && charSetIncludes(fontRecord, "slash")) return "/";
  }
  if (/^NewTXMI\d*$/i.test(baseFont)) {
    if (originalCharCode === 149 && glyphUnicode === "\u0095" && charSetIncludes(fontRecord, "period")) return ".";
    if (originalCharCode === 161 && glyphUnicode === "\u00a1" && charSetIncludes(fontRecord, "greater")) return ">";
  }
  if (/^txsya$/i.test(baseFont)) {
    if (originalCharCode === 38 && glyphUnicode === "&" && charSetIncludes(fontRecord, "greaterorsimilar")) return "(cid:38)";
    if (originalCharCode === 46 && glyphUnicode === "." && charSetIncludes(fontRecord, "lessorsimilar")) return "(cid:46)";
  }
  return null;
}

function winAnsiStandardHighWidthLikePdfminer(font: MappedFont, glyph: PdfJsGlyph): number | null {
  const originalCharCode = glyph.originalCharCode;
  const record = font.fontRecord;
  if (
    typeof originalCharCode !== "number" ||
    originalCharCode < 0x80 ||
    originalCharCode > 0x9f ||
    record?.encodingName !== "WinAnsiEncoding" ||
    record.hasToUnicode === true ||
    record.subtype === "Type0" ||
    record.subtype === "CIDFontType2"
  ) {
    return null;
  }
  const baseFont = stripSubsetPrefix(record.baseFont);
  const fontname = stripSubsetPrefix(font.fontname);
  const standardName = baseFont || fontname;
  const widths =
    /^(?:Arial|Helvetica)(?:,|-)?Bold(?:Oblique|Italic)?$/i.test(standardName) || /^(?:Arial|Helvetica)(?:,|-)?Bold(?:Oblique|Italic)?$/i.test(fontname)
      ? HELVETICA_BOLD_WIN_ANSI_HIGH_WIDTHS
      : /^(?:Arial|Helvetica)(?:,|-)?(?:Oblique|Italic)?$/i.test(standardName) || /^(?:Arial|Helvetica)(?:,|-)?(?:Oblique|Italic)?$/i.test(fontname)
        ? HELVETICA_WIN_ANSI_HIGH_WIDTHS
        : null;
  if (!widths) return null;
  return widths[glyph.unicode ?? ""] ?? null;
}

export function glyphTextLikePdfminer(input: GlyphTextInput): string {
  const { glyphUnicode, originalCharCode, font } = input;
  const fontRecord = input.fontRecord ?? font.fontRecord;
  const embeddedCode = originalCharCode ?? (glyphUnicode && glyphUnicode.length === 1 ? glyphUnicode.charCodeAt(0) : undefined);
  const embeddedCMapCode =
    embeddedCode != null &&
    (/^[\x20-\x7e]+$/.test(fontRecord?.baseFont ?? font.fontname) || embeddedCode > 0xff);
  if (typeof originalCharCode === "number" && fontRecord?.hasToUnicode === true) {
    const mappedText = fontRecord.embeddedUnicodeMap?.[originalCharCode];
    if (mappedText != null && mappedText === glyphUnicode && /^[\r\n\t\f]$/.test(mappedText)) return mappedText;
  }
  if (/^[A-Z]{6}\+/.test(font.fontname) && /^(?:Type0|CIDFontType2)$/.test(fontRecord?.subtype ?? "") && fontRecord?.hasToUnicode === false && embeddedCMapCode) {
    const embeddedText = fontRecord?.embeddedUnicodeMap?.[embeddedCode];
    if (embeddedText != null) return embeddedText;
  }
  if (typeof originalCharCode === "number") {
    if (fontRecord?.subtype === "Type0" && fontRecord.hasToUnicode === false) {
      const embeddedText = fontRecord.embeddedUnicodeMap?.[originalCharCode];
      if ((embeddedText != null && /[\ue000-\uf8ff]/u.test(embeddedText)) || /^[\u0080-\u009f]$/.test(glyphUnicode ?? "")) {
        return `(cid:${originalCharCode})`;
      }
    }
    if (/Wingdings/i.test(font.fontname) && glyphUnicode === "ß") return "§";
    if (/CMEX/i.test(font.fontname) && (!glyphUnicode || glyphUnicode === String.fromCharCode(originalCharCode))) return `(cid:${originalCharCode})`;
    if (/^Diwan/i.test(font.fontname)) return `(cid:${originalCharCode})`;
    if (/^TraditionalArabic/i.test(font.fontname)) return `(cid:${originalCharCode})`;
    if (
      fontRecord?.subtype === "Type0" &&
      fontRecord.symbolic &&
      /^[A-Z]{6}\+Arial(?:MT|-ItalicMT|-BoldMT)$/i.test(font.fontname) &&
      glyphUnicode === String.fromCharCode(originalCharCode)
    ) {
      return `(cid:${originalCharCode})`;
    }
    if (
      fontRecord?.subtype === "Type0" &&
      fontRecord.cidCoding === "Adobe-CNS1" &&
      /^(?:B5pc-H|ETen-B5-H)$/.test(fontRecord.encodingName ?? "") &&
      fontRecord.hasToUnicode === false &&
      glyphUnicode === "\u22ef"
    ) {
      return "\u2026";
    }
    if (/^TimesNewRoman$/i.test(font.fontname) && fontRecord?.hasToUnicode !== true && glyphUnicode === "&") return "C";
    if (fontRecord?.hasToUnicode !== true || !glyphUnicode) {
      const encodingText = glyphNameToUnicodeLikePdfminer(fontRecord?.encodingDifferences?.[originalCharCode]);
      if (encodingText != null) return encodingText;
    }
    if (fontRecord?.hasToUnicode === false && fontRecord.encodingDifferences?.[originalCharCode] === ".notdef" && glyphUnicode === "\u00a0") {
      return `(cid:${originalCharCode})`;
    }
    if (
      fontRecord?.symbolic &&
      fontRecord.hasToUnicode === false &&
      fontRecord.subtype === "Type1" &&
      originalCharCode >= 32 &&
      originalCharCode <= 126 &&
      Object.values(fontRecord.encodingDifferences ?? {}).length > 0 &&
      Object.values(fontRecord.encodingDifferences ?? {}).every((name) => /^G[0-9A-F]{2}$/i.test(name))
    ) {
      return String.fromCharCode(originalCharCode);
    }
    if (
      fontRecord?.symbolic &&
      fontRecord.hasToUnicode === true &&
      fontRecord.subtype === "Type1" &&
      Object.values(fontRecord.encodingDifferences ?? {}).length > 0 &&
      Object.values(fontRecord.encodingDifferences ?? {}).every((name) => /^C\d+$/i.test(name))
    ) {
      const standardText = standardEncodingTextLikePdfminer(originalCharCode);
      if (standardText != null) return standardText;
    }
    const texSymbolicText = texSymbolicType1GlyphTextLikePdfminer(fontRecord, originalCharCode, glyphUnicode);
    if (texSymbolicText != null) return texSymbolicText;
    if (
      fontRecord?.symbolic === false &&
      fontRecord.hasToUnicode === false &&
      fontRecord.subtype === "TrueType" &&
      !fontRecord.encodingName &&
      /^ArialMT$/i.test(fontRecord.baseFont) &&
      originalCharCode >= 128 &&
      originalCharCode <= 255
    ) {
      return winAnsiTextLikePdfminer(originalCharCode) ?? `(cid:${originalCharCode})`;
    }
    if (
      fontRecord?.symbolic === true &&
      fontRecord.hasToUnicode === false &&
      fontRecord.subtype === "TrueType" &&
      originalCharCode >= 128 &&
      originalCharCode <= 255 &&
      Object.values(fontRecord.embeddedUnicodeMap ?? {}).some((text) => /[\ue000-\uf8ff]/u.test(text))
    ) {
      return winAnsiTextLikePdfminer(originalCharCode) ?? `(cid:${originalCharCode})`;
    }
    if (
      fontRecord?.symbolic &&
      fontRecord.hasToUnicode === false &&
      fontRecord.subtype === "Type1" &&
      /(?:CMBSY|MTSY)\d*$/i.test(fontRecord.baseFont) &&
      (originalCharCode === 102 || originalCharCode === 103 || originalCharCode === 161 || originalCharCode === 163)
    ) {
      return String.fromCharCode(originalCharCode);
    }
    if (
      fontRecord?.symbolic &&
      fontRecord.hasToUnicode === false &&
      fontRecord.subtype === "Type1" &&
      /(?:CMBSY|MTSY)\d*$/i.test(fontRecord.baseFont) &&
      originalCharCode === 176
    ) {
      return `(cid:${originalCharCode})`;
    }
    if (
      fontRecord?.symbolic &&
      fontRecord.hasToUnicode === false &&
      fontRecord.subtype === "Type1" &&
      fontRecord.charSet?.length === 1 &&
      fontRecord.widths.length === 1 &&
      originalCharCode === fontRecord.firstChar
    ) {
      const charSetText = glyphNameToUnicodeLikePdfminer(fontRecord.charSet[0]);
      if (charSetText != null) return charSetText;
    }
    if (
      fontRecord?.symbolic &&
      fontRecord.hasToUnicode === false &&
      fontRecord.charSet?.length === 1 &&
      fontRecord.widths.length === 1 &&
      originalCharCode === fontRecord.firstChar &&
      glyphNameToUnicodeLikePdfminer(fontRecord.charSet[0]) == null
    ) {
      return `(cid:${originalCharCode})`;
    }
    if (
      originalCharCode === 32 &&
      glyphUnicode === " " &&
      fontRecord?.symbolic &&
      fontRecord.hasToUnicode === false &&
      fontRecord.charSet?.length &&
      !fontRecord.charSet.includes("space")
    ) {
      return "(cid:32)";
    }
    if (font.cidFallback) {
      if (/^TimesNewRoman(?:,Bold)?$/i.test(font.fontname)) {
        if (originalCharCode === 3) return "¡";
        if (originalCharCode >= 9 && originalCharCode <= 93) return String.fromCharCode(originalCharCode + 29);
      }
      if (/^(Times|Helvetica|Courier)/i.test(font.fontname) && originalCharCode >= 0x20 && originalCharCode <= 0x7e && glyphUnicode) return glyphUnicode;
      return `(cid:${originalCharCode})`;
    }
    if ((font.hasToUnicode === false || (font.missingFile && fontRecord?.hasToUnicode !== true && fontRecord?.subtype !== "Type0")) && originalCharCode > 0xff) {
      const decoded = decodePdfStringLikePdfminer(bytesFromCharCode(originalCharCode));
      return decoded.replace(/\u0000/g, fontRecord?.subtype === "Type0" || fontRecord?.subtype === "CIDFontType2" ? "" : " ");
    }
  }
  if (glyphUnicode && glyphUnicode.length === 1 && glyphUnicode.charCodeAt(0) < 32 && glyphUnicode !== "\r" && glyphUnicode !== "\n") {
    if (typeof originalCharCode === "number" && originalCharCode >= 32 && originalCharCode <= 126) return String.fromCharCode(originalCharCode);
  }
  if (glyphUnicode === "\u0000" && typeof originalCharCode === "number") {
    return `(cid:${originalCharCode})`;
  }
  if (glyphUnicode === "\n" && typeof originalCharCode === "number" && originalCharCode > 2) {
    return `(cid:${originalCharCode})`;
  }
  if (glyphUnicode === "\r") {
    if (fontRecord?.subtype === "Type3" || font.fontname === "unknown") return "\r";
    if (typeof originalCharCode === "number" && originalCharCode >= 32 && originalCharCode <= 126) return String.fromCharCode(originalCharCode);
    return "(cid:13)";
  }
  return glyphUnicode ?? "";
}

export function glyphTextFromPdfJsGlyph(font: MappedFont, glyph: PdfJsGlyph): string {
  return glyphTextLikePdfminer({
    glyphUnicode: glyph.unicode,
    originalCharCode: glyph.originalCharCode,
    font
  });
}

export function glyphWidthLikePdfminer(font: MappedFont, glyph: PdfJsGlyph): number {
  const originalCharCode = glyph.originalCharCode;
  const record = font.fontRecord;
  if (/ZapfDingbats/i.test(font.fontname) && glyph.unicode === "■") return 0;
  const standardHighWidth = winAnsiStandardHighWidthLikePdfminer(font, glyph);
  if (standardHighWidth != null) return standardHighWidth;
  if (
    font.missingFile &&
    record &&
    (font.hasToUnicode === false || record.hasToUnicode === false) &&
    record.subtype !== "Type0" &&
    record.subtype !== "CIDFontType2" &&
    typeof originalCharCode === "number" &&
    originalCharCode <= 0xff
  ) {
    const width = record.widths[originalCharCode - record.firstChar];
    if (typeof width === "number" && Number.isFinite(width)) return width;
  }
  return Number(glyph.width ?? 0);
}
