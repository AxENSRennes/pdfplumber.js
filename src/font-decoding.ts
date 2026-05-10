import { decodePdfStringLikePdfminer } from "./pdf-strings.js";
import type { FontRecord, MappedFont, PdfJsGlyph } from "./types.js";

export interface GlyphTextInput {
  glyphUnicode?: string;
  originalCharCode?: number;
  font: MappedFont;
  fontRecord?: FontRecord;
}

function glyphNameText(name: string | undefined): string | null {
  if (!name) return null;
  const base = name.split(".")[0];
  if (base.length === 1) return base;
  const names: Record<string, string> = {
    space: " ",
    hyphen: "-",
    minus: "-",
    period: ".",
    comma: ",",
    colon: ":",
    semicolon: ";",
    slash: "/",
    backslash: "\\",
    parenleft: "(",
    parenright: ")",
    bracketleft: "[",
    bracketright: "]",
    braceleft: "{",
    braceright: "}",
    quoteright: "\u2019",
    quoteleft: "\u2018",
    quotedblleft: "\u201c",
    quotedblright: "\u201d",
    ampersand: "&",
    asterisk: "*",
    at: "@",
    dollar: "$",
    percent: "%",
    plus: "+",
    underscore: "_",
    zero: "0",
    one: "1",
    two: "2",
    three: "3",
    four: "4",
    five: "5",
    six: "6",
    seven: "7",
    eight: "8",
    nine: "9"
  };
  return names[base] ?? null;
}

function bytesFromCharCode(code: number): number[] {
  if (code <= 0xff) return [code];
  return [(code >> 8) & 0xff, code & 0xff];
}

export function glyphTextLikePdfminer(input: GlyphTextInput): string {
  const { glyphUnicode, originalCharCode, font } = input;
  const fontRecord = input.fontRecord ?? font.fontRecord;
  if (typeof originalCharCode === "number") {
    if (fontRecord?.hasToUnicode !== true || !glyphUnicode) {
      const encodingText = glyphNameText(fontRecord?.encodingDifferences?.[originalCharCode]);
      if (encodingText != null) return encodingText;
    }
    if (font.cidFallback) {
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
  if (glyphUnicode === "\r") return "(cid:13)";
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
