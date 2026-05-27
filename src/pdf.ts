import * as pdfjs from "pdfjs-dist/legacy/build/pdf.mjs";

import { DEFAULT_FONT_ASCENT, DEFAULT_FONT_DESCENT, FONT_UNITS_PER_EM, METADATA_KEYS, STANDARD_FONT_METRICS } from "./constants.js";
import { glyphTextFromPdfJsGlyph, glyphWidthLikePdfminer } from "./font-decoding.js";
import { collectGraphicsHintsFromContent } from "./pdf/content.js";
import { parsePdfObjectsCompat } from "./pdf/document.js";
import { decodePdfName as decodePdfNamePrimitive } from "./pdf/primitives.js";
import {
  extractPageContent as extractPageContentStructured,
  parseColorSpaceResources as parseColorSpaceResourcesStructured,
  parseFontRecords as parseFontRecordsStructured,
  parseImageResources as parseImageResourcesStructured,
  parsePageFontObjectNumbers as parsePageFontObjectNumbersStructured
} from "./pdf/resources.js";
import { decodePdfStreamText } from "./pdf/streams.js";
import { pdfNumbersFromString } from "./pdf/tokenizer.js";
import { decodePdfLiteralBytesAsUtf8ThenUtf16, decodePdfStringLikePdfminer, parsePdfDictBytes, parsePdfLiteralStringBytes } from "./pdf-strings.js";
import type {
  BBox,
  ColorOp,
  FontRecord,
  GraphicsState,
  ImageResource,
  MappedFont,
  Matrix,
  MutableBBox,
  OpenOptions,
  PageBoxes,
  ParsedPath,
  PDFObject,
  PdfJsGlyph,
  Point
} from "./types.js";
import {
  applyMatrix,
  bboxFromPoints,
  cleanBBox,
  cleanMatrix,
  cleanNumber,
  cloneMatrix,
  colorSpaceName,
  colorValue,
  firstFinite,
  graphicColorValue,
  lineWidthScale,
  matrixToPageMatrix,
  multiplyMatrix,
  pointToPageCoords,
  pythonBytesName,
  rectFromPdfBBox,
  rgbColor,
  snapPdfOperand,
  snapPdfPathCoordinate,
  snapPdfPathExtent,
  snapPathOperandPoint,
  softenHalfMicro
} from "./utils.js";

export function parsePdfObjects(raw: string, password = ""): Map<number, string> {
  return parsePdfObjectsCompat(raw, password);
}

export function decodePdfStream(objectText: string | undefined): string {
  return decodePdfStreamText(objectText);
}

export function extractPageContent(pageObjectText: string | undefined, objects: Map<number, string>): string {
  return extractPageContentStructured(pageObjectText, objects);
}

export function parseImageResources(pageObjectText: string | undefined, objects: Map<number, string>, pageContent = ""): ImageResource[] {
  return parseImageResourcesStructured(pageObjectText, objects, pageContent);
}

export function parseColorSpaceResources(pageObjectText: string | undefined, objects: Map<number, string>): Record<string, string> {
  return parseColorSpaceResourcesStructured(pageObjectText, objects);
}

export function parseColorOps(content: string, colorSpaces: Record<string, string> = {}): ColorOp[] {
  return collectGraphicsHintsFromContent(content, colorSpaces).colorOps;
}

export function parseTextMatrixOps(content: string): Matrix[] {
  return collectGraphicsHintsFromContent(content).textMatrices;
}

export function parseTextMoveOps(content: string): { move: Point[]; leadingMove: Point[] } {
  const hints = collectGraphicsHintsFromContent(content);
  return { move: hints.textMoves, leadingMove: hints.leadingTextMoves };
}

export function parseTransformOps(content: string): Matrix[] {
  return collectGraphicsHintsFromContent(content).transforms;
}

export function parsePathOps(content: string): number[][] {
  return collectGraphicsHintsFromContent(content).pathOps;
}

export function parseNumbers(value: string): number[] {
  return pdfNumbersFromString(value);
}

export function decodePdfName(value: string): string {
  return decodePdfNamePrimitive(value);
}

export function decodePdfNameUtf8(value: string): string {
  const decoded = decodePdfName(value);
  if (!/[^\x00-\x7f]/.test(decoded)) return decoded;
  try {
    const bytes = Uint8Array.from([...decoded].map((char) => char.charCodeAt(0) & 0xff));
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return decoded;
  }
}

function extractRawBox(objectText: string | undefined, name: string): number[] | null {
  if (!objectText) return null;
  const match = objectText.match(new RegExp(`/${name}\\s*\\[([^\\]]+)\\]`));
  if (!match) return null;
  const numbers = parseNumbers(match[1]);
  return numbers.length >= 4 ? numbers.slice(0, 4) : null;
}

function boxToPdfplumber(box: readonly number[], mediaHeight: number): BBox {
  return cleanBBox([box[0], mediaHeight - box[3], box[2], mediaHeight - box[1]]);
}

function normalizeRawBox(box: readonly number[]): BBox {
  return [Math.min(box[0], box[2]), Math.min(box[1], box[3]), Math.max(box[0], box[2]), Math.max(box[1], box[3])];
}

export function resolvePageBoxes(pdfPage: any, pageObjectText: string | undefined): PageBoxes {
  const rawMediaOriginal = extractRawBox(pageObjectText, "MediaBox") ?? [...pdfPage.view];
  const rawMedia = normalizeRawBox(rawMediaOriginal);
  const rotate = ((pdfPage.rotate ?? 0) % 360 + 360) % 360;
  const contentYOffset = rawMediaOriginal[3] < rawMediaOriginal[1] ? -(rawMedia[3] - rawMedia[1]) : 0;
  const rawCrop = normalizeRawBox(extractRawBox(pageObjectText, "CropBox") ?? rawMedia);
  if (rotate === 90 || rotate === 270) {
    const rawWidth = rawMedia[2] - rawMedia[0];
    const rawHeight = rawMedia[3] - rawMedia[1];
    const width = rawHeight;
    const height = rawWidth;
    const box = cleanBBox([0, 0, width, height]);
    const rotatedBox = (rawBox: BBox): BBox => {
      if (rotate === 90) {
        return cleanBBox([rawBox[1] - rawMedia[1], rawBox[0] - rawMedia[0], rawBox[3] - rawMedia[1], rawBox[2] - rawMedia[0]]);
      }
      return cleanBBox([rawBox[1] - rawMedia[1], rawMedia[2] - rawBox[2], rawBox[3] - rawMedia[1], rawMedia[2] - rawBox[0]]);
    };
    return { bbox: box, mediabox: box, cropbox: rotatedBox(rawCrop), width: cleanNumber(width), height: cleanNumber(height), rotate, contentYOffset };
  }
  const mediaHeight = rawMedia[3] - rawMedia[1];
  const mediabox = boxToPdfplumber(rawMedia, mediaHeight);
  const cropbox = boxToPdfplumber(rawCrop, mediaHeight);
  const art = extractRawBox(pageObjectText, "ArtBox");
  const bleed = extractRawBox(pageObjectText, "BleedBox");
  const trim = extractRawBox(pageObjectText, "TrimBox");
  return {
    bbox: mediabox,
    mediabox,
    cropbox,
    artbox: art ? boxToPdfplumber(normalizeRawBox(art), mediaHeight) : undefined,
    bleedbox: bleed ? boxToPdfplumber(normalizeRawBox(bleed), mediaHeight) : undefined,
    trimbox: trim ? boxToPdfplumber(normalizeRawBox(trim), mediaHeight) : undefined,
    width: cleanNumber(mediabox[2] - mediabox[0]),
    height: cleanNumber(mediabox[3] - mediabox[1]),
    rotate,
    contentYOffset
  };
}

export function parseFontRecords(objects: Map<number, string>): FontRecord[] {
  return parseFontRecordsStructured(objects);
}

export function parsePageFontObjectNumbers(pageObjectText: string | undefined): number[] {
  return parsePageFontObjectNumbersStructured(pageObjectText);
}

export function parseInfoMetadata(raw: string, objects: Map<number, string>): Record<string, unknown> {
  const infoRefs = Array.from(raw.matchAll(/\/Info\s+(\d+)\s+\d+\s+R/g));
  const infoRef = infoRefs.at(-1)?.[1];
  const text = infoRef ? objects.get(Number(infoRef)) : undefined;
  if (!text) return {};
  const metadata: Record<string, unknown> = {};
  for (const key of METADATA_KEYS) {
    const indirectRef = text.match(new RegExp(`/${key}\\s+(\\d+)\\s+\\d+\\s+R\\b`))?.[1];
    const indirectText = indirectRef ? objects.get(Number(indirectRef)) : undefined;
    const indirectString = indirectText ? readFirstPdfString(indirectText) : null;
    if (indirectString != null) {
      metadata[key] = indirectString;
      continue;
    }
    const string = readPdfLiteralString(text, key);
    const hexString = readPdfHexString(text, key);
    if (string != null) metadata[key] = string;
    else if (hexString != null) metadata[key] = hexString;
    else {
      const number = text.match(new RegExp(`/${key}\\s+([-+]?(?:\\d+\\.\\d+|\\d+|\\.\\d+))\\b`))?.[1];
      if (number != null) metadata[key] = Number(number);
    }
  }
  return metadata;
}

function decodePdfStringBytes(bytes: number[]): string {
  if (bytes[0] === 0xff && bytes[1] === 0xfe) {
    let out = "";
    for (let i = 2; i < bytes.length - 1; i += 2) out += String.fromCharCode(bytes[i] | (bytes[i + 1] << 8));
    return out;
  }
  return decodePdfStringLikePdfminer(bytes);
}

function decodePdfLiteralValue(value: string): string {
  return decodePdfStringBytes([...value].map((char) => char.charCodeAt(0) & 0xff));
}

function settleArabicPresentationOrigin(text: string, value: number): number {
  if (!/[\ufb50-\ufdff\ufe70-\ufeff]/u.test(text) || text === "ﺔ" || !Number.isFinite(value)) return value;
  const scaled = Math.abs(value) * 1_000;
  const fraction = scaled - Math.floor(scaled);
  if (Math.abs(fraction - 0.500501) <= 0.000002) return value - Math.sign(value || 1) * 2e-9;
  if (text !== "ﻟ" && Math.abs(fraction - 0.499499) <= 0.000002) return value + Math.sign(value || 1) * 2e-9;
  return value;
}

function settleCyrillicGlyphEdge(text: string, value: number): number {
  if (!/[\u0400-\u04ff]/u.test(text) || !Number.isFinite(value)) return value;
  const scaled = Math.abs(value) * 1_000;
  const fraction = scaled - Math.floor(scaled);
  if (Math.abs(fraction - 0.499499) <= 0.000002) return value + Math.sign(value || 1) * 1e-6;
  return value;
}

function settleHorizontalGlyphBBoxToMatrixOrigin(font: MappedFont, obj: PDFObject): void {
  const baseFont = font.fontRecord?.baseFont ?? "";
  const isTimesNewRomanCid = /TimesNewRoman/i.test(baseFont) && font.fontRecord?.subtype === "Type0" && !font.fontRecord?.hasToUnicode;
  if (!/TimesNewRoman/i.test(font.fontname) && !isTimesNewRomanCid) return;
  if (font.vertical || !obj.upright || !Array.isArray(obj.matrix)) return;
  const size = Number(obj.size);
  const originY = Number(obj.matrix[5]);
  if (!Number.isFinite(size) || !Number.isFinite(originY) || !Number.isFinite(font.descent)) return;
  const expectedY0Raw = originY + font.descent * size;
  const expectedY1Raw = expectedY0Raw + size;
  const expectedY0 = cleanNumber(expectedY0Raw);
  const expectedY1 = cleanNumber(expectedY1Raw);
  const oldY0 = Number(obj.y0);
  const oldY1 = Number(obj.y1);
  if (Math.abs(oldY0 - expectedY0) > 0.00002 || Math.abs(oldY1 - expectedY1) > 0.00002) return;
  const y0Delta = expectedY0 - oldY0;
  const y1Delta = expectedY1 - oldY1;
  const rawTop = Number(obj.top) - (expectedY1Raw - oldY1);
  const rawBottom = Number(obj.bottom) - (expectedY0Raw - oldY0);
  obj.y0 = expectedY0;
  obj.y1 = expectedY1;
  obj.top = cleanNumber(Number(obj.top) - y1Delta);
  obj.bottom = cleanNumber(Number(obj.bottom) - y0Delta);
  obj.doctop = cleanNumber(Number(obj.doctop) - y1Delta);
  Object.defineProperty(obj, "__pdfplumber_raw_bbox", {
    value: [Number(obj.x0), rawTop, Number(obj.x1), rawBottom],
    enumerable: false,
    configurable: true
  });
}

function readPdfLiteralString(text: string, key: string): string | null {
  const start = text.search(new RegExp(`/${key}\\s*\\(`));
  if (start < 0) return null;
  return readPdfLiteralStringAt(text, text.indexOf("(", start));
}

function readPdfLiteralStringAt(text: string, start: number): string | null {
  if (start < 0) return null;
  const parsed = parsePdfLiteralStringBytes(text, start);
  return parsed ? decodePdfStringBytes(Array.from(parsed.bytes)) : null;
}

function readPdfHexString(text: string, key: string): string | null {
  const match = text.match(new RegExp(`/${key}\\s*<([0-9A-Fa-f\\s]*)>`));
  return match ? decodePdfHexValue(match[1]) : null;
}

function decodePdfHexValue(value: string): string {
  const hex = value.replace(/\s+/g, "");
  const bytes: number[] = [];
  for (let i = 0; i < hex.length - 1; i += 2) bytes.push(Number.parseInt(hex.slice(i, i + 2), 16));
  return decodePdfStringBytes(bytes);
}

function readFirstPdfString(text: string): string | null {
  const literal = readPdfLiteralStringAt(text, text.indexOf("("));
  if (literal != null) return literal;
  const hex = text.match(/<([0-9A-Fa-f\s]+)>/)?.[1];
  return hex == null ? null : decodePdfHexValue(hex);
}

function stripSubsetPrefix(name: string): string {
  return name.replace(/^[A-Z]{6}\+/, "");
}

function hasSubsetPrefix(name: string | undefined): boolean {
  return /^[A-Z]{6}\+/.test(name ?? "");
}

function standardFontMetrics(name: string | undefined): typeof STANDARD_FONT_METRICS[string] | undefined {
  if (!name) return undefined;
  return STANDARD_FONT_METRICS[name];
}

function canonicalFontName(name: string | undefined): string | undefined {
  if (!name) return undefined;
  const stripped = stripSubsetPrefix(name);
  return standardFontMetrics(stripped)?.fontName ?? stripped;
}

function pdfminerCjkFontName(name: string | undefined, allowAliases = true): string | undefined {
  if (!name || !/[^\x20-\x7e]/.test(name)) return undefined;
  const bytes = Uint8Array.from([...name].map((char) => char.charCodeAt(0) & 0xff));
  const key = Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
  const aliases: Record<string, string> = {
    cbcecce5: "SimSun,Regular",
    badacce5: "SimHei,Regular",
    bfaccce55f474232333132: "SimKai,Regular",
    b7c2cbce5f474232333132: "SimFang,Regular",
    c1a5cae9: "SimLi,Regular"
  };
  return (allowAliases ? aliases[key] : undefined) ?? pythonBytesName(name);
}

function fontFamilyName(name: string | undefined): string | undefined {
  return canonicalFontName(name)?.replace(/(?:[-,](?:Bold|Italic|Oblique|Roman))*$/i, "");
}

function fontBBoxMetric(fontObj: any, index: 1 | 3): number | undefined {
  return Array.isArray(fontObj?.bbox) && typeof fontObj.bbox[index] === "number" ? Number(fontObj.bbox[index]) / FONT_UNITS_PER_EM : undefined;
}

function widthComparison(record: FontRecord, fontObj: any): { averageDelta: number; matchedWidths: number } | null {
  if (!fontObj?.widths) return null;
  let totalDelta = 0;
  let matchedWidths = 0;
  for (let code = 0; code < 256; code += 1) {
    const raw = record.widths[code - record.firstChar];
    const actual = fontObj.widths[code];
    if (raw != null && actual != null && (raw !== 0 || actual !== 0)) {
      totalDelta += Math.abs(raw - actual);
      matchedWidths += 1;
    }
  }
  return matchedWidths ? { averageDelta: totalDelta / matchedWidths, matchedWidths } : null;
}

function chooseFontRecord(fontRecords: FontRecord[], used: Set<number>, fontObjName: string | undefined, fontObj: any): FontRecord | undefined {
  const matchingName = (record: FontRecord): boolean => {
    const recordStandard = standardFontMetrics(record.baseFont);
    const objectStandard = standardFontMetrics(fontObjName);
    return record.baseFont === fontObjName || (recordStandard != null && objectStandard != null && recordStandard.fontName === objectStandard.fontName);
  };
  const exactMatches = fontRecords.filter(matchingName);
  exactMatches.sort(
    (a, b) =>
      Number(Boolean(b.hasToUnicode)) - Number(Boolean(a.hasToUnicode)) ||
      Number(Boolean(b.ascent != null || b.descent != null)) - Number(Boolean(a.ascent != null || a.descent != null)) ||
      Number(b.subtype === "Type0") - Number(a.subtype === "Type0")
  );
  const exact = exactMatches[0];
  if (exact) {
    const exactBase = stripSubsetPrefix(exact.baseFont);
    const parentType0 = fontRecords.find(
      (record) =>
        record.subtype === "Type0" &&
        (stripSubsetPrefix(record.baseFont) === exactBase || stripSubsetPrefix(record.baseFont).startsWith(`${exactBase}-`)) &&
        (record.hasToUnicode || !exact.hasToUnicode)
    );
    return parentType0
      ? { ...parentType0, widths: exact.widths, ascent: exact.ascent, descent: exact.descent, symbolic: exact.symbolic }
      : exact;
  }

  const available = fontRecords.filter((record) => !used.has(record.objectNumber));
  const scored = available.flatMap((record, index) => {
    const widths = widthComparison(record, fontObj);
    const recordFamily = fontFamilyName(record.baseFont);
    const objectFamily = fontFamilyName(fontObjName);
    const familyRank = !recordFamily || !objectFamily ? 1 : recordFamily === objectFamily ? 0 : 2;
    return widths ? [{ record, familyRank, averageDelta: widths.averageDelta, matchedWidths: widths.matchedWidths, index }] : [];
  });
  scored.sort((a, b) => a.familyRank - b.familyRank || a.averageDelta - b.averageDelta || b.matchedWidths - a.matchedWidths || a.index - b.index);
  return scored[0]?.record;
}

function resolveFontName(best: FontRecord | undefined, fontObjName: string | undefined, fallbackId: string): string {
  if (best?.subtype === "Type3" || fontObjName === "Type3") return "unknown";
  if (/TimesNewRoman/i.test(best?.baseFont ?? fontObjName ?? "") && best?.subtype === "Type0" && !best?.hasToUnicode) {
    return pythonBytesName(fontObjName ?? best?.baseFont ?? fallbackId);
  }
  const standard = standardFontMetrics(best?.baseFont) ?? standardFontMetrics(fontObjName);
  const rawName = fontObjName ?? best?.baseFont;
  const cjkName = pdfminerCjkFontName(rawName, best?.subtype !== "Type0" && best?.subtype !== "CIDFontType2");
  if (cjkName) return cjkName;
  return pythonBytesName(standard?.fontName ?? rawName ?? fallbackId);
}

function resolveFontMetrics(best: FontRecord | undefined, fontObj: any, style: Record<string, any>, fontname: string): Pick<MappedFont, "ascent" | "descent"> {
  if (best?.subtype === "Type3" || fontObj?.name === "Type3") return { ascent: 1, descent: 0 };
  if (/^Diwan/i.test(fontname) && best?.ascent == null && best?.descent == null) return { ascent: 1, descent: 0 };
  if (/^TraditionalArabic/i.test(fontname) && best?.ascent == null && best?.descent == null) return { ascent: 1, descent: 0 };
  if (/TimesNewRoman/i.test(best?.baseFont ?? fontObj?.name ?? "") && best?.subtype === "Type0" && !best?.hasToUnicode && best.ascent == null && best.descent == null) {
    return { ascent: 1, descent: 0 };
  }
  const mayUseStandardMetrics =
    best == null
      ? true
      : (best.subtype === "Type1" || best.subtype === "TrueType") && !hasSubsetPrefix(best.baseFont);
  const standard = mayUseStandardMetrics
    ? standardFontMetrics(best?.baseFont) ?? standardFontMetrics(fontname)
    : undefined;
  const preferEmbeddedMetrics = Boolean(best?.subtype === "Type0" && !best.hasToUnicode && /TimesNewRoman/i.test(best.baseFont));
  const ascent = firstFinite(
    DEFAULT_FONT_ASCENT,
    ...(preferEmbeddedMetrics ? [best?.ascent, standard?.ascent] : [standard?.ascent, best?.ascent]),
    fontObj?.ascent,
    style.ascent,
    fontBBoxMetric(fontObj, 3)
  );
  const descent = firstFinite(
    DEFAULT_FONT_DESCENT,
    ...(preferEmbeddedMetrics ? [best?.descent, standard?.descent] : [standard?.descent, best?.descent]),
    fontObj?.descent,
    style.descent,
    fontBBoxMetric(fontObj, 1)
  );
  return { ascent, descent: descent > 0 ? -descent : descent };
}

function shouldUseCidFallback(record: FontRecord | undefined): boolean {
  if (!record?.symbolic || record.hasToUnicode) return false;
  if (record.subtype !== "Type1" && record.subtype !== "TrueType" && record.subtype !== "CIDFontType2") return false;
  if (standardFontMetrics(stripSubsetPrefix(record.baseFont)) != null) return false;
  if (!record.charSet?.length) return false;
  const cidOnlyGlyphNames = new Set(["check", "checkmark", "summationtext"]);
  return record.charSet.every((name) => cidOnlyGlyphNames.has(name));
}

function mapFonts(pdfPage: any, styles: Record<string, any>, fontRecords: FontRecord[], fontIds: string[]): Map<string, MappedFont> {
  const mapped = new Map<string, MappedFont>();
  const used = new Set<number>();
  for (const id of fontIds) {
    let fontObj: any;
    try {
      fontObj = pdfPage.commonObjs.get(id);
    } catch {
      fontObj = null;
    }
    const fontObjName = typeof fontObj?.name === "string" ? fontObj.name : undefined;
    const best = chooseFontRecord(fontRecords, used, fontObjName, fontObj);
    if (best) used.add(best.objectNumber);
    const style = styles[id] ?? {};
    const fontname = resolveFontName(best, fontObjName, id);
    const metrics = resolveFontMetrics(best, fontObj, style, fontname);
    const scopedRecords = fontRecords.filter((record) => record.pageScoped);
    mapped.set(id, {
      fontname,
      ascent: metrics.ascent,
      descent: metrics.descent,
      fontMatrix0: Number(fontObj?.fontMatrix?.[0] ?? 0.001),
      vertical: Boolean(style.vertical ?? fontObj?.vertical ?? best?.vertical),
      cidFallback:
        shouldUseCidFallback(best) ||
        Boolean(best?.subtype === "CIDFontType2" && !best?.hasToUnicode) ||
        Boolean(
          fontObj?.composite &&
            !fontObj?.missingFile &&
            /TimesNewRoman/i.test(fontObjName ?? "") &&
            scopedRecords.length > 0 &&
            scopedRecords.every((record) => !record.hasToUnicode)
        ),
      fontRecord: best,
      hasToUnicode: Boolean(fontObj?.toUnicode) || Boolean(best?.hasToUnicode),
      missingFile: Boolean(fontObj?.missingFile)
    });
  }
  return mapped;
}

function cloneState(state: GraphicsState): GraphicsState {
  return { ...state, ctm: cloneMatrix(state.ctm), textMatrix: cloneMatrix(state.textMatrix) };
}

function initialState(): GraphicsState {
  return {
    ctm: [1, 0, 0, 1, 0, 0],
    fillColor: "#000000",
    strokeColor: "#000000",
    fillColorSpace: "DeviceGray",
    strokeColorSpace: "DeviceGray",
    lineWidth: 1,
    lineWidthSet: false,
    dash: null,
    charSpacing: 0,
    wordSpacing: 0,
    textHScale: 1,
    leading: 0,
    textRise: 0,
    fontId: null,
    fontSize: 1,
    fontDirection: 1,
    textMatrix: [1, 0, 0, 1, 0, 0],
    x: 0,
    y: 0,
    lineX: 0,
    lineY: 0
  };
}

function isGlyph(value: unknown): value is PdfJsGlyph {
  return typeof value === "object" && value !== null && "width" in value;
}

function parseDrawPath(data: ArrayLike<number>): ParsedPath[] {
  const paths: ParsedPath[] = [];
  let current: ParsedPath | null = null;
  let start: Point | null = null;
  for (let i = 0; i < data.length;) {
    const op = data[i++];
    if (op === 0) {
      const point: Point = [Number(data[i++]), Number(data[i++])];
      current = { points: [point], closed: false, hasCurve: false, lastOp: op, trailingMove: paths.length > 0 };
      start = point;
      paths.push(current);
    } else if (op === 1) {
      current?.points.push([Number(data[i++]), Number(data[i++])]);
      if (current) current.lastOp = op;
    } else if (op === 2) {
      i += 4;
      current?.points.push([Number(data[i++]), Number(data[i++])]);
      if (current) {
        current.hasCurve = true;
        current.lastOp = op;
      }
    } else if (op === 3) {
      i += 2;
      current?.points.push([Number(data[i++]), Number(data[i++])]);
      if (current) {
        current.hasCurve = true;
        current.lastOp = op;
      }
    } else if (op === 4) {
      if (current) {
        current.explicitClose = true;
        current.closed = true;
        const last = current.points[current.points.length - 1];
        if (start && last) {
          if (last[0] !== start[0] || last[1] !== start[1]) {
            current.points.push(start);
          } else if (current.lastOp !== 1) {
            current.points.push(start);
          }
        }
        current.lastOp = op;
      }
    } else if (op === 5) {
      const x = Number(data[i++]);
      const y = Number(data[i++]);
      const width = Number(data[i++]);
      const height = Number(data[i++]);
      const point: Point = [x, y];
      current = {
        points: [point, [x + width, y], [x + width, y + height], [x, y + height], point],
        closed: true,
        hasCurve: false,
        lastOp: op,
        fromRect: true
      };
      start = point;
      paths.push(current);
    } else {
      break;
    }
  }
  return paths;
}

function isAxisAlignedRect(path: ParsedPath, inferFromGeometry = false): boolean {
  if (path.forceCurve) return false;
  if (path.hasCurve) return false;
  if (path.fromRect && path.explicitClose) return false;
  if (path.fromRect) return true;
  if (!inferFromGeometry) return false;
  const pts = [...path.points];
  const first = pts[0];
  const last = pts[pts.length - 1];
  const geometricallyClosed = path.closed || (first != null && last != null && cleanNumber(first[0]) === cleanNumber(last[0]) && cleanNumber(first[1]) === cleanNumber(last[1]));
  if (geometricallyClosed) {
    while (pts.length > 1) {
      const firstPoint = pts[0];
      const lastPoint = pts[pts.length - 1];
      if (cleanNumber(firstPoint[0]) !== cleanNumber(lastPoint[0]) || cleanNumber(firstPoint[1]) !== cleanNumber(lastPoint[1])) break;
      pts.pop();
    }
  }
  if (!geometricallyClosed || pts.length !== 4) return false;
  const [[x0, y0], [x1, y1], [x2, y2], [x3, y3]] = pts.map((point) => [cleanNumber(point[0]), cleanNumber(point[1])] as Point);
  return (x0 === x1 && y1 === y2 && x2 === x3 && y3 === y0) || (y0 === y1 && x1 === x2 && y2 === y3 && x3 === x0);
}

function pathSignature(data: ArrayLike<number>): string {
  const ops: number[] = [];
  for (let i = 0; i < data.length;) {
    const op = Number(data[i++]);
    ops.push(op);
    if (op === 0 || op === 1) i += 2;
    else if (op === 2) i += 6;
    else if (op === 3 || op === 5) i += 4;
    else if (op !== 4) break;
  }
  return ops.join(",");
}

function pathOpsCompatible(candidate: number[], fallback: ArrayLike<number>): boolean {
  return candidate.length === fallback.length && pathSignature(candidate) === pathSignature(fallback);
}

function pathHasCurveOps(data: ArrayLike<number>): boolean {
  for (let i = 0; i < data.length;) {
    const op = Number(data[i++]);
    if (op === 2 || op === 3) return true;
    if (op === 0 || op === 1) i += 2;
    else if (op === 2) i += 6;
    else if (op === 3 || op === 5) i += 4;
    else if (op !== 4) break;
  }
  return false;
}

function collinearPathEndpoints(points: Point[]): [Point, Point] | null {
  const unique: Point[] = [];
  for (const point of points) {
    if (!unique.some((other) => cleanNumber(other[0]) === cleanNumber(point[0]) && cleanNumber(other[1]) === cleanNumber(point[1]))) unique.push(point);
  }
  if (unique.length !== 2) return null;
  return [unique[0], unique[1]];
}

export function extractPageObjects(
  pdfPage: any,
  operatorList: { fnArray: number[]; argsArray: unknown[] },
  textStyles: Record<string, any>,
  fontRecords: FontRecord[],
  pageNumber: number,
  pageWidth: number,
  pageHeight: number,
  pageRotate: number,
  doctopOffset: number,
  contentYOffset = 0,
  pageX0 = 0,
  pageTop = 0,
  colorOps: ColorOp[] = [],
  transformOps: Matrix[] = [],
  textMatrixOps: Matrix[] = [],
  textMoveOps: Point[] = [],
  textLeadingMoveOps: Point[] = [],
  pathOps: number[][] = [],
  imageResources: ImageResource[] = [],
  unicodeNorm?: OpenOptions["unicode_norm"],
  preferRgbFallback = false
): { chars: PDFObject[]; rects: PDFObject[]; lines: PDFObject[]; curves: PDFObject[]; images: PDFObject[] } {
  const fontIds = Array.from(new Set(operatorList.fnArray.flatMap((fn, i) => (fn === pdfjs.OPS.setFont ? [String((operatorList.argsArray[i] as unknown[])[0])] : []))));
  const fonts = mapFonts(pdfPage, textStyles, fontRecords, fontIds);
  const state = initialState();
  const stack: GraphicsState[] = [];
  const chars: PDFObject[] = [];
  const rects: PDFObject[] = [];
  const lines: PDFObject[] = [];
  const curves: PDFObject[] = [];
  const images: PDFObject[] = [];
  const contentPoint = (point: Point): Point => (contentYOffset ? [point[0], point[1] + contentYOffset] : point);
  const coordOffset: Point = [0, pageTop];
  const markedContentStack: Array<{ tag: string | null; mcid: number | null }> = [];
  const fillColorOps = colorOps.filter((op) => op.target === "fill");
  const strokeColorOps = colorOps.filter((op) => op.target === "stroke");
  let transformIndex = 0;
  let textMatrixIndex = 0;
  let textMoveIndex = 0;
  let textLeadingMoveIndex = 0;
  let pathIndex = 0;
  let imageIndex = 0;
  let sawDeviceCMYK = false;
  const constructPathTotal = operatorList.fnArray.filter((fn) => fn === pdfjs.OPS.constructPath).length;
  const applyFormXObjectMatrices =
    !operatorList.fnArray.some((fn) => fn === pdfjs.OPS.setTextMatrix) &&
    !operatorList.fnArray.some((fn) => fn === pdfjs.OPS.beginGroup);
  const formYExtras = () => (state.formMatrixApplied ? { __pdfplumber_page_y_from_top: true } : {});
  const partialRawPaths = pathOps.length > 0 && pathOps.length !== constructPathTotal;
  let usedPartialRawCurve = false;
  const markedExtras = () => {
    const markedContent =
      [...markedContentStack].reverse().find((item) => item.tag !== "OC") ?? markedContentStack[markedContentStack.length - 1];
    return {
      mcid: markedContent?.mcid ?? null,
      tag: markedContent?.tag ?? null
    };
  };
  const hasFutureDarkFillFallback = (startIndex: number): boolean => {
    for (let j = startIndex + 1; j < operatorList.fnArray.length; j += 1) {
      if (operatorList.fnArray[j] !== pdfjs.OPS.setFillRGBColor) continue;
      const value = (operatorList.argsArray[j] as any[] | null)?.[0];
      if (typeof value === "string" && /^#[0-9a-f]{6}$/i.test(value) && !/^#(?:0{6}|f{6})$/i.test(value)) return true;
    }
    return false;
  };
  const colorFromHint = (fallback: unknown, queue: ColorOp[], currentColorSpace: string, options: { deferBlackRawTint?: boolean } = {}): { color: unknown; colorSpace: string | null; fromHint: boolean } => {
    const hintMatchesExtremeFallback = (hint: ColorOp, target: 0 | 1): boolean => {
      if (hint.pattern || !hint.components.length) return false;
      const tolerance = 1e-5;
      const quantizedFallbackTolerance = 0.002;
      if ((hint.colorSpace === "DeviceCMYK" || hint.colorSpace === "ICCBased") && hint.components.length === 4) {
        const [c, m, y, k] = hint.components;
        return target === 0
          ? Math.abs(c) <= tolerance && Math.abs(m) <= tolerance && Math.abs(y) <= tolerance && k >= 1 - tolerance
          : hint.components.every((component) => Math.abs(component) <= tolerance);
      }
      return hint.components.every((component) => Math.abs(component - target) <= quantizedFallbackTolerance);
    };
    let hint: ColorOp | undefined = queue[0];
    if (typeof fallback === "string" && /^#(?:0{6}|f{6})$/i.test(fallback)) {
      const target = fallback.toLowerCase() === "#000000" ? 0 : 1;
      const rawTintColorSpace = hint && !["DeviceGray", "DeviceRGB", "DeviceCMYK", "ICCBased"].includes(hint.colorSpace);
      if (target === 0 && options.deferBlackRawTint && hint && !hintMatchesExtremeFallback(hint, target)) {
        return { color: rgbColor(fallback), colorSpace: currentColorSpace === "DeviceGray" ? "DeviceGray" : currentColorSpace, fromHint: false };
      }
      if (rawTintColorSpace && !(target === 0 && options.deferBlackRawTint && !hintMatchesExtremeFallback(hint, target))) {
        hint = queue.shift();
      } else {
        const matchingIndex = queue.findIndex((candidate) => hintMatchesExtremeFallback(candidate, target));
        if (matchingIndex >= 0) {
          hint = queue.splice(matchingIndex, 1)[0];
        } else if (hint && !hintMatchesExtremeFallback(hint, target)) {
          return { color: rgbColor(fallback), colorSpace: currentColorSpace === "DeviceGray" ? "DeviceGray" : currentColorSpace, fromHint: false };
        } else {
          hint = queue.shift();
        }
      }
    } else {
      hint = queue.shift();
    }
    if (hint?.pattern) return { color: hint.pattern, colorSpace: "Pattern", fromHint: true };
    if (hint?.components.length === 1) return { color: [cleanNumber(hint.components[0])], colorSpace: hint.colorSpace, fromHint: true };
    if (hint) return { color: hint.components.map(cleanNumber), colorSpace: hint.colorSpace, fromHint: true };
    if (!preferRgbFallback && currentColorSpace === "DeviceGray" && typeof fallback === "string" && /^#([0-9a-f]{2})\1\1$/i.test(fallback)) {
      const component = parseInt(fallback.slice(1, 3), 16);
      return { color: [component === 128 ? 0.5 : cleanNumber(component / 255)], colorSpace: "DeviceGray", fromHint: false };
    }
    if (typeof fallback === "string" && /^#[0-9a-f]{6}$/i.test(fallback)) {
      const preservedColorSpace = ["ICCBased", "Separation"].includes(currentColorSpace) ? currentColorSpace : "DeviceRGB";
      return { color: rgbColor(fallback), colorSpace: preservedColorSpace, fromHint: false };
    }
    return { color: rgbColor(fallback), colorSpace: null, fromHint: false };
  };
  const setColorFromGeneric = (target: "fill" | "stroke", fallback: unknown): void => {
    const queue = target === "fill" ? fillColorOps : strokeColorOps;
    const currentColorSpace = target === "fill" ? state.fillColorSpace : state.strokeColorSpace;
    if (Array.isArray(fallback) && fallback[0] === "TilingPattern") {
      const patternIndex = queue.findIndex((hint) => hint.pattern);
      if (patternIndex >= 0) {
        const [hint] = queue.splice(patternIndex, 1);
        if (target === "fill") {
          state.fillColor = hint.pattern;
          state.fillColorSpace = "Pattern";
        } else {
          state.strokeColor = hint.pattern;
          state.strokeColorSpace = "Pattern";
        }
        return;
      }
    }
    const hinted = colorFromHint(fallback, queue, currentColorSpace);
    if (target === "fill") {
      state.fillColor = hinted.color;
      if (hinted.colorSpace) state.fillColorSpace = hinted.colorSpace;
    } else {
      state.strokeColor = hinted.color;
      if (hinted.colorSpace) state.strokeColorSpace = hinted.colorSpace;
    }
  };

  for (let i = 0; i < operatorList.fnArray.length; i += 1) {
    const fn = operatorList.fnArray[i];
    const args = operatorList.argsArray[i] as any[] | null;
    switch (fn) {
      case pdfjs.OPS.save:
        stack.push(cloneState(state));
        break;
      case pdfjs.OPS.restore: {
        const restored = stack.pop();
        if (restored) Object.assign(state, restored);
        break;
      }
      case pdfjs.OPS.transform:
        state.ctm = multiplyMatrix(state.ctm, transformOps[transformIndex++] ?? (args as Matrix));
        break;
      case pdfjs.OPS.paintFormXObjectBegin:
        if (applyFormXObjectMatrices) {
          stack.push(cloneState(state));
          const formMatrix = Array.from(args?.[0] ?? []).map((value) => Number(value)) as Matrix;
          if (formMatrix.length === 6 && formMatrix.every(Number.isFinite)) {
            state.ctm = multiplyMatrix(state.ctm, formMatrix);
            state.formMatrixApplied = true;
          }
        }
        break;
      case pdfjs.OPS.paintFormXObjectEnd: {
        if (applyFormXObjectMatrices) {
          const restored = stack.pop();
          if (restored) Object.assign(state, restored);
        }
        break;
      }
      case pdfjs.OPS.setLineWidth:
        state.lineWidth = Number(args?.[0] ?? state.lineWidth) * lineWidthScale(state.ctm);
        state.lineWidthSet = true;
        break;
      case pdfjs.OPS.setDash:
        state.dash = args ? [args[0] ?? [], args[1] ?? 0] : null;
        break;
      case pdfjs.OPS.beginMarkedContent:
        markedContentStack.push({ tag: decodePdfNameUtf8(args?.[0]?.name ?? String(args?.[0] ?? "")), mcid: null });
        break;
      case pdfjs.OPS.beginMarkedContentProps:
        markedContentStack.push({ tag: decodePdfNameUtf8(args?.[0]?.name ?? String(args?.[0] ?? "")), mcid: typeof args?.[1] === "number" ? args[1] : null });
        break;
      case pdfjs.OPS.endMarkedContent:
        {
          const ended = markedContentStack.pop();
          if (ended?.mcid == null && ended?.tag != null && ended.tag !== "OC" && markedContentStack.length > 0) {
            markedContentStack.pop();
          }
        }
        break;
      case pdfjs.OPS.setFillColorSpace:
        {
          const nextColorSpace = colorSpaceName(args?.[0]);
          state.fillColorSpace = nextColorSpace === "Indexed" && state.fillColorSpace === "Separation" ? state.fillColorSpace : nextColorSpace;
        }
        break;
      case pdfjs.OPS.setStrokeColorSpace:
        state.strokeColorSpace = colorSpaceName(args?.[0]);
        break;
      case pdfjs.OPS.setFillRGBColor: {
        const hinted = colorFromHint(args?.[0], fillColorOps, state.fillColorSpace, {
          deferBlackRawTint: typeof args?.[0] === "string" && args[0].toLowerCase() === "#000000" && hasFutureDarkFillFallback(i)
        });
        if (
          !hinted.fromHint &&
          (sawDeviceCMYK || state.fillColorSpace === "DeviceCMYK" || state.strokeColorSpace === "DeviceCMYK") &&
          typeof args?.[0] === "string" &&
          args[0].toLowerCase() === "#ffffff"
        ) {
          state.fillColor = [0, 0, 0, 0];
          state.fillColorSpace = "DeviceCMYK";
        } else {
          state.fillColor = hinted.color;
          if (hinted.colorSpace) state.fillColorSpace = hinted.colorSpace;
        }
        if (state.fillColorSpace === "DeviceCMYK") sawDeviceCMYK = true;
        break;
      }
      case pdfjs.OPS.setFillGray:
        state.fillColor = args?.length === 1 ? args[0] : args ?? state.fillColor;
        state.fillColorSpace = "DeviceGray";
        break;
      case pdfjs.OPS.setFillCMYKColor:
        state.fillColorSpace = "DeviceCMYK";
        state.fillColor = args ?? state.fillColor;
        sawDeviceCMYK = true;
        break;
      case pdfjs.OPS.setFillColor:
      case pdfjs.OPS.setFillColorN:
        setColorFromGeneric("fill", args ?? state.fillColor);
        break;
      case pdfjs.OPS.setStrokeRGBColor: {
        const hinted = colorFromHint(args?.[0], strokeColorOps, state.strokeColorSpace);
        if (
          !hinted.fromHint &&
          (sawDeviceCMYK || state.fillColorSpace === "DeviceCMYK" || state.strokeColorSpace === "DeviceCMYK") &&
          typeof args?.[0] === "string" &&
          args[0].toLowerCase() === "#ffffff"
        ) {
          state.strokeColor = [0, 0, 0, 0];
          state.strokeColorSpace = "DeviceCMYK";
        } else {
          state.strokeColor = hinted.color;
          if (hinted.colorSpace) state.strokeColorSpace = hinted.colorSpace;
        }
        if (state.strokeColorSpace === "DeviceCMYK") sawDeviceCMYK = true;
        break;
      }
      case pdfjs.OPS.setStrokeGray:
        state.strokeColor = args?.length === 1 ? args[0] : args ?? state.strokeColor;
        state.strokeColorSpace = "DeviceGray";
        break;
      case pdfjs.OPS.setStrokeCMYKColor:
        state.strokeColorSpace = "DeviceCMYK";
        state.strokeColor = args ?? state.strokeColor;
        sawDeviceCMYK = true;
        break;
      case pdfjs.OPS.setStrokeColor:
      case pdfjs.OPS.setStrokeColorN:
        setColorFromGeneric("stroke", args ?? state.strokeColor);
        break;
      case pdfjs.OPS.setCharSpacing:
        state.charSpacing = Number(args?.[0] ?? 0);
        break;
      case pdfjs.OPS.setWordSpacing:
        state.wordSpacing = Number(args?.[0] ?? 0);
        break;
      case pdfjs.OPS.setHScale:
        state.textHScale = Number(args?.[0] ?? 100) / 100;
        break;
      case pdfjs.OPS.setLeading:
        state.leading = -Number(args?.[0] ?? 0);
        break;
      case pdfjs.OPS.setTextRise:
        state.textRise = Number(args?.[0] ?? 0);
        break;
      case pdfjs.OPS.beginText:
        state.textMatrix = [1, 0, 0, 1, 0, 0];
        state.x = state.lineX = 0;
        state.y = state.lineY = 0;
        break;
      case pdfjs.OPS.setFont: {
        state.fontId = String(args?.[0]);
        const size = Number(args?.[1] ?? 1);
        state.fontSize = Math.abs(size);
        state.fontDirection = size < 0 ? -1 : 1;
        break;
      }
      case pdfjs.OPS.moveText:
        {
          const rawMove = textMoveOps[textMoveIndex++];
          const tx = rawMove?.[0] ?? Number(args?.[0] ?? 0);
          const ty = rawMove?.[1] ?? Number(args?.[1] ?? 0);
          state.x = state.lineX += tx;
          state.y = state.lineY += ty;
        }
        break;
      case pdfjs.OPS.setLeadingMoveText:
        {
          const rawMove = textLeadingMoveOps[textLeadingMoveIndex++];
          const tx = rawMove?.[0] ?? Number(args?.[0] ?? 0);
          const ty = rawMove?.[1] ?? Number(args?.[1] ?? 0);
          state.leading = ty;
          state.x = state.lineX += tx;
          state.y = state.lineY += ty;
        }
        break;
      case pdfjs.OPS.nextLine:
        state.x = state.lineX;
        state.y = state.lineY += state.leading;
        break;
      case pdfjs.OPS.setTextMatrix:
        {
          const rawMatrix = textMatrixOps[textMatrixIndex++];
          state.textMatrix = rawMatrix
            ? cloneMatrix(rawMatrix)
            : (Array.from(args?.[0] ?? args ?? [1, 0, 0, 1, 0, 0]).map((value) => snapPdfOperand(Number(value))) as Matrix);
        }
        state.x = state.lineX = 0;
        state.y = state.lineY = 0;
        break;
      case pdfjs.OPS.showText:
      case pdfjs.OPS.showSpacedText: {
        const glyphs = (args?.[0] ?? []) as unknown[];
        const font = state.fontId ? fonts.get(state.fontId) : undefined;
        if (!font) break;
        const matrix = multiplyMatrix(state.ctm, state.textMatrix);
        const textHScale = state.textHScale * state.fontDirection;
        let x = 0;
        let needCharSpacing = false;
        const spacingDelta = (value: number): number => font.vertical ? -value * state.fontDirection : value * state.fontDirection;
        for (let glyphIndex = 0; glyphIndex < glyphs.length; glyphIndex += 1) {
          const glyph = glyphs[glyphIndex];
          if (typeof glyph === "number") {
            x += (font.vertical ? 1 : -1) * glyph * state.fontSize / 1000;
            needCharSpacing = true;
            continue;
          }
          if (!isGlyph(glyph)) continue;
          if (needCharSpacing) x += spacingDelta(state.charSpacing);
          const rawText = glyphTextFromPdfJsGlyph(font, glyph);
          const text = unicodeNorm ? rawText.normalize(unicodeNorm) : rawText;
          const glyphWidth = glyphWidthLikePdfminer(font, glyph);
          const verticalMetric = Array.isArray(glyph.vmetric) ? (glyph.vmetric as number[]) : null;
          const advance =
            text === "\n" && Number(glyph.originalCharCode ?? 0) <= 2
              ? 0
              : font.vertical && !verticalMetric
                ? state.fontSize
                : glyphWidth * state.fontSize * font.fontMatrix0;
          const verticalX = Number(verticalMetric?.[1] ?? 500) * state.fontSize * font.fontMatrix0;
          const verticalY = Number(verticalMetric?.[2] ?? 880) * state.fontSize * font.fontMatrix0;
          const originalCharCode = Number(glyph.originalCharCode ?? 0);
          const splitChars =
            !font.vertical && font.missingFile && originalCharCode > 0xff && !text.startsWith("(cid:") ? Array.from(text) : [text];
          const metricParts =
            !font.vertical &&
            font.missingFile &&
            ((text === " " && glyphWidth >= 900) ||
              (originalCharCode > 0xff && splitChars.length === 1 && font.fontRecord?.subtype !== "Type0" && font.fontRecord?.subtype !== "CIDFontType2"))
              ? 2
              : splitChars.length;
          for (let partIndex = 0; partIndex < splitChars.length; partIndex += 1) {
            const partText = splitChars[partIndex];
            const partStart = x + (advance * partIndex) / metricParts;
            const partEnd = x + (advance * (partIndex + 1)) / metricParts;
            const xStart = font.vertical ? state.x - verticalX : state.x + partStart * textHScale;
            const xEnd = font.vertical ? xStart + state.fontSize : state.x + partEnd * textHScale;
            const yStart = font.vertical ? state.y + state.textRise - verticalY - partStart : state.y + state.textRise + font.descent * state.fontSize;
            const yEnd = font.vertical ? yStart + state.fontSize : yStart + state.fontSize;
            const rawBBox = bboxFromPoints([
              contentPoint(applyMatrix([xStart, yStart], matrix)),
              contentPoint(applyMatrix([xStart, yEnd], matrix)),
              contentPoint(applyMatrix([xEnd, yStart], matrix)),
              contentPoint(applyMatrix([xEnd, yEnd], matrix))
            ]);
            const rawSize = font.vertical ? Math.abs(rawBBox[2] - rawBBox[0]) : Math.abs(rawBBox[3] - rawBBox[1]);
            const originX = font.vertical ? state.x + x : xStart;
            const originY = state.y + state.textRise;
            const [matrixE, matrixF] = contentPoint(applyMatrix([originX, originY], matrix));
            const glyphMatrix = cleanMatrix(matrixToPageMatrix([matrix[0], matrix[1], matrix[2], matrix[3], matrixE, matrixF], pageWidth, pageHeight, pageRotate));
            const upright =
              pageRotate === 90 || pageRotate === 270
                ? Math.abs(matrix[0]) < 1e-6 && Math.abs(matrix[3]) < 1e-6
                : matrix[0] * matrix[3] * textHScale > 0 && matrix[1] * matrix[2] <= 0;
            glyphMatrix[4] = cleanNumber(softenHalfMicro(glyphMatrix[4] - pageX0));
            glyphMatrix[5] = cleanNumber(softenHalfMicro(glyphMatrix[5] + pageTop));
            const obj = rectFromPdfBBox(rawBBox, pageWidth, pageHeight, pageRotate, pageNumber, "char", doctopOffset, {
              text: partText,
              fontname: font.fontname,
              adv:
                font.vertical && verticalMetric
                  ? cleanNumber(Number(verticalMetric[0]) * state.fontSize * font.fontMatrix0)
                  : cleanNumber(Math.abs((advance * textHScale) / metricParts)),
              upright,
              matrix: glyphMatrix,
              ncs: state.fillColorSpace === "Indexed" && Array.isArray(state.fillColor) && state.fillColor.length === 1 && Number(state.fillColor[0]) === 1 ? "Separation" : state.fillColorSpace,
              non_stroking_color: colorValue(state.fillColor),
              stroking_color: colorValue(state.strokeColor),
              ...formYExtras(),
              ...markedExtras()
            }, coordOffset);
            if (Array.isArray(obj.matrix) && Math.abs(Number(obj.matrix[4]) - Number(obj.x0)) < 0.002) {
              obj.matrix[4] = obj.x0;
            }
            if (font.vertical && Array.isArray(obj.matrix)) {
              obj.matrix[4] = cleanNumber(Number(obj.x0) + Number(obj.width) / 2);
              obj.matrix[5] = cleanNumber(Number(obj.y1) - Number(obj.height) * 0.12);
            }
            const settledX0 = settleArabicPresentationOrigin(partText, Number(obj.x0));
            if (settledX0 !== obj.x0) {
              if (Array.isArray(obj.matrix) && obj.matrix[4] === obj.x0) obj.matrix[4] = settledX0;
              obj.x0 = settledX0;
              obj.width = Number(obj.x1) - settledX0;
            }
            const settledX1 = settleArabicPresentationOrigin(partText, Number(obj.x1));
            if (settledX1 !== obj.x1) {
              obj.x1 = settledX1;
              obj.width = settledX1 - Number(obj.x0);
            }
            const settledCyrillicX1 = settleCyrillicGlyphEdge(partText, Number(obj.x1));
            if (settledCyrillicX1 !== obj.x1) {
              obj.x1 = settledCyrillicX1;
              obj.width = settledCyrillicX1 - Number(obj.x0);
            }
            obj.size = font.vertical ? obj.width : obj.height;
            settleHorizontalGlyphBBoxToMatrixOrigin(font, obj);
            const rawSizeKey = font.vertical ? `${rawSize}:${obj.x0}:${obj.x1}` : `${rawSize}:${obj.y0}:${obj.y1}`;
            Object.defineProperty(obj, "__pdfplumber_raw_size", { value: rawSize, enumerable: false });
            Object.defineProperty(obj, "__pdfplumber_raw_size_key", { value: rawSizeKey, enumerable: false });
            chars.push(obj);
          }
          x += advance;
          if (glyph.isSpace) x += spacingDelta(state.wordSpacing);
          needCharSpacing = true;
        }
        if (font.vertical) state.y -= x;
        else state.x += x * textHScale;
        break;
      }
      case pdfjs.OPS.constructPath: {
        const paintOp = Number(args?.[0]);
        const pdfJsPath = (args?.[1]?.[0] ?? []) as ArrayLike<number>;
        const candidatePath = pathOps[pathIndex];
        const compatibleRawPath = candidatePath && (!partialRawPaths || pathOpsCompatible(candidatePath, pdfJsPath)) ? candidatePath : undefined;
        const compatibleHasCurve = compatibleRawPath ? pathHasCurveOps(compatibleRawPath) : false;
        const rawPath = compatibleRawPath && (!partialRawPaths || (compatibleHasCurve && !usedPartialRawCurve)) ? compatibleRawPath : undefined;
        if (compatibleRawPath) pathIndex += 1;
        if (rawPath && partialRawPaths && compatibleHasCurve) {
          usedPartialRawCurve = true;
          pathIndex = pathOps.length;
        }
        const raw = (rawPath ?? pdfJsPath) as ArrayLike<number>;
        const pathBBox = (points: Point[]): MutableBBox => bboxFromPoints(points);
        const paths = parseDrawPath(raw);
        const isStroke =
          paintOp === pdfjs.OPS.stroke ||
          paintOp === pdfjs.OPS.closeStroke ||
          paintOp === pdfjs.OPS.fillStroke ||
          paintOp === pdfjs.OPS.eoFillStroke ||
          paintOp === pdfjs.OPS.closeFillStroke ||
          paintOp === pdfjs.OPS.closeEOFillStroke;
        const isFill =
          paintOp === pdfjs.OPS.fill ||
          paintOp === pdfjs.OPS.eoFill ||
          paintOp === pdfjs.OPS.fillStroke ||
          paintOp === pdfjs.OPS.eoFillStroke ||
          paintOp === pdfjs.OPS.closeFillStroke ||
          paintOp === pdfjs.OPS.closeEOFillStroke;
        const linewidth = state.lineWidthSet ? state.lineWidth : 0;
        const vectorExtras = {
          linewidth,
          dash: state.dash,
          fill: isFill,
          stroke: isStroke,
          evenodd: paintOp === pdfjs.OPS.eoFill || paintOp === pdfjs.OPS.eoFillStroke || paintOp === pdfjs.OPS.closeEOFillStroke,
          stroking_color: graphicColorValue(state.strokeColor, state.strokeColorSpace),
          non_stroking_color: graphicColorValue(state.fillColor, state.fillColorSpace),
          ...markedExtras()
        };
        const lineExtras = vectorExtras;
        const pathEntries = paths.map((path) => {
          const operandPoints = rawPath ? path.points : path.points.map(snapPathOperandPoint);
          const transformed = operandPoints.map((point) => contentPoint(applyMatrix(point, state.ctm)));
          const pagePoints = transformed.map((point) => pointToPageCoords(point, pageWidth, pageHeight, pageRotate).map(cleanNumber) as Point);
          return { path, transformed, pagePoints };
        });
        const pointKey = (points: Point[]): string | null => {
          const first = points[0];
          if (!first) return null;
          if (!points.every((point) => cleanNumber(point[0]) === cleanNumber(first[0]) && cleanNumber(point[1]) === cleanNumber(first[1]))) return null;
          return `${cleanNumber(first[0])}:${cleanNumber(first[1])}`;
        };
        const closedSinglePointKeys = new Set(
          pathEntries
            .filter(({ path, transformed }) => path.closed && transformed.length > 1)
            .map(({ transformed }) => pointKey(transformed))
            .filter((key): key is string => key != null)
        );
        for (const { path, transformed, pagePoints } of pathEntries) {
          const inferRectFromGeometry = rawPath === undefined;
          if (transformed.length < 2) {
            if (transformed.length === 1 && (isFill || isStroke)) {
              if (!path.closed && closedSinglePointKeys.has(pointKey(transformed) ?? "")) continue;
              const rawBBox = pathBBox(transformed);
              const pts = pagePoints;
              const [pointX, pointTop] = pts[0] ?? [Number.NaN, Number.NaN];
              if (!isStroke && (pointX < 0 || pointX > pageWidth || pointTop < 0 || pointTop > pageHeight)) continue;
              curves.push(
	                rectFromPdfBBox(rawBBox, pageWidth, pageHeight, pageRotate, pageNumber, "curve", doctopOffset, {
	                  pts,
	                  closepath: path.closed,
	                  ...vectorExtras
	                }, coordOffset)
              );
            }
            continue;
          }
          if (isStroke && !isFill) {
            const rawBBox = pathBBox(transformed);
            if (path.closed || path.hasCurve || transformed.length > 2) {
              const lineEndpoints = !path.hasCurve ? collinearPathEndpoints(transformed) : null;
              if (lineEndpoints && vectorExtras.tag !== "P" && transformed.length === 2) {
                const rawLineBBox = pathBBox(lineEndpoints);
                lines.push(rectFromPdfBBox(rawLineBBox, pageWidth, pageHeight, pageRotate, pageNumber, "line", doctopOffset, path.closed ? vectorExtras : lineExtras, coordOffset));
              } else if (
                isAxisAlignedRect({ ...path, points: transformed }, inferRectFromGeometry || (path.closed && transformed.length === 5)) ||
                isAxisAlignedRect({ ...path, points: pagePoints }, inferRectFromGeometry || transformed.length === 5)
              ) {
                rects.push(
                  rectFromPdfBBox(rawBBox, pageWidth, pageHeight, pageRotate, pageNumber, "rect", doctopOffset, {
                    ...vectorExtras,
                    linewidth: isStroke ? linewidth : 0
                  }, coordOffset)
                );
              } else {
                curves.push(
	                  rectFromPdfBBox(rawBBox, pageWidth, pageHeight, pageRotate, pageNumber, "curve", doctopOffset, {
	                    pts: pagePoints,
	                    closepath: path.closed,
	                    ...vectorExtras
	                  }, coordOffset)
                );
              }
            } else {
              for (let p = 0; p < transformed.length - 1; p += 1) {
                const rawLineBBox = pathBBox([transformed[p], transformed[p + 1]]);
                lines.push(rectFromPdfBBox(rawLineBBox, pageWidth, pageHeight, pageRotate, pageNumber, "line", doctopOffset, lineExtras, coordOffset));
              }
            }
          } else if (isFill) {
            const rawBBox = pathBBox(transformed);
            if (isStroke && transformed.length > 1 && rawBBox[0] === rawBBox[2] && rawBBox[1] === rawBBox[3]) {
              const pts = pagePoints;
              const curve = rectFromPdfBBox(rawBBox, pageWidth, pageHeight, pageRotate, pageNumber, "curve", doctopOffset, {
                pts,
                closepath: path.closed,
                ...vectorExtras,
                linewidth
              }, coordOffset);
              if (!curves.some((other) => other.x0 === curve.x0 && other.top === curve.top && other.fill === curve.fill && other.stroke === curve.stroke)) {
                curves.push(curve);
              }
              continue;
            }
            const lineEndpoints = !path.hasCurve ? collinearPathEndpoints(transformed) : null;
            if (lineEndpoints && (transformed.length === 2 || (transformed.length === 3 && path.explicitClose))) {
              const rawLineBBox = pathBBox(lineEndpoints);
              lines.push(rectFromPdfBBox(rawLineBBox, pageWidth, pageHeight, pageRotate, pageNumber, "line", doctopOffset, vectorExtras, coordOffset));
            } else if (
              !(rawPath && path.fromRect && vectorExtras.tag === "Figure" && linewidth > 0 && !isStroke && typeof vectorExtras.non_stroking_color === "number") &&
              isAxisAlignedRect({ ...path, points: transformed }, inferRectFromGeometry || (!isStroke && transformed.length === 5))
            ) {
              const strokeColor = vectorExtras.stroking_color;
              const hasNonDefaultStrokeColor = Array.isArray(strokeColor)
                ? strokeColor.some((component, index) => Number(component) !== (strokeColor.length === 4 && index === 3 ? 1 : 0))
                : strokeColor !== 0;
              const fillColor = vectorExtras.non_stroking_color;
              const hasRgbWhiteFill = Array.isArray(fillColor) && fillColor.length === 3 && fillColor.every((component) => Number(component) === 1);
              rects.push(
                rectFromPdfBBox(rawBBox, pageWidth, pageHeight, pageRotate, pageNumber, "rect", doctopOffset, {
                  ...vectorExtras,
                  linewidth: isStroke || (state.lineWidthSet && ((Math.abs(linewidth - 1) <= 1e-9 && (hasNonDefaultStrokeColor || hasRgbWhiteFill || vectorExtras.tag === "Document")) || (Math.abs(linewidth - 1) > 1e-9 && (linewidth <= 1 || linewidth > 10 || vectorExtras.tag !== "OC")))) ? linewidth : 0
                }, coordOffset)
              );
            } else if (transformed.length > 0 && !(isStroke && !path.hasCurve && transformed.length > 1 && (rawBBox[0] === rawBBox[2] || rawBBox[1] === rawBBox[3]))) {
	              const curve = rectFromPdfBBox(rawBBox, pageWidth, pageHeight, pageRotate, pageNumber, "curve", doctopOffset, {
	                pts: pagePoints,
	                closepath: path.closed,
	                ...vectorExtras,
                linewidth: isStroke || state.lineWidthSet ? linewidth : 0
              }, coordOffset);
              curves.push(curve);
            }
          }
        }
        break;
      }
      case pdfjs.OPS.paintImageXObject:
      case pdfjs.OPS.paintInlineImageXObject:
      case pdfjs.OPS.paintImageMaskXObject: {
        const resourceBacked = typeof args?.[0] === "string";
        const argWidth = Number(args?.[1] ?? args?.[0]?.width ?? 0);
        const argHeight = Number(args?.[2] ?? args?.[0]?.height ?? 0);
        const resourceCandidate = imageResources[imageIndex];
        const dimensionsMatch = (resource: ImageResource | undefined): boolean =>
          Boolean(
            resource &&
              resource.width != null &&
              resource.height != null &&
              argWidth &&
              argHeight &&
              Math.abs(argWidth - resource.width) <= 1e-6 &&
              Math.abs(argHeight - resource.height) <= 1e-6
          );
        const resource = resourceBacked ? resourceCandidate : dimensionsMatch(resourceCandidate) ? resourceCandidate : undefined;
        if (
          resource &&
          resource.width != null &&
          resource.height != null &&
          argWidth &&
          argHeight &&
          (Math.abs(argWidth - resource.width) > 1e-6 || Math.abs(argHeight - resource.height) > 1e-6)
        ) {
          break;
        }
        if (resourceBacked && !resource && imageResources.length > 0) break;
        if (resource) imageIndex += 1;
        const srcWidth = Number(resource?.width ?? argWidth);
        const srcHeight = Number(resource?.height ?? argHeight);
        const rawBBox = bboxFromPoints([
          contentPoint(applyMatrix([0, 0], state.ctm)),
          contentPoint(applyMatrix([1, 0], state.ctm)),
          contentPoint(applyMatrix([0, 1], state.ctm)),
          contentPoint(applyMatrix([1, 1], state.ctm))
        ]);
        const image = rectFromPdfBBox(rawBBox, pageWidth, pageHeight, pageRotate, pageNumber, "image", doctopOffset, {
            name: resource?.name ?? (typeof args?.[0] === "string" ? args[0] : undefined),
            srcsize: srcWidth && srcHeight ? [srcWidth, srcHeight] : undefined,
            colorspace: resource?.colorspace,
            bits: resource?.bits,
            ...markedExtras()
          }, coordOffset);
        for (const key of ["width", "height"] as const) {
          image[key] = softenHalfMicro(Number(image[key]));
        }
        images.push(image);
        break;
      }
    }
  }

  return { chars, rects, lines, curves, images };
}

export function annotationToObject(
  annotation: any,
  pageNumber: number,
  pageWidth: number,
  pageHeight: number,
  pageRotate: number,
  doctopOffset: number,
  objects?: Map<number, string>
): PDFObject {
  const optionalString = (value: unknown): string | null => (typeof value === "string" && value.length > 0 ? value : null);
  const pdfplumberContents = (value: unknown): string | null => (typeof value === "string" ? value : null);
  const objectNumber = Number.parseInt(String(annotation.id ?? ""), 10);
  const annotationObject = Number.isFinite(objectNumber) ? objects?.get(objectNumber) : undefined;
  const rawRect = extractRawBox(annotationObject, "Rect");
  const rect = rawRect ? normalizeRawBox(rawRect) : (annotation.rect as number[]);
  const rawContents = annotation.contentsObj?.str ?? annotation.contents ?? null;
  const rawContentsFromObject = annotationObject ? readPdfDictStringLikePdfplumber(annotationObject, "Contents") : null;
  const rawTitle = annotationObject ? readPdfDictStringLikePdfplumber(annotationObject, "T") : null;
  const rawUri = annotationObject ? readAnnotationUriLikePdfplumber(annotationObject, objects) : null;
  const isTinyPopup = annotation.subtype === "Popup" && Math.abs(Number(rect[2]) - Number(rect[0])) <= 2 && Math.abs(Number(rect[3]) - Number(rect[1])) <= 2;
  return rectFromPdfBBox([rect[0], rect[1], rect[2], rect[3]], pageWidth, pageHeight, pageRotate, pageNumber, "annot", doctopOffset, {
    uri: annotationUri(annotation, rawUri),
    title: annotation.subtype === "Popup" ? null : optionalString(annotation.titleObj?.str ?? annotation.title ?? rawTitle ?? null),
    contents:
      (annotation.subtype === "Popup" && !isTinyPopup) ||
      ((annotation.subtype === "Link" || annotation.subtype === "Widget" || (annotation.subtype === "Stamp" && pageRotate !== 0)) && (rawContentsFromObject ?? rawContents) === "")
        ? null
        : pdfplumberContents(rawContentsFromObject ?? rawContents)
  });
}

function annotationUri(annotation: any, rawUri: string | null): string | null {
  const optionalString = (value: unknown): string | null => (typeof value === "string" && value.length > 0 ? value : null);
  const value = optionalString(rawUri ?? annotation.url ?? annotation.unsafeUrl ?? null);
  if (!value) return null;
  return /^[A-Za-z][A-Za-z0-9+.-]*:/.test(value) || !/\s/.test(value) ? value : null;
}

function readPdfDictStringLikePdfplumber(objectText: string, key: string): string | null {
  const bytes = parsePdfDictBytes(objectText, key);
  return bytes ? decodePdfLiteralBytesAsUtf8ThenUtf16(bytes) : null;
}

function readAnnotationUriLikePdfplumber(annotationObject: string, objects?: Map<number, string>): string | null {
  const direct = readPdfDictStringLikePdfplumber(annotationObject, "URI");
  if (direct != null) return direct;
  const actionRef = /\/A\s+(\d+)\s+\d+\s+R/.exec(annotationObject)?.[1];
  const actionObject = actionRef ? objects?.get(Number(actionRef)) : undefined;
  return actionObject ? readPdfDictStringLikePdfplumber(actionObject, "URI") : null;
}
