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
  at: "@",
  backslash: "\\",
  braceleft: "{",
  braceright: "}",
  bracketleft: "[",
  bracketright: "]",
  colon: ":",
  comma: ",",
  dollar: "$",
  eight: "8",
  five: "5",
  four: "4",
  hyphen: "-",
  Lcommaaccent: "\u013b",
  minus: "-",
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

export function glyphTextLikePdfminer(input: GlyphTextInput): string {
  const { glyphUnicode, originalCharCode, font } = input;
  const fontRecord = input.fontRecord ?? font.fontRecord;
  if (typeof originalCharCode === "number") {
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
    if (/^TimesNewRoman$/i.test(font.fontname) && fontRecord?.hasToUnicode !== true && glyphUnicode === "&") return "C";
    if (fontRecord?.hasToUnicode !== true || !glyphUnicode) {
      const encodingText = glyphNameToUnicodeLikePdfminer(fontRecord?.encodingDifferences?.[originalCharCode]);
      if (encodingText != null) return encodingText;
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
  if (glyphUnicode === "\r") {
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
