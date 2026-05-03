import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import path from "node:path";
import { inflateSync } from "node:zlib";

import * as pdfjs from "pdfjs-dist/legacy/build/pdf.mjs";

export type BBox = readonly [number, number, number, number];

export type PDFInput = string | URL | ArrayBuffer | Uint8Array;

export interface OpenOptions {
  pages?: number[];
  password?: string;
  laparams?: Record<string, unknown>;
  unicode_norm?: "NFC" | "NFKC" | "NFD" | "NFKD";
  raise_unicode_errors?: boolean;
}

export interface PDFObject {
  object_type: string;
  page_number: number;
  x0?: number;
  x1?: number;
  y0?: number;
  y1?: number;
  top?: number;
  bottom?: number;
  doctop?: number;
  width?: number;
  height?: number;
  text?: string;
  [key: string]: unknown;
}

export interface SearchResult {
  text: string;
  x0: number;
  top: number;
  x1: number;
  bottom: number;
  groups?: string[];
  chars?: PDFObject[];
}

export interface PDFPlumberPage {
  page_number: number;
  pageNumber: number;
  width: number;
  height: number;
  bbox: BBox;
  mediabox: BBox;
  cropbox: BBox;
  artbox?: BBox;
  bleedbox?: BBox;
  trimbox?: BBox;
  objects: Record<string, PDFObject[]>;
  chars: PDFObject[];
  rects: PDFObject[];
  lines: PDFObject[];
  curves: PDFObject[];
  images: PDFObject[];
  annots: PDFObject[];
  hyperlinks: PDFObject[];
  rect_edges: PDFObject[];
  curve_edges: PDFObject[];
  edges: PDFObject[];
  extract_text(options?: Record<string, unknown>): string | Promise<string>;
  extractText(options?: Record<string, unknown>): string | Promise<string>;
  extract_words(options?: Record<string, unknown>): PDFObject[] | Promise<PDFObject[]>;
  extractWords(options?: Record<string, unknown>): PDFObject[] | Promise<PDFObject[]>;
  search(pattern: string | RegExp, options?: Record<string, unknown>): SearchResult[] | Promise<SearchResult[]>;
  crop(bbox: BBox, options?: { relative?: boolean; strict?: boolean }): PDFPlumberPage;
  within_bbox(bbox: BBox, options?: { relative?: boolean; strict?: boolean }): PDFPlumberPage;
  withinBbox(bbox: BBox, options?: { relative?: boolean; strict?: boolean }): PDFPlumberPage;
  outside_bbox(bbox: BBox, options?: { relative?: boolean; strict?: boolean }): PDFPlumberPage;
  outsideBbox(bbox: BBox, options?: { relative?: boolean; strict?: boolean }): PDFPlumberPage;
  dedupe_chars(options?: Record<string, unknown>): PDFPlumberPage;
  dedupeChars(options?: Record<string, unknown>): PDFPlumberPage;
  extract_table(options?: Record<string, unknown>): (Array<Array<string | null>> | null) | Promise<Array<Array<string | null>> | null>;
  extractTable(options?: Record<string, unknown>): (Array<Array<string | null>> | null) | Promise<Array<Array<string | null>> | null>;
  extract_tables(options?: Record<string, unknown>): Array<Array<Array<string | null>>> | Promise<Array<Array<Array<string | null>>>>;
  extractTables(options?: Record<string, unknown>): Array<Array<Array<string | null>>> | Promise<Array<Array<Array<string | null>>>>;
  find_tables(options?: Record<string, unknown>): Table[] | Promise<Table[]>;
  findTables(options?: Record<string, unknown>): Table[] | Promise<Table[]>;
}

export interface PDFPlumberDocument {
  metadata: Record<string, unknown>;
  pages: PDFPlumberPage[];
  objects: Record<string, PDFObject[]>;
  annots: PDFObject[];
  hyperlinks: PDFObject[];
  close(): void | Promise<void>;
}

type Dir = "ttb" | "btt" | "ltr" | "rtl";
type Matrix = [number, number, number, number, number, number];
type Point = [number, number];
type MutableBBox = [number, number, number, number];

interface PdfJsGlyph {
  originalCharCode?: number;
  unicode?: string;
  width?: number;
  isSpace?: boolean;
  isInFont?: boolean;
  vmetric?: number[];
}

interface FontRecord {
  objectNumber: number;
  baseFont: string;
  firstChar: number;
  widths: number[];
  ascent?: number;
  descent?: number;
}

interface MappedFont {
  fontname: string;
  ascent: number;
  descent: number;
  fontMatrix0: number;
  vertical: boolean;
}

interface PageBoxes {
  bbox: BBox;
  mediabox: BBox;
  cropbox: BBox;
  artbox?: BBox;
  bleedbox?: BBox;
  trimbox?: BBox;
  width: number;
  height: number;
  rotate: number;
  contentYOffset: number;
}

interface GraphicsState {
  ctm: Matrix;
  fillColor: unknown;
  strokeColor: unknown;
  fillColorSpace: string;
  strokeColorSpace: string;
  lineWidth: number;
  lineWidthSet: boolean;
  dash: unknown;
  charSpacing: number;
  wordSpacing: number;
  textHScale: number;
  leading: number;
  textRise: number;
  fontId: string | null;
  fontSize: number;
  fontDirection: 1 | -1;
  textMatrix: Matrix;
  x: number;
  y: number;
  lineX: number;
  lineY: number;
}

interface ParsedPath {
  points: Point[];
  closed: boolean;
  hasCurve: boolean;
  lastOp: number;
  fromRect?: boolean;
  forceCurve?: boolean;
}

interface ColorOp {
  target: "fill" | "stroke";
  components: number[];
  colorSpace: string;
  pattern?: string;
}

interface ImageResource {
  name: string;
  width?: number;
  height?: number;
  bits?: number;
  colorspace?: unknown[];
}

interface PdfEncryption {
  encryptRef: number | null;
  key: Uint8Array;
  keyLength: number;
  streamMethod: "RC4" | "Identity" | "Unsupported";
  encryptMetadata: boolean;
}

const require = createRequire(import.meta.url);
const pdfjsRoot = path.dirname(require.resolve("pdfjs-dist/package.json"));

const DEFAULT_X_TOLERANCE = 3;
const DEFAULT_Y_TOLERANCE = 3;
const DEFAULT_X_DENSITY = 7.25;
const DEFAULT_Y_DENSITY = 13;

const LIGATURES: Record<string, string> = {
  "\ufb00": "ff",
  "\ufb03": "ffi",
  "\ufb04": "ffl",
  "\ufb01": "fi",
  "\ufb02": "fl",
  "\ufb06": "st",
  "\ufb05": "st"
};

const METADATA_KEYS = ["Author", "Creator", "Producer", "Title", "Subject", "Keywords", "CreationDate", "ModDate", "Copies"];
const PUNCTUATION = "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~";
const STANDARD_FONT_ALIASES: Record<string, string> = {
  Arial: "Helvetica",
  "Arial,Bold": "Helvetica-Bold",
  "Arial,Italic": "Helvetica-Oblique",
  "Arial,BoldItalic": "Helvetica-BoldOblique"
};
const STANDARD_DESCENTS: Record<string, number> = {
  Helvetica: -0.207,
  "Helvetica-Bold": -0.207,
  "Helvetica-Oblique": -0.207,
  "Helvetica-BoldOblique": -0.207,
  "Times-Roman": -0.217,
  "Times-Bold": -0.205,
  "Times-Italic": -0.217,
  "Times-BoldItalic": -0.205
};

export class NotImplementedError extends Error {
  constructor(feature: string) {
    super(`${feature} is not implemented yet. Run npm run test:compat to drive the TypeScript port against Python pdfplumber goldens.`);
    this.name = "NotImplementedError";
  }
}

function namedError(name: string, message: string): Error {
  const error = new Error(message);
  error.name = name;
  return error;
}

function cleanNumber(value: number): number {
  if (!Number.isFinite(value)) return value;
  const factor = 1_000_000;
  const scaled = Math.abs(value) * factor;
  const floor = Math.floor(scaled);
  const fraction = scaled - floor;
  const roundedInt =
    Math.abs(fraction - 0.5) < 1e-12
      ? floor + (floor % 2 === 0 ? 0 : 1)
      : Math.round(scaled);
  const rounded = Math.sign(value) * roundedInt / factor;
  return Object.is(rounded, -0) ? 0 : rounded;
}

function cleanBBox(bbox: readonly number[]): BBox {
  return [cleanNumber(bbox[0]), cleanNumber(bbox[1]), cleanNumber(bbox[2]), cleanNumber(bbox[3])];
}

function cleanMatrix(matrix: Matrix): Matrix {
  return matrix.map((value) => {
    const rounded = Number(value.toFixed(6));
    return Object.is(rounded, -0) ? 0 : rounded;
  }) as Matrix;
}

function firstFinite(fallback: number, ...values: unknown[]): number {
  for (const value of values) {
    if (typeof value === "number" && Number.isFinite(value)) return value;
  }
  return fallback;
}

function snapPdfCoordinate(value: number): number {
  if (!Number.isFinite(value)) return value;
  if (Math.abs(value) < 1) return value;
  for (let digits = 0; digits <= 4; digits += 1) {
    const rounded = Number(value.toFixed(digits));
    const tolerance = digits <= 2 ? 0.000075 : 0.00003;
    if (Math.abs(value - rounded) <= tolerance) return rounded;
  }
  return value;
}

function snapPdfPathCoordinate(value: number): number {
  if (!Number.isFinite(value)) return value;
  if (Math.abs(value) < 1) return value;
  for (let digits = 0; digits <= 3; digits += 1) {
    const rounded = Number(value.toFixed(digits));
    const tolerance = digits <= 2 ? 0.000075 : 0.000035;
    if (Math.abs(value - rounded) <= tolerance) return rounded;
  }
  return value;
}

function snapPdfOperand(value: number): number {
  if (!Number.isFinite(value)) return value;
  for (let digits = 0; digits <= 4; digits += 1) {
    const rounded = Number(value.toFixed(digits));
    const tolerance = digits <= 2 ? 0.000075 : 0.00003;
    if (Math.abs(value - rounded) <= tolerance) return rounded;
  }
  return value;
}

function snapRawTextMatrixOperand(value: number): number {
  if (!Number.isFinite(value)) return value;
  for (let digits = 0; digits <= 3; digits += 1) {
    const rounded = Number(value.toFixed(digits));
    const tolerance = digits <= 2 ? 0.000075 : 0.00003;
    if (Math.abs(value - rounded) <= tolerance) return rounded;
  }
  return value;
}

function snapPdfExtent(value: number): number {
  if (!Number.isFinite(value)) return value;
  const sign = value < 0 ? -1 : 1;
  const magnitude = Math.abs(value);
  if (magnitude < 1) {
    const rounded5 = Number(magnitude.toFixed(5));
    const rounded2 = Number(rounded5.toFixed(2));
    const delta2 = Math.abs(rounded5 - rounded2);
    if (Math.abs(magnitude - rounded5) <= 0.000005 && delta2 > 0.000005 && delta2 <= 0.00003) {
      return sign * rounded5;
    }
    if (Math.abs(magnitude - rounded2) <= 0.000075) return sign * rounded2;
  }
  return sign * snapPdfCoordinate(magnitude);
}

function snapPdfPathExtent(value: number): number {
  if (!Number.isFinite(value)) return value;
  const sign = value < 0 ? -1 : 1;
  const magnitude = Math.abs(value);
  if (magnitude < 1) {
    const rounded5 = Number(magnitude.toFixed(5));
    const rounded2 = Number(rounded5.toFixed(2));
    const delta2 = Math.abs(rounded5 - rounded2);
    if (Math.abs(magnitude - rounded5) <= 0.000005 && delta2 > 0.000005 && delta2 <= 0.00003) {
      return sign * rounded5;
    }
    if (Math.abs(magnitude - rounded2) <= 0.000075) return sign * rounded2;
  }
  return sign * snapPdfPathCoordinate(magnitude);
}

function cloneMatrix(matrix: Matrix): Matrix {
  return [...matrix] as Matrix;
}

function multiplyMatrix(a: Matrix, b: Matrix): Matrix {
  return [
    a[0] * b[0] + a[2] * b[1],
    a[1] * b[0] + a[3] * b[1],
    a[0] * b[2] + a[2] * b[3],
    a[1] * b[2] + a[3] * b[3],
    a[0] * b[4] + a[2] * b[5] + a[4],
    a[1] * b[4] + a[3] * b[5] + a[5]
  ];
}

function lineWidthScale(matrix: Matrix): number {
  const scale = Math.hypot(matrix[0], matrix[1]);
  return scale || 1;
}

function applyMatrix(point: Point, matrix: Matrix, snap = false): Point {
  const [x, y] = point;
  const transformed: Point = [x * matrix[0] + y * matrix[2] + matrix[4], x * matrix[1] + y * matrix[3] + matrix[5]];
  return snap ? [snapPdfCoordinate(transformed[0]), snapPdfCoordinate(transformed[1])] : transformed;
}

function bboxFromPoints(points: Point[]): MutableBBox {
  const xs = points.map((p) => p[0]);
  const ys = points.map((p) => p[1]);
  return [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)];
}

function snapPathBBox(bbox: MutableBBox): MutableBBox {
  const x0 = snapPdfPathCoordinate(bbox[0]);
  const y0 = snapPdfPathCoordinate(bbox[1]);
  return [x0, y0, x0 + snapPdfPathExtent(bbox[2] - bbox[0]), y0 + snapPdfPathExtent(bbox[3] - bbox[1])];
}

function pathBBoxHasMeaningfulPrecision(bbox: MutableBBox): boolean {
  return bbox.some((value) => Number.isFinite(value) && Math.abs(value) >= 1 && Math.abs(value - Number(value.toFixed(2))) > 0.000075);
}

function snapPathOperandPoint(point: Point): Point {
  return [snapPdfPathCoordinate(point[0]), snapPdfPathCoordinate(point[1])];
}

function pointToPageCoords(point: Point, pageWidth: number, pageHeight: number, rotate: number): Point {
  const normalized = ((rotate % 360) + 360) % 360;
  const [x, y] = point;
  if (normalized === 90) return [y, x];
  if (normalized === 180) return [pageWidth - x, y];
  if (normalized === 270) return [pageWidth - y, pageHeight - x];
  return [x, pageHeight - y];
}

function bboxToPageBBox(raw: MutableBBox, pageWidth: number, pageHeight: number, rotate: number): MutableBBox {
  return bboxFromPoints([
    pointToPageCoords([raw[0], raw[1]], pageWidth, pageHeight, rotate),
    pointToPageCoords([raw[0], raw[3]], pageWidth, pageHeight, rotate),
    pointToPageCoords([raw[2], raw[1]], pageWidth, pageHeight, rotate),
    pointToPageCoords([raw[2], raw[3]], pageWidth, pageHeight, rotate)
  ]);
}

function matrixToPageMatrix(matrix: Matrix, pageWidth: number, pageHeight: number, rotate: number): Matrix {
  const normalized = ((rotate % 360) + 360) % 360;
  const rawWidth = normalized === 90 || normalized === 270 ? pageHeight : pageWidth;
  const rawHeight = normalized === 90 || normalized === 270 ? pageWidth : pageHeight;
  const pageMatrix: Matrix =
    normalized === 90
      ? [0, -1, 1, 0, 0, rawWidth]
      : normalized === 180
        ? [-1, 0, 0, -1, rawWidth, rawHeight]
        : normalized === 270
          ? [0, 1, -1, 0, rawHeight, 0]
          : [1, 0, 0, 1, 0, 0];
  return multiplyMatrix(pageMatrix, matrix);
}

function rectFromPdfBBox(
  raw: MutableBBox,
  pageWidth: number,
  pageHeight: number,
  pageRotate: number,
  pageNumber: number,
  objectType: string,
  doctopOffset: number,
  extras: Record<string, unknown> = {},
  coordOffset: Point = [0, 0]
): PDFObject {
  const [x0, top, x1, bottom] = bboxToPageBBox(raw, pageWidth, pageHeight, pageRotate);
  const [y0, y1] =
    ((pageRotate % 360) + 360) % 360 === 0
      ? [raw[1] + coordOffset[1], raw[3] + coordOffset[1]]
      : [pageHeight - bottom + coordOffset[1], pageHeight - top + coordOffset[1]];
  return {
    object_type: objectType,
    page_number: pageNumber,
    x0,
    x1,
    y0,
    y1,
    top,
    bottom,
    doctop: top + doctopOffset,
    width: x1 - x0,
    height: y1 - y0,
    ...extras
  };
}

function rectFromPageBBox(raw: MutableBBox, pageHeight: number, pageNumber: number, objectType: string, doctopOffset: number, extras: Record<string, unknown> = {}): PDFObject {
  const [x0, y0, x1, y1] = raw;
  const top = pageHeight - y1;
  const bottom = pageHeight - y0;
  return {
    object_type: objectType,
    page_number: pageNumber,
    x0,
    x1,
    y0,
    y1,
    top,
    bottom,
    doctop: top + doctopOffset,
    width: x1 - x0,
    height: bottom - top,
    ...extras
  };
}

function cleanObject<T extends Record<string, unknown>>(obj: T): T {
  const out: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(obj)) {
    if (typeof value === "number") out[key] = cleanNumber(value);
    else if (Array.isArray(value)) out[key] = value.map((v) => (typeof v === "number" ? cleanNumber(v) : v));
    else out[key] = value;
  }
  return out as T;
}

function softenHalfMicro(value: number): number {
  if (!Number.isFinite(value)) return value;
  const scaled = Math.abs(value) * 1_000_000;
  const fraction = scaled - Math.floor(scaled);
  if (Math.abs(fraction - 0.5) > 1e-7) return value;
  return value - Math.sign(value || 1) * 1e-9;
}

function colorValue(value: unknown): unknown {
  if (Array.isArray(value) && value.length === 0) return [0];
  if (typeof value === "string" && /^#[0-9a-f]{6}$/i.test(value)) {
    const r = parseInt(value.slice(1, 3), 16) / 255;
    const g = parseInt(value.slice(3, 5), 16) / 255;
    const b = parseInt(value.slice(5, 7), 16) / 255;
    if (Math.abs(r - g) < 1e-9 && Math.abs(g - b) < 1e-9) return [cleanNumber(r)];
    return [cleanNumber(r), cleanNumber(g), cleanNumber(b)];
  }
  if (Array.isArray(value)) return value.map((v) => (typeof v === "number" ? cleanNumber(v) : v));
  return value;
}

function graphicColorValue(value: unknown, colorSpace: string): unknown {
  if (Array.isArray(value) && value.length === 1 && typeof value[0] === "number") return cleanNumber(value[0]);
  if (colorSpace === "DeviceGray" && Array.isArray(value) && value.length === 1 && typeof value[0] === "number") return cleanNumber(value[0]);
  if (colorSpace === "DeviceGray" && typeof value === "string" && /^#[0-9a-f]{6}$/i.test(value)) {
    const rgb = rgbColor(value);
    if (Math.abs(rgb[0] - rgb[1]) < 1e-9 && Math.abs(rgb[1] - rgb[2]) < 1e-9) return rgb[0];
  }
  return colorValue(value);
}

function colorSpaceName(value: unknown): string {
  const raw =
    typeof value === "string"
      ? value
      : typeof (value as { name?: unknown } | null)?.name === "string"
        ? String((value as { name: string }).name)
        : String(value ?? "");
  if (raw === "DeviceRGB" || raw === "RGB") return "DeviceRGB";
  if (raw === "DeviceCMYK" || raw === "CMYK") return "DeviceCMYK";
  if (raw === "DeviceGray" || raw === "G") return "DeviceGray";
  if (raw === "CSp") return "DeviceRGB";
  if (raw === "Pattern") return "Pattern";
  if (/ICCBased/i.test(raw)) return "ICCBased";
  if (/^CS\d+$/i.test(raw)) return "ICCBased";
  return raw.replace(/^\//, "") || "DeviceGray";
}

function pythonBytesName(value: string): string {
  if (!/[^\x20-\x7e]/.test(value)) return value;
  const body = [...value].map((char) => {
    const code = char.charCodeAt(0) & 0xff;
    if (code === 0x5c) return "\\\\";
    if (code === 0x27) return "\\'";
    return code >= 0x20 && code <= 0x7e ? String.fromCharCode(code) : `\\x${code.toString(16).padStart(2, "0")}`;
  }).join("");
  return `b'${body}'`;
}

function rgbColor(value: unknown): number[] {
  if (typeof value === "string" && /^#[0-9a-f]{6}$/i.test(value)) {
    return [parseInt(value.slice(1, 3), 16) / 255, parseInt(value.slice(3, 5), 16) / 255, parseInt(value.slice(5, 7), 16) / 255].map(cleanNumber);
  }
  return Array.isArray(value) ? value.map(Number).map(cleanNumber) : [0, 0, 0];
}

function objectsByType(entries: Array<[string, PDFObject[]]>): Record<string, PDFObject[]> {
  const out: Record<string, PDFObject[]> = {};
  for (const [key, value] of entries) {
    if (value.length > 0) out[key] = value;
  }
  return out;
}

function aggregateObjects(pages: PDFPlumberPage[]): Record<string, PDFObject[]> {
  const out: Record<string, PDFObject[]> = {};
  for (const page of pages) {
    for (const [key, value] of Object.entries(page.objects)) {
      out[key] ??= [];
      out[key].push(...value);
    }
  }
  return out;
}

function clusterList(xs: number[], tolerance = 0): number[][] {
  const sorted = [...xs].sort((a, b) => a - b);
  if (sorted.length < 2) return sorted.map((x) => [x]);
  const groups: number[][] = [];
  let current = [sorted[0]];
  let last = sorted[0];
  for (const x of sorted.slice(1)) {
    if (x <= last + tolerance) current.push(x);
    else {
      groups.push(current);
      current = [x];
    }
    last = x;
  }
  groups.push(current);
  return groups;
}

function clusterObjectsSimple<T>(items: T[], key: (item: T) => number, tolerance: number, preserveOrder = false): T[][] {
  const values = Array.from(new Set(items.map(key)));
  const clusters = clusterList(values, tolerance);
  const dict = new Map<number, number>();
  clusters.forEach((cluster, index) => {
    for (const value of cluster) dict.set(value, index);
  });
  const tuples = items.map((item) => [item, dict.get(key(item)) ?? 0] as const);
  const sorted = preserveOrder ? tuples : [...tuples].sort((a, b) => a[1] - b[1]);
  const grouped: T[][] = [];
  let previous: number | undefined;
  for (const [item, cluster] of sorted) {
    if (cluster !== previous) {
      grouped.push([item]);
      previous = cluster;
    } else {
      grouped[grouped.length - 1].push(item);
    }
  }
  return grouped;
}

function objectsToBBox(objects: PDFObject[]): BBox {
  return [
    Math.min(...objects.map((o) => Number(o.x0))),
    Math.min(...objects.map((o) => Number(o.top))),
    Math.max(...objects.map((o) => Number(o.x1))),
    Math.max(...objects.map((o) => Number(o.bottom)))
  ];
}

function getBBoxOverlap(a: BBox, b: BBox): BBox | null {
  const left = Math.max(a[0], b[0]);
  const right = Math.min(a[2], b[2]);
  const top = Math.max(a[1], b[1]);
  const bottom = Math.min(a[3], b[3]);
  if (bottom >= top && right >= left && bottom - top + (right - left) > 0) return [left, top, right, bottom];
  return null;
}

function objToBBox(obj: PDFObject): BBox {
  return [Number(obj.x0), Number(obj.top), Number(obj.x1), Number(obj.bottom)];
}

function clipObj(obj: PDFObject, bbox: BBox): PDFObject | null {
  const overlap = getBBoxOverlap(objToBBox(obj), bbox);
  if (!overlap) return null;
  const copy = { ...obj };
  const diff = overlap[1] - Number(obj.top);
  copy.x0 = overlap[0];
  copy.top = overlap[1];
  copy.x1 = overlap[2];
  copy.bottom = overlap[3];
  if (typeof copy.doctop === "number") copy.doctop += diff;
  copy.width = overlap[2] - overlap[0];
  copy.height = overlap[3] - overlap[1];
  if (typeof copy.y0 === "number") copy.y0 -= overlap[3] - Number(obj.bottom);
  if (typeof copy.y1 === "number") copy.y1 -= diff;
  return cleanObject(copy);
}

function cropToBBox(objects: PDFObject[], bbox: BBox): PDFObject[] {
  return objects.map((obj) => clipObj(obj, bbox)).filter((obj): obj is PDFObject => obj !== null);
}

function withinBBox(objects: PDFObject[], bbox: BBox): PDFObject[] {
  return objects.filter((obj) => {
    const overlap = getBBoxOverlap(objToBBox(obj), bbox);
    return overlap !== null && JSON.stringify(cleanBBox(overlap)) === JSON.stringify(cleanBBox(objToBBox(obj)));
  });
}

function outsideBBox(objects: PDFObject[], bbox: BBox): PDFObject[] {
  return objects.filter((obj) => getBBoxOverlap(objToBBox(obj), bbox) === null);
}

function resizeObject(obj: PDFObject, key: "x0" | "x1" | "top" | "bottom", value: number): PDFObject {
  const copy = { ...obj, [key]: value };
  if (key === "x0" || key === "x1") copy.width = Number(copy.x1) - Number(copy.x0);
  if (key === "top") {
    const diff = value - Number(obj.top);
    if (typeof copy.doctop === "number") copy.doctop += diff;
    copy.height = Number(copy.bottom) - value;
    if (typeof copy.y1 === "number") copy.y1 -= diff;
  }
  if (key === "bottom") {
    copy.height = value - Number(copy.top);
    if (typeof copy.y0 === "number") copy.y0 -= value - Number(obj.bottom);
  }
  return cleanObject(copy);
}

function moveObject(obj: PDFObject, axis: "h" | "v", value: number): PDFObject {
  const copy = { ...obj };
  if (axis === "h") {
    copy.x0 = Number(obj.x0) + value;
    copy.x1 = Number(obj.x1) + value;
  } else {
    copy.top = Number(obj.top) + value;
    copy.bottom = Number(obj.bottom) + value;
    if (typeof copy.doctop === "number") copy.doctop += value;
    if (typeof copy.y0 === "number") copy.y0 -= value;
    if (typeof copy.y1 === "number") copy.y1 -= value;
  }
  return cleanObject(copy);
}

function snapObjects(objects: PDFObject[], attr: "x0" | "x1" | "top" | "bottom", tolerance: number): PDFObject[] {
  const axis = attr === "x0" || attr === "x1" ? "h" : "v";
  return clusterObjectsSimple(objects, (obj) => Number(obj[attr]), tolerance).flatMap((cluster) => {
    const avg = cluster.reduce((sum, obj) => sum + Number(obj[attr]), 0) / cluster.length;
    return cluster.map((obj) => moveObject(obj, axis, avg - Number(obj[attr])));
  });
}

function rectToEdges(rect: PDFObject): PDFObject[] {
  const top = { ...rect, object_type: "rect_edge", height: 0, y0: rect.y1, bottom: rect.top, orientation: "h" };
  const bottom = {
    ...rect,
    object_type: "rect_edge",
    height: 0,
    y1: rect.y0,
    top: Number(rect.top) + Number(rect.height),
    doctop: Number(rect.doctop) + Number(rect.height),
    orientation: "h"
  };
  const left = { ...rect, object_type: "rect_edge", width: 0, x1: rect.x0, orientation: "v" };
  const right = { ...rect, object_type: "rect_edge", width: 0, x0: rect.x1, orientation: "v" };
  return [top, bottom, left, right].map((obj) => cleanObject(obj as PDFObject));
}

function curveToEdges(curve: PDFObject): PDFObject[] {
  const pts = (curve.pts as Point[] | undefined) ?? [];
  const edges: PDFObject[] = [];
  for (let i = 0; i < pts.length - 1; i += 1) {
    const [p0, p1] = [pts[i], pts[i + 1]];
    const x0 = Math.min(p0[0], p1[0]);
    const x1 = Math.max(p0[0], p1[0]);
    const top = Math.min(p0[1], p1[1]);
    const bottom = Math.max(p0[1], p1[1]);
    edges.push(
      cleanObject({
        object_type: "curve_edge",
        page_number: curve.page_number,
        x0,
        x1,
        top,
        doctop: top + (Number(curve.doctop) - Number(curve.top)),
        bottom,
        width: x1 - x0,
        height: bottom - top,
        orientation: p0[0] === p1[0] ? "v" : p0[1] === p1[1] ? "h" : null
      })
    );
  }
  return edges;
}

function lineToEdge(line: PDFObject): PDFObject {
  return cleanObject({ ...line, orientation: Number(line.top) === Number(line.bottom) ? "h" : "v" });
}

function objToEdges(obj: PDFObject): PDFObject[] {
  if (obj.object_type.includes("_edge")) return [obj];
  if (obj.object_type === "line") return [lineToEdge(obj)];
  if (obj.object_type === "rect") return rectToEdges(obj);
  if (obj.object_type === "curve") return curveToEdges(obj);
  return [];
}

function filterEdges(edges: PDFObject[], orientation?: "h" | "v", edgeType?: string, minLength = 1): PDFObject[] {
  return edges.filter((edge) => {
    const edgeOrientation = edge.orientation as "h" | "v" | undefined;
    const dim = edgeOrientation === "v" ? "height" : "width";
    return (!orientation || edgeOrientation === orientation) && (!edgeType || edge.object_type === edgeType) && Number(edge[dim]) >= minLength;
  });
}

function getLineClusterKey(lineDir: Dir): (obj: PDFObject) => number {
  return {
    ttb: (x: PDFObject) => Number(x.top),
    btt: (x: PDFObject) => -Number(x.bottom),
    ltr: (x: PDFObject) => Number(x.x0),
    rtl: (x: PDFObject) => -Number(x.x1)
  }[lineDir];
}

function getCharSortKey(charDir: Dir): (obj: PDFObject) => [number, number] {
  return {
    ttb: (x: PDFObject): [number, number] => [Number(x.top), Number(x.bottom)],
    btt: (x: PDFObject): [number, number] => [-(Number(x.top) + Number(x.height)), -Number(x.top)],
    ltr: (x: PDFObject): [number, number] => [Number(x.x0), Number(x.x0)],
    rtl: (x: PDFObject): [number, number] => [-Number(x.x1), -Number(x.x0)]
  }[charDir];
}

function bboxOriginKey(dir: Dir, bbox: BBox): number {
  return { ttb: bbox[1], btt: bbox[3], ltr: bbox[0], rtl: bbox[2] }[dir];
}

function positionKey(dir: Dir, obj: PDFObject): number {
  return { ttb: Number(obj.top), btt: Number(obj.bottom), ltr: Number(obj.x0), rtl: Number(obj.x1) }[dir];
}

function validateDirections(lineDir: Dir, charDir: Dir): void {
  if (new Set(lineDir).size === new Set([...lineDir, ...charDir]).size && [...lineDir].every((c) => charDir.includes(c))) {
    throw new Error(`Incompatible text directions: ${lineDir}, ${charDir}`);
  }
}

class TextMap {
  constructor(
    readonly tuples: Array<[string, PDFObject | null]>,
    readonly lineDirRender: Dir,
    readonly charDirRender: Dir
  ) {}

  get as_string(): string {
    return this.toString();
  }

  toString(): string {
    const base = this.tuples.map(([text]) => text).join("");
    if (this.charDirRender === "ltr" && this.lineDirRender === "ttb") return base;
    let lines = base.split("\n");
    if (this.lineDirRender === "btt" || this.lineDirRender === "rtl") lines = [...lines].reverse();
    if (this.charDirRender === "rtl") lines = lines.map((line) => [...line].reverse().join(""));
    if (this.lineDirRender === "rtl" || this.lineDirRender === "ltr") {
      const maxLineLength = Math.max(...lines.map((line) => line.length));
      if (this.charDirRender === "btt") lines = lines.map((line) => " ".repeat(maxLineLength - line.length) + line);
      else lines = lines.map((line) => line + " ".repeat(maxLineLength - line.length));
      return Array.from({ length: maxLineLength }, (_, i) => lines.map((line) => line[i]).join("")).join("\n");
    }
    return lines.join("\n");
  }

  search(pattern: string | RegExp, options: Record<string, unknown> = {}): SearchResult[] {
    const regexOption = options.regex !== false;
    const caseOption = options.case !== false;
    const mainGroup = Number(options.main_group ?? 0);
    const returnGroups = options.return_groups !== false;
    const returnChars = options.return_chars !== false;
    let regex: RegExp;
    if (pattern instanceof RegExp) {
      regex = pattern;
    } else {
      const source = regexOption ? pattern : pattern.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
      regex = new RegExp(source, `g${caseOption ? "" : "i"}`);
    }
    const out: SearchResult[] = [];
    for (const match of this.as_string.matchAll(regex)) {
      const text = match[mainGroup] ?? "";
      if (!text.trim()) continue;
      const start = (match.index ?? 0) + (mainGroup === 0 ? 0 : match[0].indexOf(text));
      const chars = this.tuples.slice(start, start + text.length).map(([, char]) => char).filter((char): char is PDFObject => char !== null);
      if (!chars.length) continue;
      const bbox = objectsToBBox(chars);
      const result: SearchResult = {
        text,
        x0: cleanNumber(bbox[0]),
        top: cleanNumber(bbox[1]),
        x1: cleanNumber(bbox[2]),
        bottom: cleanNumber(bbox[3])
      };
      if (returnGroups) result.groups = match.slice(1);
      if (returnChars) result.chars = chars;
      out.push(result);
    }
    return out;
  }
}

class WordMap {
  constructor(readonly tuples: Array<[PDFObject, PDFObject[]]>) {}

  toTextMap(options: Record<string, unknown> = {}): TextMap {
    const layout = Boolean(options.layout);
    const layoutWidth = Number(options.layout_width ?? 0);
    const layoutHeight = Number(options.layout_height ?? 0);
    let layoutWidthChars = Number(options.layout_width_chars ?? 0);
    let layoutHeightChars = Number(options.layout_height_chars ?? 0);
    const layoutBBox = (options.layout_bbox as BBox | undefined) ?? ([0, 0, 0, 0] as BBox);
    const xDensity = Number(options.x_density ?? DEFAULT_X_DENSITY);
    const yDensity = Number(options.y_density ?? DEFAULT_Y_DENSITY);
    const xShift = Number(options.x_shift ?? 0);
    const yShift = Number(options.y_shift ?? 0);
    const yTolerance = Number(options.y_tolerance ?? DEFAULT_Y_TOLERANCE);
    const lineDir = (options.line_dir as Dir | undefined) ?? "ttb";
    const charDir = (options.char_dir as Dir | undefined) ?? "ltr";
    const lineDirRender = (options.line_dir_render as Dir | undefined) ?? lineDir;
    const charDirRender = (options.char_dir_render as Dir | undefined) ?? charDir;
    const useTextFlow = Boolean(options.use_text_flow);
    const presorted = Boolean(options.presorted);
    const expansions = options.expand_ligatures === false ? {} : LIGATURES;
    validateDirections(lineDirRender, charDirRender);

    const textmap: Array<[string, PDFObject | null]> = [];
    if (!this.tuples.length) return new TextMap(textmap, lineDirRender, charDirRender);

    let blankLine: Array<[string, null]> = [];
    if (layout) {
      if (!layoutWidthChars) layoutWidthChars = Math.round(layoutWidth / xDensity);
      if (!layoutHeightChars) layoutHeightChars = Math.round(layoutHeight / yDensity);
      blankLine = Array.from({ length: layoutWidthChars }, () => [" ", null]);
    }

    const lineClusterKey = getLineClusterKey(lineDir);
    const charSortKey = getCharSortKey(charDir);
    const yOrigin = bboxOriginKey(lineDir, layoutBBox);
    const xOrigin = bboxOriginKey(charDir, layoutBBox);
    const wordsSorted = presorted || useTextFlow ? this.tuples : [...this.tuples].sort((a, b) => lineClusterKey(a[0]) - lineClusterKey(b[0]));
    const lines = clusterObjectsSimple(wordsSorted, (tuple) => lineClusterKey(tuple[0]), yTolerance, presorted || useTextFlow);
    let numNewlines = 0;

    lines.forEach((lineTuples, lineIndex) => {
      let yDist = 0;
      if (layout) {
        const raw = positionKey(lineDir, lineTuples[0][0]) - (yOrigin + yShift);
        yDist = (raw * (lineDir === "btt" || lineDir === "rtl" ? -1 : 1)) / yDensity;
      }
      const newlines = Math.max(lineIndex > 0 ? 1 : 0, Math.round(yDist) - numNewlines);
      for (let i = 0; i < newlines; i += 1) {
        if (!textmap.length || textmap[textmap.length - 1][0] === "\n") textmap.push(...blankLine);
        textmap.push(["\n", null]);
      }
      numNewlines += newlines;

      let lineLength = 0;
      const sortedLine = presorted || useTextFlow ? lineTuples : [...lineTuples].sort((a, b) => {
        const ak = charSortKey(a[0]);
        const bk = charSortKey(b[0]);
        return ak[0] - bk[0] || ak[1] - bk[1];
      });
      for (const [word, chars] of sortedLine) {
        let xDist = 0;
        if (layout) {
          const raw = positionKey(charDir, word) - (xOrigin + xShift);
          xDist = (raw * (charDir === "btt" || charDir === "rtl" ? -1 : 1)) / xDensity;
        }
        const spaces = Math.max(Math.min(1, lineLength), Math.round(xDist) - lineLength);
        for (let i = 0; i < spaces; i += 1) textmap.push([" ", null]);
        lineLength += spaces;
        for (const char of chars) {
          for (const letter of [...(expansions[char.text ?? ""] ?? char.text ?? "")]) {
            textmap.push([letter, char]);
            lineLength += 1;
          }
        }
      }
      if (layout) {
        for (let i = lineLength; i < layoutWidthChars; i += 1) textmap.push([" ", null]);
      }
    });

    if (layout) {
      const append = layoutHeightChars - (numNewlines + 1);
      for (let i = 0; i < append; i += 1) {
        if (i > 0) textmap.push(...blankLine);
        textmap.push(["\n", null]);
      }
      if (textmap[textmap.length - 1]?.[0] === "\n") textmap.pop();
    }
    return new TextMap(textmap, lineDirRender, charDirRender);
  }
}

class WordExtractor {
  readonly xTolerance: number;
  readonly yTolerance: number;
  readonly xToleranceRatio: number | null;
  readonly yToleranceRatio: number | null;
  readonly keepBlankChars: boolean;
  readonly useTextFlow: boolean;
  readonly horizontalLtr: boolean;
  readonly verticalTtb: boolean;
  readonly lineDir: Dir;
  readonly charDir: Dir;
  readonly lineDirRotated: Dir;
  readonly charDirRotated: Dir;
  readonly extraAttrs: string[];
  readonly splitAtPunctuation: string;
  readonly expansions: Record<string, string>;

  constructor(options: Record<string, unknown> = {}) {
    this.xTolerance = Number(options.x_tolerance ?? DEFAULT_X_TOLERANCE);
    this.yTolerance = Number(options.y_tolerance ?? DEFAULT_Y_TOLERANCE);
    this.xToleranceRatio = options.x_tolerance_ratio == null ? null : Number(options.x_tolerance_ratio);
    this.yToleranceRatio = options.y_tolerance_ratio == null ? null : Number(options.y_tolerance_ratio);
    this.keepBlankChars = Boolean(options.keep_blank_chars);
    this.useTextFlow = Boolean(options.use_text_flow);
    this.horizontalLtr = options.horizontal_ltr !== false;
    this.verticalTtb = options.vertical_ttb !== false;
    this.lineDir = (options.line_dir as Dir | undefined) ?? "ttb";
    this.charDir = (options.char_dir as Dir | undefined) ?? "ltr";
    this.lineDirRotated = (options.line_dir_rotated as Dir | undefined) ?? this.charDir;
    this.charDirRotated = (options.char_dir_rotated as Dir | undefined) ?? this.lineDir;
    this.extraAttrs = Array.isArray(options.extra_attrs) ? options.extra_attrs.map(String) : [];
    this.splitAtPunctuation = options.split_at_punctuation === true ? PUNCTUATION : options.split_at_punctuation ? String(options.split_at_punctuation) : "";
    this.expansions = options.expand_ligatures === false ? {} : LIGATURES;
    validateDirections(this.lineDir, this.charDir);
    validateDirections(this.lineDirRotated, this.charDirRotated);
  }

  getCharDir(upright: boolean): Dir {
    if (!upright && !this.verticalTtb) return "btt";
    if (upright && !this.horizontalLtr) return "rtl";
    return upright ? this.charDir : this.charDirRotated;
  }

  mergeChars(chars: PDFObject[]): PDFObject {
    const [x0, top, x1, bottom] = objectsToBBox(chars);
    const doctopAdj = Number(chars[0].doctop) - Number(chars[0].top);
    const upright = Boolean(chars[0].upright);
    const direction = this.getCharDir(upright);
    const word: Record<string, unknown> = {
      text: chars.map((c) => this.expansions[c.text ?? ""] ?? c.text ?? "").join(""),
      x0,
      x1,
      top,
      doctop: top + doctopAdj,
      bottom,
      upright,
      height: bottom - top,
      width: x1 - x0,
      direction
    };
    for (const key of this.extraAttrs) word[key] = chars[0][key];
    return cleanObject(word) as PDFObject;
  }

  charBeginsNewWord(prev: PDFObject, curr: PDFObject, direction: Dir, xTolerance: number, yTolerance: number): boolean {
    let ax: number;
    let bx: number;
    let cx: number;
    let ay: number;
    let cy: number;
    let x: number;
    let y: number;
    if (direction === "ltr" || direction === "rtl") {
      x = xTolerance;
      y = yTolerance;
      ay = Number(prev.top);
      cy = Number(curr.top);
      if (direction === "ltr") {
        ax = Number(prev.x0);
        bx = Number(prev.x1);
        cx = Number(curr.x0);
      } else {
        ax = -Number(prev.x1);
        bx = -Number(prev.x0);
        cx = -Number(curr.x1);
      }
    } else {
      x = yTolerance;
      y = xTolerance;
      ay = Number(prev.x0);
      cy = Number(curr.x0);
      if (direction === "ttb") {
        ax = Number(prev.top);
        bx = Number(prev.bottom);
        cx = Number(curr.top);
      } else {
        ax = -Number(prev.bottom);
        bx = -Number(prev.top);
        cx = -Number(curr.bottom);
      }
    }
    return cx < ax || cx > bx + x || Math.abs(cy - ay) > y;
  }

  *iterCharsToWords(chars: PDFObject[], direction: Dir): Generator<PDFObject[]> {
    let current: PDFObject[] = [];
    const flush = function* (next?: PDFObject): Generator<PDFObject[]> {
      if (current.length) yield current;
      current = next ? [next] : [];
    };
    for (const char of chars) {
      const text = char.text ?? "";
      if (!this.keepBlankChars && /^\s$/u.test(text)) {
        yield* flush();
      } else if (this.splitAtPunctuation.includes(text)) {
        yield* flush(char);
        yield* flush();
      } else if (
        current.length &&
        this.charBeginsNewWord(
          current[current.length - 1],
          char,
          direction,
          this.xToleranceRatio == null ? this.xTolerance : this.xToleranceRatio * Number(current[current.length - 1].size),
          this.yToleranceRatio == null ? this.yTolerance : this.yToleranceRatio * Number(current[current.length - 1].size)
        )
      ) {
        yield* flush(char);
      } else {
        current.push(char);
      }
    }
    if (current.length) yield current;
  }

  *iterCharsToLines(chars: PDFObject[]): Generator<[PDFObject[], Dir]> {
    const upright = Boolean(chars[0].upright);
    const lineDir = upright ? this.lineDir : this.lineDirRotated;
    const charDir = this.getCharDir(upright);
    const lineClusterKey = getLineClusterKey(lineDir);
    const charSortKey = getCharSortKey(charDir);
    const subclusters = clusterObjectsSimple(chars, lineClusterKey, lineDir === "ttb" || lineDir === "btt" ? this.yTolerance : this.xTolerance);
    for (const cluster of subclusters) {
      const sorted = [...cluster].sort((a, b) => {
        const ak = charSortKey(a);
        const bk = charSortKey(b);
        return ak[0] - bk[0] || ak[1] - bk[1];
      });
      yield [sorted, charDir];
    }
  }

  *iterExtractTuples(chars: PDFObject[]): Generator<[PDFObject, PDFObject[]]> {
    const groups: PDFObject[][] = [];
    let lastKey: string | null = null;
    for (const char of chars) {
      const key = [char.upright, ...this.extraAttrs.map((attr) => char[attr])].join("\u0000");
      if (key !== lastKey) {
        groups.push([char]);
        lastKey = key;
      } else {
        groups[groups.length - 1].push(char);
      }
    }
    for (const group of groups) {
      const lineGroups = this.useTextFlow ? ([[group, this.charDir]] as Array<[PDFObject[], Dir]>) : Array.from(this.iterCharsToLines(group));
      for (const [lineChars, direction] of lineGroups) {
        for (const wordChars of this.iterCharsToWords(lineChars, direction)) {
          yield [this.mergeChars(wordChars), wordChars];
        }
      }
    }
  }

  extractWordMap(chars: PDFObject[]): WordMap {
    return new WordMap(Array.from(this.iterExtractTuples(chars)));
  }

  extractWords(chars: PDFObject[], returnChars = false): PDFObject[] {
    return Array.from(this.iterExtractTuples(chars)).map(([word, wordChars]) => (returnChars ? { ...word, chars: wordChars } : word));
  }
}

function charsToTextMap(chars: PDFObject[], options: Record<string, unknown> = {}): TextMap {
  if (!chars.length) return new TextMap([], "ttb", "ltr");
  const fullOptions = {
    ...options,
    presorted: true,
    layout_bbox: (options.layout_bbox as BBox | undefined) ?? objectsToBBox(chars)
  };
  const extractor = new WordExtractor(fullOptions);
  return extractor.extractWordMap(chars).toTextMap(fullOptions);
}

function extractTextFromChars(chars: PDFObject[], options: Record<string, unknown> = {}): string {
  if (!chars.length) return "";
  if (options.layout) return charsToTextMap(chars, options).as_string;
  const extractor = new WordExtractor(options);
  const words = extractor.extractWords(chars);
  const lineDirRender = (options.line_dir_render as Dir | undefined) ?? extractor.lineDir;
  const charDirRender = (options.char_dir_render as Dir | undefined) ?? extractor.charDir;
  const lineClusterKey = getLineClusterKey(extractor.lineDir);
  const xTolerance = Number(options.x_tolerance ?? DEFAULT_X_TOLERANCE);
  const yTolerance = Number(options.y_tolerance ?? DEFAULT_Y_TOLERANCE);
  const lines = clusterObjectsSimple(words, lineClusterKey, lineDirRender === "ttb" || lineDirRender === "btt" ? yTolerance : xTolerance);
  return new TextMap(
    [...lines.map((line) => line.map((word) => word.text).join(" ")).join("\n")].map((char) => [char, null]),
    lineDirRender,
    charDirRender
  ).as_string;
}

function dedupeChars(chars: PDFObject[], tolerance = 1, extraAttrs: string[] = ["fontname", "size"]): PDFObject[] {
  const sorted = [...chars].sort((a, b) => {
    const ak = [a.upright, a.text, ...extraAttrs.map((attr) => a[attr])].join("\u0000");
    const bk = [b.upright, b.text, ...extraAttrs.map((attr) => b[attr])].join("\u0000");
    return ak.localeCompare(bk);
  });
  const unique: PDFObject[] = [];
  let i = 0;
  while (i < sorted.length) {
    const key = [sorted[i].upright, sorted[i].text, ...extraAttrs.map((attr) => sorted[i][attr])].join("\u0000");
    const group: PDFObject[] = [];
    while (i < sorted.length && [sorted[i].upright, sorted[i].text, ...extraAttrs.map((attr) => sorted[i][attr])].join("\u0000") === key) {
      group.push(sorted[i]);
      i += 1;
    }
    for (const yCluster of clusterObjectsSimple(group, (char) => Number(char.doctop), tolerance)) {
      for (const xCluster of clusterObjectsSimple(yCluster, (char) => Number(char.x0), tolerance)) {
        unique.push([...xCluster].sort((a, b) => Number(a.doctop) - Number(b.doctop) || Number(a.x0) - Number(b.x0))[0]);
      }
    }
  }
  return unique.sort((a, b) => chars.indexOf(a) - chars.indexOf(b));
}

const PDF_PASSWORD_PADDING = Uint8Array.from([
  0x28, 0xbf, 0x4e, 0x5e, 0x4e, 0x75, 0x8a, 0x41,
  0x64, 0x00, 0x4e, 0x56, 0xff, 0xfa, 0x01, 0x08,
  0x2e, 0x2e, 0x00, 0xb6, 0xd0, 0x68, 0x3e, 0x80,
  0x2f, 0x0c, 0xa9, 0xfe, 0x64, 0x53, 0x69, 0x7a
]);

function latin1Bytes(value: string): Uint8Array {
  const bytes = new Uint8Array(value.length);
  for (let i = 0; i < value.length; i += 1) bytes[i] = value.charCodeAt(i) & 0xff;
  return bytes;
}

function latin1String(bytes: Uint8Array): string {
  let out = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) out += String.fromCharCode(...bytes.subarray(i, i + chunk));
  return out;
}

function hexBytes(value: string): Uint8Array {
  const clean = value.replace(/\s+/g, "");
  const padded = clean.length % 2 ? `${clean}0` : clean;
  const bytes = new Uint8Array(padded.length / 2);
  for (let i = 0; i < bytes.length; i += 1) bytes[i] = parseInt(padded.slice(i * 2, i * 2 + 2), 16);
  return bytes;
}

function concatBytes(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

function int32LittleEndian(value: number): Uint8Array {
  const out = new Uint8Array(4);
  const unsigned = value >>> 0;
  out[0] = unsigned & 0xff;
  out[1] = (unsigned >>> 8) & 0xff;
  out[2] = (unsigned >>> 16) & 0xff;
  out[3] = (unsigned >>> 24) & 0xff;
  return out;
}

function pdfPasswordBytes(password: string): Uint8Array {
  const raw = latin1Bytes(password);
  const out = new Uint8Array(32);
  const used = Math.min(raw.length, 32);
  out.set(raw.subarray(0, used));
  if (used < 32) out.set(PDF_PASSWORD_PADDING.subarray(0, 32 - used), used);
  return out;
}

function md5Bytes(input: Uint8Array): Uint8Array {
  const s = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
    5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
    4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
    6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21
  ];
  const k = Array.from({ length: 64 }, (_, i) => Math.floor(Math.abs(Math.sin(i + 1)) * 0x100000000) >>> 0);
  const bitLength = input.length * 8;
  const paddedLength = (((input.length + 8) >>> 6) + 1) << 6;
  const padded = new Uint8Array(paddedLength);
  padded.set(input);
  padded[input.length] = 0x80;
  let low = bitLength >>> 0;
  let high = Math.floor(bitLength / 0x100000000) >>> 0;
  for (let i = 0; i < 4; i += 1) {
    padded[paddedLength - 8 + i] = low & 0xff;
    low >>>= 8;
    padded[paddedLength - 4 + i] = high & 0xff;
    high >>>= 8;
  }

  let a0 = 0x67452301;
  let b0 = 0xefcdab89;
  let c0 = 0x98badcfe;
  let d0 = 0x10325476;
  const m = new Uint32Array(16);
  const leftRotate = (value: number, amount: number): number => ((value << amount) | (value >>> (32 - amount))) >>> 0;

  for (let offset = 0; offset < padded.length; offset += 64) {
    for (let i = 0; i < 16; i += 1) {
      const j = offset + i * 4;
      m[i] = (padded[j] | (padded[j + 1] << 8) | (padded[j + 2] << 16) | (padded[j + 3] << 24)) >>> 0;
    }
    let a = a0;
    let b = b0;
    let c = c0;
    let d = d0;
    for (let i = 0; i < 64; i += 1) {
      let f: number;
      let g: number;
      if (i < 16) {
        f = (b & c) | (~b & d);
        g = i;
      } else if (i < 32) {
        f = (d & b) | (~d & c);
        g = (5 * i + 1) % 16;
      } else if (i < 48) {
        f = b ^ c ^ d;
        g = (3 * i + 5) % 16;
      } else {
        f = c ^ (b | ~d);
        g = (7 * i) % 16;
      }
      const temp = d;
      d = c;
      c = b;
      b = (b + leftRotate((a + f + k[i] + m[g]) >>> 0, s[i])) >>> 0;
      a = temp;
    }
    a0 = (a0 + a) >>> 0;
    b0 = (b0 + b) >>> 0;
    c0 = (c0 + c) >>> 0;
    d0 = (d0 + d) >>> 0;
  }

  const out = new Uint8Array(16);
  for (const [index, value] of [a0, b0, c0, d0].entries()) {
    out[index * 4] = value & 0xff;
    out[index * 4 + 1] = (value >>> 8) & 0xff;
    out[index * 4 + 2] = (value >>> 16) & 0xff;
    out[index * 4 + 3] = (value >>> 24) & 0xff;
  }
  return out;
}

function rc4Bytes(data: Uint8Array, key: Uint8Array): Uint8Array {
  const s = new Uint8Array(256);
  for (let i = 0; i < 256; i += 1) s[i] = i;
  let j = 0;
  for (let i = 0; i < 256; i += 1) {
    j = (j + s[i] + key[i % key.length]) & 0xff;
    [s[i], s[j]] = [s[j], s[i]];
  }
  const out = new Uint8Array(data.length);
  let i = 0;
  j = 0;
  for (let n = 0; n < data.length; n += 1) {
    i = (i + 1) & 0xff;
    j = (j + s[i]) & 0xff;
    [s[i], s[j]] = [s[j], s[i]];
    out[n] = data[n] ^ s[(s[i] + s[j]) & 0xff];
  }
  return out;
}

function parsePdfLiteralStringBytes(source: string, start: number): { bytes: Uint8Array; end: number } | null {
  if (source[start] !== "(") return null;
  const out: number[] = [];
  let depth = 1;
  let index = start + 1;
  while (index < source.length && depth > 0) {
    const char = source[index++];
    if (char === "\\") {
      if (index >= source.length) break;
      const escaped = source[index++];
      if (escaped === "n") out.push(0x0a);
      else if (escaped === "r") out.push(0x0d);
      else if (escaped === "t") out.push(0x09);
      else if (escaped === "b") out.push(0x08);
      else if (escaped === "f") out.push(0x0c);
      else if (escaped === "\r" || escaped === "\n") {
        if (escaped === "\r" && source[index] === "\n") index += 1;
      } else if (/[0-7]/.test(escaped)) {
        let octal = escaped;
        for (let i = 0; i < 2 && /[0-7]/.test(source[index] ?? ""); i += 1) octal += source[index++];
        out.push(parseInt(octal, 8) & 0xff);
      } else {
        out.push(escaped.charCodeAt(0) & 0xff);
      }
    } else if (char === "(") {
      depth += 1;
      out.push(0x28);
    } else if (char === ")") {
      depth -= 1;
      if (depth > 0) out.push(0x29);
    } else {
      out.push(char.charCodeAt(0) & 0xff);
    }
  }
  return depth === 0 ? { bytes: Uint8Array.from(out), end: index } : null;
}

function parsePdfDictBytes(source: string, key: string): Uint8Array | null {
  const match = new RegExp(`/${key}\\b`).exec(source);
  if (!match) return null;
  let index = match.index + match[0].length;
  while (/\s/.test(source[index] ?? "")) index += 1;
  if (source[index] === "(") return parsePdfLiteralStringBytes(source, index)?.bytes ?? null;
  if (source[index] === "<" && source[index + 1] !== "<") {
    const end = source.indexOf(">", index + 1);
    return end >= 0 ? hexBytes(source.slice(index + 1, end)) : null;
  }
  return null;
}

function parseFirstFileId(raw: string): Uint8Array | null {
  const hex = raw.match(/\/ID\s*\[\s*<([0-9A-Fa-f\s]+)>/)?.[1];
  if (hex) return hexBytes(hex);
  const literalStart = raw.search(/\/ID\s*\[\s*\(/);
  if (literalStart >= 0) {
    const open = raw.indexOf("(", literalStart);
    return open >= 0 ? parsePdfLiteralStringBytes(raw, open)?.bytes ?? null : null;
  }
  return null;
}

function parseCryptFilterMethod(encryptText: string, filterName: string | undefined, fallback: PdfEncryption["streamMethod"]): PdfEncryption["streamMethod"] {
  if (!filterName || filterName === "Identity") return filterName === "Identity" ? "Identity" : fallback;
  const match = new RegExp(`/${filterName}\\s*<<([\\s\\S]*?)>>`).exec(encryptText);
  const cfm = match?.[1].match(/\/CFM\s*\/([^\s/<>[\]()]+)/)?.[1];
  if (!cfm || cfm === "None" || cfm === "Identity") return cfm ? "Identity" : fallback;
  if (cfm === "V2") return "RC4";
  return "Unsupported";
}

function parsePdfEncryption(raw: string, objects: Map<number, string>, password = ""): PdfEncryption | null {
  const encryptRef = Number(raw.match(/\/Encrypt\s+(\d+)\s+\d+\s+R/)?.[1] ?? Number.NaN);
  if (!Number.isFinite(encryptRef)) return null;
  const encryptText = objects.get(encryptRef) ?? "";
  if (!/\/Filter\s*\/Standard\b/.test(encryptText)) return null;
  const owner = parsePdfDictBytes(encryptText, "O");
  const permissions = Number(encryptText.match(/\/P\s+(-?\d+)/)?.[1] ?? Number.NaN);
  const revision = Number(encryptText.match(/\/R\s+(\d+)/)?.[1] ?? 0);
  const version = Number(encryptText.match(/\/V\s+(\d+)/)?.[1] ?? 0);
  const lengthValues = Array.from(encryptText.matchAll(/\/Length\s+(\d+)/g), (match) => Number(match[1]));
  const lengthBits = lengthValues.length ? Math.max(...lengthValues) : (version === 1 ? 40 : 40);
  const fileId = parseFirstFileId(raw);
  if (!owner || !fileId || !Number.isFinite(permissions) || revision >= 5) return null;
  const keyLength = Math.max(5, Math.min(16, Math.floor(lengthBits / 8) || 5));
  const encryptMetadata = !/\/EncryptMetadata\s+false\b/.test(encryptText);
  let hashInput = concatBytes(pdfPasswordBytes(password), owner, int32LittleEndian(permissions), fileId);
  if (revision >= 4 && !encryptMetadata) hashInput = concatBytes(hashInput, Uint8Array.of(0xff, 0xff, 0xff, 0xff));
  let hash = md5Bytes(hashInput);
  if (revision >= 3) {
    for (let i = 0; i < 50; i += 1) hash = md5Bytes(hash.subarray(0, keyLength));
  }
  const defaultMethod: PdfEncryption["streamMethod"] = version === 1 || version === 2 ? "RC4" : "Unsupported";
  const stmf = encryptText.match(/\/StmF\s*\/([^\s/<>[\]()]+)/)?.[1];
  return {
    encryptRef,
    key: hash.subarray(0, keyLength),
    keyLength,
    streamMethod: version === 4 ? parseCryptFilterMethod(encryptText, stmf, defaultMethod) : defaultMethod,
    encryptMetadata
  };
}

function pdfObjectRefFromText(objectText: string): { num: number; gen: number } | null {
  const match = objectText.match(/^\s*(\d+)\s+(\d+)\s+obj\b/);
  return match ? { num: Number(match[1]), gen: Number(match[2]) } : null;
}

function pdfObjectKey(encryption: PdfEncryption, num: number, gen: number): Uint8Array {
  const extra = new Uint8Array(5);
  extra[0] = num & 0xff;
  extra[1] = (num >>> 8) & 0xff;
  extra[2] = (num >>> 16) & 0xff;
  extra[3] = gen & 0xff;
  extra[4] = (gen >>> 8) & 0xff;
  return md5Bytes(concatBytes(encryption.key, extra)).subarray(0, Math.min(encryption.keyLength + 5, 16));
}

function decryptPdfStreamObject(objectText: string, encryption: PdfEncryption | null): string {
  if (!encryption || encryption.streamMethod !== "RC4") return objectText;
  const ref = pdfObjectRefFromText(objectText);
  if (!ref || ref.num === encryption.encryptRef || /\/Type\s*\/XRef\b/.test(objectText)) return objectText;
  if (!encryption.encryptMetadata && /\/Type\s*\/Metadata\b/.test(objectText)) return objectText;
  const streamIndex = objectText.indexOf("stream");
  const endstreamIndex = objectText.lastIndexOf("endstream");
  if (streamIndex === -1 || endstreamIndex === -1 || endstreamIndex <= streamIndex) return objectText;

  let start = streamIndex + "stream".length;
  if (objectText[start] === "\r" && objectText[start + 1] === "\n") start += 2;
  else if (objectText[start] === "\r" || objectText[start] === "\n") start += 1;

  let end = endstreamIndex;
  if (objectText[end - 2] === "\r" && objectText[end - 1] === "\n") end -= 2;
  else if (objectText[end - 1] === "\r" || objectText[end - 1] === "\n") end -= 1;

  const decrypted = rc4Bytes(latin1Bytes(objectText.slice(start, end)), pdfObjectKey(encryption, ref.num, ref.gen));
  return `${objectText.slice(0, start)}${latin1String(decrypted)}${objectText.slice(end)}`;
}

function parsePdfObjects(raw: string, password = ""): Map<number, string> {
  const objects = new Map<number, string>();
  const re = /(?:^|[\r\n])(\d+)\s+(\d+)\s+obj\b/g;
  let match: RegExpExecArray | null;
  while ((match = re.exec(raw))) {
    const start = match.index + (match[0].match(/^[\r\n]/) ? 1 : 0);
    const end = raw.indexOf("endobj", re.lastIndex);
    if (end === -1) continue;
    objects.set(Number(match[1]), raw.slice(start, end + "endobj".length));
    re.lastIndex = end + "endobj".length;
  }
  const encryption = parsePdfEncryption(raw, objects, password);
  if (encryption) {
    for (const [objectNumber, text] of objects) objects.set(objectNumber, decryptPdfStreamObject(text, encryption));
  }
  for (const text of [...objects.values()]) {
    if (!/\/Type\s*\/ObjStm\b/.test(text)) continue;
    const first = Number(text.match(/\/First\s+(\d+)/)?.[1] ?? Number.NaN);
    const count = Number(text.match(/\/N\s+(\d+)/)?.[1] ?? Number.NaN);
    if (!Number.isFinite(first) || !Number.isFinite(count)) continue;
    const decoded = decodePdfStream(text);
    if (!decoded || decoded.length <= first) continue;
    const header = decoded.slice(0, first);
    const values = parseNumbers(header);
    if (values.length < count * 2) continue;
    const body = decoded.slice(first);
    const entries = Array.from({ length: count }, (_, index) => ({
      objectNumber: values[index * 2],
      offset: values[index * 2 + 1]
    })).sort((a, b) => a.offset - b.offset);
    for (let i = 0; i < entries.length; i += 1) {
      const entry = entries[i];
      const end = entries[i + 1]?.offset ?? body.length;
      if (objects.has(entry.objectNumber)) continue;
      objects.set(entry.objectNumber, `${entry.objectNumber} 0 obj\n${body.slice(entry.offset, end).trim()}\nendobj`);
    }
  }
  return objects;
}

function decodePdfStream(objectText: string | undefined): string {
  if (!objectText) return "";
  const streamIndex = objectText.indexOf("stream");
  const endstreamIndex = objectText.lastIndexOf("endstream");
  if (streamIndex === -1 || endstreamIndex === -1 || endstreamIndex <= streamIndex) return "";

  let start = streamIndex + "stream".length;
  if (objectText[start] === "\r" && objectText[start + 1] === "\n") start += 2;
  else if (objectText[start] === "\r" || objectText[start] === "\n") start += 1;

  let end = endstreamIndex;
  if (objectText[end - 2] === "\r" && objectText[end - 1] === "\n") end -= 2;
  else if (objectText[end - 1] === "\r" || objectText[end - 1] === "\n") end -= 1;

  let bytes: Buffer<ArrayBufferLike> = Buffer.from(objectText.slice(start, end), "latin1");
  if (/\/ASCII85Decode\b/.test(objectText)) bytes = ascii85Decode(bytes.toString("latin1"));
  if (/\/FlateDecode\b/.test(objectText)) {
    try {
      return inflateSync(bytes).toString("latin1");
    } catch {
      return "";
    }
  }
  return bytes.toString("latin1");
}

function stripPdfObject(text: string): string {
  return text.replace(/^\s*\d+\s+\d+\s+obj\s*/, "").replace(/\s*endobj\s*$/, "").trim();
}

function ascii85Decode(value: string): Buffer<ArrayBufferLike> {
  const out: number[] = [];
  let group: number[] = [];
  const flush = (partial = false): void => {
    if (!group.length) return;
    const length = group.length;
    if (partial) {
      while (group.length < 5) group.push(84);
    }
    let acc = 0;
    for (const code of group) acc = acc * 85 + code;
    const bytes = [(acc >>> 24) & 0xff, (acc >>> 16) & 0xff, (acc >>> 8) & 0xff, acc & 0xff];
    out.push(...bytes.slice(0, partial ? length - 1 : 4));
    group = [];
  };
  for (let i = 0; i < value.length; i += 1) {
    const char = value[i];
    if (/\s/.test(char)) continue;
    if (char === "~") break;
    if (char === "z" && group.length === 0) {
      out.push(0, 0, 0, 0);
      continue;
    }
    const code = char.charCodeAt(0) - 33;
    if (code < 0 || code > 84) continue;
    group.push(code);
    if (group.length === 5) flush();
  }
  flush(true);
  return Buffer.from(out);
}

function xObjectEntries(ownerText: string | undefined, objects: Map<number, string>): Array<{ name: string; objectNumber: number }> {
  if (!ownerText) return [];
  const sources: string[] = [ownerText];
  const resourceRef = ownerText.match(/\/Resources\s+(\d+)\s+\d+\s+R/)?.[1];
  const resourceText = resourceRef ? objects.get(Number(resourceRef)) : undefined;
  if (resourceText) sources.push(resourceText);

  const xObjectRefs: string[] = [];
  for (const source of sources) {
    const ref = source.match(/\/XObject\s+(\d+)\s+\d+\s+R/)?.[1];
    if (ref) xObjectRefs.push(ref);
  }
  for (const ref of xObjectRefs) {
    const objectText = objects.get(Number(ref));
    if (objectText) sources.push(stripPdfObject(objectText));
  }

  const entries: Array<{ name: string; objectNumber: number }> = [];
  const addEntries = (body: string): void => {
    const entryRe = /\/([^\s/<>[\]()]+)\s+(\d+)\s+\d+\s+R/g;
    let entry: RegExpExecArray | null;
    while ((entry = entryRe.exec(body))) entries.push({ name: entry[1], objectNumber: Number(entry[2]) });
  };
  for (const source of sources) {
    const direct = source.match(/\/XObject\s*<<([\s\S]*?)>>/);
    if (direct) addEntries(direct[1]);
    else if (!/\/XObject\b/.test(source) && /^\s*<</.test(source)) addEntries(source);
  }
  return entries;
}

function expandFormXObjects(content: string, ownerText: string | undefined, objects: Map<number, string>, depth = 0): string {
  if (!content || depth > 4) return content;
  const forms = new Map<string, string>();
  for (const entry of xObjectEntries(ownerText, objects)) {
    const objectText = objects.get(entry.objectNumber);
    if (objectText && /\/Subtype\s*\/Form\b/.test(objectText)) forms.set(entry.name, objectText);
  }
  if (!forms.size) return content;
  return content.replace(/\/([^\s/<>[\]()]+)\s+Do\b/g, (match, name: string) => {
    const form = forms.get(name);
    if (!form) return match;
    const formContent = expandFormXObjects(decodePdfStream(form), form, objects, depth + 1);
    const matrix = form.match(/\/Matrix\s*\[([^\]]+)\]/)?.[1];
    return matrix ? `q\n${matrix} cm\n${formContent}\nQ` : formContent;
  });
}

function extractPageContent(pageObjectText: string | undefined, objects: Map<number, string>): string {
  if (!pageObjectText) return "";
  const contentRefs = (source: string | undefined, seen = new Set<number>()): number[] => {
    if (!source) return [];
    const arrayMatch = source.match(/\/Contents\s*\[([\s\S]*?)\]/);
    if (arrayMatch) return Array.from(arrayMatch[1].matchAll(/(\d+)\s+\d+\s+R/g), (match) => Number(match[1]));
    const refMatch = source.match(/\/Contents\s+(\d+)\s+\d+\s+R/);
    if (!refMatch) return [];
    const ref = Number(refMatch[1]);
    if (seen.has(ref)) return [];
    seen.add(ref);
    const objectText = objects.get(ref);
    const stripped = objectText ? stripPdfObject(objectText) : "";
    if (/^\s*\[/.test(stripped)) return Array.from(stripped.matchAll(/(\d+)\s+\d+\s+R/g), (match) => Number(match[1]));
    if (!/\bstream\b/.test(stripped)) return contentRefs(`/Contents ${stripped}`, seen);
    return [ref];
  };
  const content = contentRefs(pageObjectText)
    .map((ref) => decodePdfStream(objects.get(ref)))
    .join("\n");
  return expandFormXObjects(content, pageObjectText, objects);
}

function parseImageResources(pageObjectText: string | undefined, objects: Map<number, string>, pageContent = ""): ImageResource[] {
  const numberAttr = (objectText: string, name: string): number | undefined => {
    const indirect = objectText.match(new RegExp(`/${name}\\s+(\\d+)\\s+\\d+\\s+R`));
    if (indirect) {
      const resolved = objects.get(Number(indirect[1]))?.match(/\bobj\s+([-+]?(?:\d+\.\d+|\d+|\.\d+))/);
      if (resolved) return Number(resolved[1]);
    }
    const direct = objectText.match(new RegExp(`/${name}\\s+([-+]?(?:\\d+\\.\\d+|\\d+|\\.\\d+))`));
    return direct ? Number(direct[1]) : undefined;
  };
  const resolvedLength = (objectText: string): number | undefined => {
    const direct = objectText.match(/\/Length\s+(\d+)\b(?!\s+\d+\s+R)/)?.[1];
    if (direct) return Number(direct);
    const ref = objectText.match(/\/Length\s+(\d+)\s+\d+\s+R/)?.[1];
    const resolved = ref ? objects.get(Number(ref))?.match(/\bobj\s+(\d+)/)?.[1] : undefined;
    return resolved ? Number(resolved) : undefined;
  };
  const pdfValueRepr = (objectText: string, name: string): string | number | undefined => {
    const ref = objectText.match(new RegExp(`/${name}\\s+(\\d+)\\s+\\d+\\s+R`))?.[1];
    if (ref) return `<PDFObjRef:${ref}>`;
    const number = objectText.match(new RegExp(`/${name}\\s+([-+]?(?:\\d+\\.\\d+|\\d+|\\.\\d+))`))?.[1];
    if (number) return Number(number);
    const named = objectText.match(new RegExp(`/${name}\\s*/([^\\s/<>[\\]()]+)`))?.[1];
    if (named) return `/'${named}'`;
    return undefined;
  };
  const pdfStreamRepr = (objectNumber: number, objectText: string): string => {
    const raw = resolvedLength(objectText) ?? decodePdfStream(objectText).length;
    const attrs: string[] = [];
    const dictText = objectText.slice(0, objectText.search(/\bstream\b/));
    const keys = Array.from(dictText.matchAll(/\/(Length|N|Alternate|Filter)\b/g), (match) => match[1])
      .filter((key, index, all) => all.indexOf(key) === index);
    for (const key of keys) {
      const value = pdfValueRepr(objectText, key);
      if (value !== undefined) attrs.push(`'${key}': ${typeof value === "string" ? value : String(value)}`);
    }
    return `<PDFStream(${objectNumber}): raw=${raw}, {${attrs.join(", ")}}>`;
  };
  const parseColorSpaceArray = (body: string): unknown[] => {
    const values: unknown[] = [];
    const tokenRe = /(\d+)\s+\d+\s+R|\/([^\s/<>[\]()]+)|[-+]?(?:\d+\.\d+|\d+|\.\d+)/g;
    let token: RegExpExecArray | null;
    while ((token = tokenRe.exec(body))) {
      if (token[1]) {
        const objectNumber = Number(token[1]);
        const referenced = objects.get(objectNumber) ?? "";
        const body = referenced.replace(/^\d+\s+\d+\s+obj\s*/, "").replace(/\s*endobj$/, "").trim();
        const array = body.match(/^\[([\s\S]*)\]$/);
        values.push(/\bstream\b/.test(referenced) ? pdfStreamRepr(objectNumber, referenced) : array ? parseColorSpaceArray(array[1]) : cleanObject({ value: body }).value);
      } else if (token[2]) {
        values.push(`/'${token[2]}'`);
      } else {
        values.push(Number(token[0]));
      }
    }
    return values;
  };
  const colorSpaceAttr = (objectText: string): unknown[] | undefined => {
    const directArray = objectText.match(/\/ColorSpace\s*\[([\s\S]*?)\]/);
    if (directArray) return parseColorSpaceArray(directArray[1]);
    const directName = objectText.match(/\/ColorSpace\s*\/([^\s/<>[\]()]+)/)?.[1];
    if (directName) return [`/'${directName}'`];
    const colorspaceRef = objectText.match(/\/ColorSpace\s+(\d+)\s+\d+\s+R/)?.[1];
    if (!colorspaceRef) return undefined;
    const referenced = objects.get(Number(colorspaceRef)) ?? "";
    const referencedArray = referenced.match(/\bobj\s*\[([\s\S]*?)\]\s*endobj/);
    if (referencedArray) return [parseColorSpaceArray(referencedArray[1])];
    const referencedName = referenced.match(/\bobj\s*\/([^\s/<>[\]()]+)/)?.[1] ?? referenced.match(/\/(DeviceRGB|DeviceGray|DeviceCMYK)\b/)?.[1];
    return referencedName ? [`/'${referencedName}'`] : undefined;
  };
  const resourceByName = new Map<string, ImageResource>();
  const out: ImageResource[] = [];
  const addResource = (resource: ImageResource): void => {
    if (resourceByName.has(resource.name)) return;
    resourceByName.set(resource.name, resource);
    out.push(resource);
  };
  const resourceRef = pageObjectText?.match(/\/Resources\s+(\d+)\s+\d+\s+R/)?.[1];
  const resourceObject = resourceRef ? objects.get(Number(resourceRef)) : undefined;
  const xObjectRef = (pageObjectText ?? resourceObject)?.match(/\/XObject\s+(\d+)\s+\d+\s+R/)?.[1];
  const xObjectObject = xObjectRef ? objects.get(Number(xObjectRef)) : undefined;
  const sources = [
    pageObjectText ?? "",
    resourceObject ?? "",
    xObjectObject ? `/XObject ${xObjectObject.replace(/^\d+\s+\d+\s+obj\s*/, "").replace(/\s*endobj$/, "")}` : "",
    ...objects.values()
  ];
  for (const source of sources) {
    const match = source.match(/\/XObject\s*<<([\s\S]*?)>>/);
    if (!match) continue;
    const entryRe = /\/([^\s/<>[\]()]+)\s+(\d+)\s+\d+\s+R/g;
    let entry: RegExpExecArray | null;
    while ((entry = entryRe.exec(match[1]))) {
      const name = entry[1];
      if (out.some((image) => image.name === name)) continue;
      const objectText = objects.get(Number(entry[2])) ?? "";
      if (!/\/Subtype\s*\/Image\b/.test(objectText) && objectText) continue;
      const width = numberAttr(objectText, "Width");
      const height = numberAttr(objectText, "Height");
      const bits = numberAttr(objectText, "BitsPerComponent");
      addResource({
        name,
        width,
        height,
        bits,
        colorspace: colorSpaceAttr(objectText) ?? [null]
      });
    }
    if (out.length) break;
  }
  const drawNames = Array.from(pageContent.matchAll(/\/([^\s/<>[\]()]+)\s+Do\b/g), (match) => match[1]);
  return drawNames.length ? drawNames.map((name) => resourceByName.get(name) ?? { name }) : out;
}

function parseColorSpaceResources(pageObjectText: string | undefined, objects: Map<number, string>): Record<string, string> {
  const out: Record<string, string> = {};
  const collectSources = (ownerText: string | undefined, seen = new Set<number>(), depth = 0): string[] => {
    if (!ownerText || depth > 4) return [];
    const sources = [ownerText];
    const resourceRef = ownerText.match(/\/Resources\s+(\d+)\s+\d+\s+R/)?.[1];
    const resourceText = resourceRef ? objects.get(Number(resourceRef)) : undefined;
    if (resourceText) sources.push(resourceText);
    for (const entry of xObjectEntries(ownerText, objects)) {
      if (seen.has(entry.objectNumber)) continue;
      seen.add(entry.objectNumber);
      const objectText = objects.get(entry.objectNumber);
      if (!objectText || !/\/Subtype\s*\/Form\b/.test(objectText)) continue;
      sources.push(objectText, ...collectSources(objectText, seen, depth + 1));
    }
    return sources;
  };
  const sources = collectSources(pageObjectText);
  for (const source of sources) {
    const match = source.match(/\/ColorSpace\s*<<([\s\S]*?)>>/);
    if (!match) continue;
    const body = match[1];
    const entryRe = /\/([^\s/<>[\]()]+)\s+(?:(\d+)\s+\d+\s+R|\[\s*\/([^\s/<>[\]()]+))/g;
    let entry: RegExpExecArray | null;
    while ((entry = entryRe.exec(body))) {
      const name = entry[1];
      const direct = entry[3];
      const referenced = entry[2] ? objects.get(Number(entry[2])) : undefined;
      const indexedBaseRef = referenced?.match(/\[\s*\/Indexed\s+(\d+)\s+\d+\s+R/)?.[1];
      const indexedBase = indexedBaseRef ? objects.get(Number(indexedBaseRef)) : undefined;
      out[name] = colorSpaceName(
        direct ??
          indexedBase?.match(/\/Separation\b/)?.[0].slice(1) ??
          referenced?.match(/\/Separation\b/)?.[0].slice(1) ??
          referenced?.match(/\[\s*\/([^\s/<>[\]()]+)/)?.[1] ??
          referenced?.match(/\/(DeviceRGB|DeviceGray|DeviceCMYK|ICCBased)\b/)?.[1] ??
          name
      );
    }
  }
  return out;
}

function parseColorOps(content: string, colorSpaces: Record<string, string> = {}): ColorOp[] {
  const ops: ColorOp[] = [];
  const operands: Array<number | { name: string }> = [];
  let fillColorSpace = "DeviceGray";
  let strokeColorSpace = "DeviceGray";
  let index = 0;

  const isWhite = (char: string) => /\s/.test(char);
  const isDelimiter = (char: string) => "()<>[]{}/%".includes(char);
  const skipString = (): void => {
    let depth = 1;
    index += 1;
    while (index < content.length && depth > 0) {
      const char = content[index++];
      if (char === "\\") {
        index += 1;
      } else if (char === "(") {
        depth += 1;
      } else if (char === ")") {
        depth -= 1;
      }
    }
  };
  const skipHexOrDict = (): boolean => {
    if (content[index + 1] === "<") {
      index += 2;
      return false;
    }
    index += 1;
    while (index < content.length && content[index] !== ">") index += 1;
    index += 1;
    return true;
  };
  const skipArray = (): void => {
    let depth = 1;
    index += 1;
    while (index < content.length && depth > 0) {
      const char = content[index];
      if (char === "(") skipString();
      else if (char === "<") skipHexOrDict();
      else {
        if (char === "[") depth += 1;
        else if (char === "]") depth -= 1;
        index += 1;
      }
    }
  };
  const readToken = (): string => {
    const start = index;
    while (index < content.length && !isWhite(content[index]) && !isDelimiter(content[index])) index += 1;
    return content.slice(start, index);
  };
  const resetOperands = (): void => {
    operands.length = 0;
  };

  while (index < content.length) {
    const char = content[index];
    if (isWhite(char)) {
      index += 1;
      continue;
    }
    if (char === "%") {
      while (index < content.length && content[index] !== "\n" && content[index] !== "\r") index += 1;
      continue;
    }
    if (char === "(") {
      skipString();
      continue;
    }
    if (char === "[") {
      skipArray();
      continue;
    }
    if (char === "<") {
      const skippedHex = skipHexOrDict();
      if (skippedHex) continue;
      resetOperands();
      continue;
    }
    if (char === ">") {
      index += content[index + 1] === ">" ? 2 : 1;
      resetOperands();
      continue;
    }
    if (char === "/") {
      index += 1;
      operands.push({ name: readToken() });
      continue;
    }

    const token = readToken();
    if (!token) {
      index += 1;
      continue;
    }
    if (/^[-+]?(?:\d+\.\d+|\d+|\.\d+)$/.test(token)) {
      operands.push(Number(token));
      continue;
    }

    const op = token;
    if (op === "cs" || op === "CS") {
      const name = [...operands].reverse().find((operand): operand is { name: string } => typeof operand === "object")?.name ?? "";
      const resolved = colorSpaceName(colorSpaces[name] ?? name);
      if (op === "cs") fillColorSpace = resolved;
      else strokeColorSpace = resolved;
      resetOperands();
      continue;
    }
    const components = operands.filter((operand): operand is number => typeof operand === "number");
    const isStroke = op === op.toUpperCase();
    let colorSpace = isStroke ? strokeColorSpace : fillColorSpace;
    if (op === "g" || op === "G") colorSpace = "DeviceGray";
    else if (op === "rg" || op === "RG") colorSpace = "DeviceRGB";
    else if (op === "k" || op === "K") colorSpace = "DeviceCMYK";
    if (["g", "G", "rg", "RG", "k", "K", "sc", "SC", "scn", "SCN"].includes(op)) {
      const pattern = op.endsWith("cn") ? [...operands].reverse().find((operand): operand is { name: string } => typeof operand === "object")?.name : undefined;
      ops.push({
        target: isStroke ? "stroke" : "fill",
        components,
        colorSpace,
        pattern
      });
      if (isStroke) strokeColorSpace = colorSpace;
      else fillColorSpace = colorSpace;
    }
    resetOperands();
  }
  return ops;
}

function parseTextMatrixOps(content: string): Matrix[] {
  const matrices: Matrix[] = [];
  const operands: number[] = [];
  let index = 0;

  const isWhite = (char: string): boolean => /\s/.test(char);
  const isDelimiter = (char: string): boolean => "()<>[]{}/%".includes(char);
  const skipString = (): void => {
    let depth = 1;
    index += 1;
    while (index < content.length && depth > 0) {
      const char = content[index++];
      if (char === "\\") index += 1;
      else if (char === "(") depth += 1;
      else if (char === ")") depth -= 1;
    }
  };
  const skipHexOrDict = (): void => {
    if (content[index + 1] === "<") {
      index += 2;
      return;
    }
    index += 1;
    while (index < content.length && content[index] !== ">") index += 1;
    index += 1;
  };
  const skipArray = (): void => {
    let depth = 1;
    index += 1;
    while (index < content.length && depth > 0) {
      const char = content[index];
      if (char === "(") skipString();
      else if (char === "<") skipHexOrDict();
      else {
        if (char === "[") depth += 1;
        else if (char === "]") depth -= 1;
        index += 1;
      }
    }
  };
  const readToken = (): string => {
    const start = index;
    while (index < content.length && !isWhite(content[index]) && !isDelimiter(content[index])) index += 1;
    return content.slice(start, index);
  };

  while (index < content.length) {
    const char = content[index];
    if (isWhite(char)) {
      index += 1;
      continue;
    }
    if (char === "%") {
      while (index < content.length && content[index] !== "\n" && content[index] !== "\r") index += 1;
      continue;
    }
    if (char === "(") {
      skipString();
      continue;
    }
    if (char === "[") {
      skipArray();
      operands.length = 0;
      continue;
    }
    if (char === "<") {
      skipHexOrDict();
      operands.length = 0;
      continue;
    }
    if (char === ">") {
      index += content[index + 1] === ">" ? 2 : 1;
      operands.length = 0;
      continue;
    }
    if (char === "/") {
      index += 1;
      readToken();
      operands.length = 0;
      continue;
    }

    const token = readToken();
    if (!token) {
      index += 1;
      continue;
    }
    if (/^[-+]?(?:\d+\.\d+|\d+|\.\d+)$/.test(token)) {
      operands.push(Number(token));
      continue;
    }
    if (token === "Tm" && operands.length >= 6) matrices.push(operands.slice(-6) as Matrix);
    operands.length = 0;
  }

  return matrices;
}

function parseTextMoveOps(content: string): { move: Point[]; leadingMove: Point[] } {
  const move: Point[] = [];
  const leadingMove: Point[] = [];
  const operands: number[] = [];
  let index = 0;

  const isWhite = (char: string): boolean => /\s/.test(char);
  const isDelimiter = (char: string): boolean => "()<>[]{}/%".includes(char);
  const skipString = (): void => {
    let depth = 1;
    index += 1;
    while (index < content.length && depth > 0) {
      const char = content[index++];
      if (char === "\\") index += 1;
      else if (char === "(") depth += 1;
      else if (char === ")") depth -= 1;
    }
  };
  const skipHexOrDict = (): void => {
    if (content[index + 1] === "<") {
      index += 2;
      return;
    }
    index += 1;
    while (index < content.length && content[index] !== ">") index += 1;
    index += 1;
  };
  const skipArray = (): void => {
    let depth = 1;
    index += 1;
    while (index < content.length && depth > 0) {
      const char = content[index];
      if (char === "(") skipString();
      else if (char === "<") skipHexOrDict();
      else {
        if (char === "[") depth += 1;
        else if (char === "]") depth -= 1;
        index += 1;
      }
    }
  };
  const readToken = (): string => {
    const start = index;
    while (index < content.length && !isWhite(content[index]) && !isDelimiter(content[index])) index += 1;
    return content.slice(start, index);
  };

  while (index < content.length) {
    const char = content[index];
    if (isWhite(char)) {
      index += 1;
      continue;
    }
    if (char === "%") {
      while (index < content.length && content[index] !== "\n" && content[index] !== "\r") index += 1;
      continue;
    }
    if (char === "(") {
      skipString();
      continue;
    }
    if (char === "[") {
      skipArray();
      operands.length = 0;
      continue;
    }
    if (char === "<") {
      skipHexOrDict();
      operands.length = 0;
      continue;
    }
    if (char === ">") {
      index += content[index + 1] === ">" ? 2 : 1;
      operands.length = 0;
      continue;
    }
    if (char === "/") {
      index += 1;
      readToken();
      operands.length = 0;
      continue;
    }

    const token = readToken();
    if (!token) {
      index += 1;
      continue;
    }
    if (/^[-+]?(?:\d+\.\d+|\d+|\.\d+)$/.test(token)) {
      operands.push(Number(token));
      continue;
    }
    if ((token === "Td" || token === "TD") && operands.length >= 2) {
      const point = operands.slice(-2) as Point;
      if (token === "TD") leadingMove.push(point);
      else move.push(point);
    }
    operands.length = 0;
  }

  return { move, leadingMove };
}

function parseTransformOps(content: string): Matrix[] {
  const matrices: Matrix[] = [];
  const operands: number[] = [];
  let index = 0;

  const isWhite = (char: string): boolean => /\s/.test(char);
  const isDelimiter = (char: string): boolean => "()<>[]{}/%".includes(char);
  const skipString = (): void => {
    let depth = 1;
    index += 1;
    while (index < content.length && depth > 0) {
      const char = content[index++];
      if (char === "\\") index += 1;
      else if (char === "(") depth += 1;
      else if (char === ")") depth -= 1;
    }
  };
  const skipHexOrDict = (): void => {
    if (content[index + 1] === "<") {
      index += 2;
      return;
    }
    index += 1;
    while (index < content.length && content[index] !== ">") index += 1;
    index += 1;
  };
  const skipArray = (): void => {
    let depth = 1;
    index += 1;
    while (index < content.length && depth > 0) {
      const char = content[index];
      if (char === "(") skipString();
      else if (char === "<") skipHexOrDict();
      else {
        if (char === "[") depth += 1;
        else if (char === "]") depth -= 1;
        index += 1;
      }
    }
  };
  const readToken = (): string => {
    const start = index;
    while (index < content.length && !isWhite(content[index]) && !isDelimiter(content[index])) index += 1;
    return content.slice(start, index);
  };

  while (index < content.length) {
    const char = content[index];
    if (isWhite(char)) {
      index += 1;
      continue;
    }
    if (char === "%") {
      while (index < content.length && content[index] !== "\n" && content[index] !== "\r") index += 1;
      continue;
    }
    if (char === "(") {
      skipString();
      continue;
    }
    if (char === "[") {
      skipArray();
      operands.length = 0;
      continue;
    }
    if (char === "<") {
      skipHexOrDict();
      operands.length = 0;
      continue;
    }
    if (char === ">") {
      index += content[index + 1] === ">" ? 2 : 1;
      operands.length = 0;
      continue;
    }
    if (char === "/") {
      index += 1;
      readToken();
      operands.length = 0;
      continue;
    }

    const token = readToken();
    if (!token) {
      index += 1;
      continue;
    }
    if (/^[-+]?(?:\d+\.\d+|\d+|\.\d+)$/.test(token)) {
      operands.push(Number(token));
      continue;
    }
    if (token === "cm" && operands.length >= 6) matrices.push(operands.slice(-6) as Matrix);
    operands.length = 0;
  }

  return matrices;
}

function parsePathOps(content: string): number[][] {
  const paths: number[][] = [];
  let current: number[] = [];
  const operands: number[] = [];
  let point: Point = [0, 0];
  let index = 0;

  const isWhite = (char: string): boolean => /\s/.test(char);
  const isDelimiter = (char: string): boolean => "()<>[]{}/%".includes(char);
  const skipString = (): void => {
    let depth = 1;
    index += 1;
    while (index < content.length && depth > 0) {
      const char = content[index++];
      if (char === "\\") index += 1;
      else if (char === "(") depth += 1;
      else if (char === ")") depth -= 1;
    }
  };
  const skipHexOrDict = (): void => {
    if (content[index + 1] === "<") {
      index += 2;
      return;
    }
    index += 1;
    while (index < content.length && content[index] !== ">") index += 1;
    index += 1;
  };
  const skipArray = (): void => {
    let depth = 1;
    index += 1;
    while (index < content.length && depth > 0) {
      const char = content[index];
      if (char === "(") skipString();
      else if (char === "<") skipHexOrDict();
      else {
        if (char === "[") depth += 1;
        else if (char === "]") depth -= 1;
        index += 1;
      }
    }
  };
  const readToken = (): string => {
    const start = index;
    while (index < content.length && !isWhite(content[index]) && !isDelimiter(content[index])) index += 1;
    return content.slice(start, index);
  };
  const flush = (close = false, includeEmpty = false): void => {
    if (close) current.push(4);
    if (current.length || includeEmpty) paths.push(current);
    current = [];
  };

  while (index < content.length) {
    const char = content[index];
    if (isWhite(char)) {
      index += 1;
      continue;
    }
    if (char === "%") {
      while (index < content.length && content[index] !== "\n" && content[index] !== "\r") index += 1;
      continue;
    }
    if (char === "(") {
      skipString();
      continue;
    }
    if (char === "[") {
      skipArray();
      operands.length = 0;
      continue;
    }
    if (char === "<") {
      skipHexOrDict();
      operands.length = 0;
      continue;
    }
    if (char === ">") {
      index += content[index + 1] === ">" ? 2 : 1;
      operands.length = 0;
      continue;
    }
    if (char === "/") {
      index += 1;
      readToken();
      operands.length = 0;
      continue;
    }

    const token = readToken();
    if (!token) {
      index += 1;
      continue;
    }
    if (/^[-+]?(?:\d+\.\d+|\d+|\.\d+)$/.test(token)) {
      operands.push(Number(token));
      continue;
    }

    if (token === "m" && operands.length >= 2) {
      const [x, y] = operands.slice(-2);
      current.push(0, x, y);
      point = [x, y];
    } else if (token === "l" && operands.length >= 2) {
      const [x, y] = operands.slice(-2);
      current.push(1, x, y);
      point = [x, y];
    } else if (token === "c" && operands.length >= 6) {
      const values = operands.slice(-6);
      current.push(2, ...values);
      point = [values[4], values[5]];
    } else if (token === "v" && operands.length >= 4) {
      const values = operands.slice(-4);
      current.push(2, point[0], point[1], ...values);
      point = [values[2], values[3]];
    } else if (token === "y" && operands.length >= 4) {
      const values = operands.slice(-4);
      current.push(3, ...values);
      point = [values[2], values[3]];
    } else if (token === "h") {
      current.push(4);
    } else if (token === "re" && operands.length >= 4) {
      const [x, y, width, height] = operands.slice(-4);
      current.push(5, x, y, width, height);
      point = [x, y];
    } else if (["S", "f", "F", "f*", "B", "B*", "n"].includes(token)) {
      flush(false, token === "n");
    } else if (["s", "b", "b*"].includes(token)) {
      flush(true);
    }
    operands.length = 0;
  }

  return paths;
}

function parseNumbers(value: string): number[] {
  return Array.from(value.matchAll(/[-+]?(?:\d+\.\d+|\d+|\.\d+)/g)).map((m) => Number(m[0]));
}

function decodePdfName(value: string): string {
  return value.replace(/#([0-9A-Fa-f]{2})/g, (_match, hex: string) => String.fromCharCode(Number.parseInt(hex, 16)));
}

function decodePdfNameUtf8(value: string): string {
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

function resolvePageBoxes(pdfPage: any, pageObjectText: string | undefined): PageBoxes {
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
        return cleanBBox([rawMedia[3] - rawBox[3], rawBox[0] - rawMedia[0], rawMedia[3] - rawBox[1], rawBox[2] - rawMedia[0]]);
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

function parseFontRecords(objects: Map<number, string>): FontRecord[] {
  const fonts: FontRecord[] = [];
  for (const [objectNumber, text] of objects) {
    if (!/\/Type\s*\/Font\b/.test(text) || !/\/BaseFont\s*(?:\/|\d+\s+\d+\s+R)/.test(text)) continue;
    const baseFontRef = text.match(/\/BaseFont\s+(\d+)\s+\d+\s+R/)?.[1];
    const baseFontRaw =
      text.match(/\/BaseFont\s*\/([^\s/>]+)/)?.[1] ??
      (baseFontRef ? objects.get(Number(baseFontRef))?.match(/\bobj\s*\/([^\s/>]+)/)?.[1] : undefined);
    if (!baseFontRaw) continue;
    const baseFont = decodePdfName(baseFontRaw);
    const firstChar = Number(text.match(/\/FirstChar\s+(\d+)/)?.[1] ?? 0);
    const widthsRef = text.match(/\/Widths\s+(\d+)\s+\d+\s+R/)?.[1];
    const widthsText = widthsRef ? objects.get(Number(widthsRef)) : text;
    const widthsMatch = widthsText?.match(/\[([\s\S]*?)\]/);
    const widths = widthsMatch ? parseNumbers(widthsMatch[1]) : [];
    const descendantRef = text.match(/\/DescendantFonts\s*\[\s*(\d+)\s+\d+\s+R/)?.[1];
    const descendantContainer = descendantRef ? objects.get(Number(descendantRef)) : undefined;
    const nestedDescendantRef = descendantContainer?.match(/^\s*\[\s*(\d+)\s+\d+\s+R/)?.[1];
    const descendant = nestedDescendantRef ? objects.get(Number(nestedDescendantRef)) : descendantContainer;
    const descriptorRef = text.match(/\/FontDescriptor\s+(\d+)\s+\d+\s+R/)?.[1] ?? descendant?.match(/\/FontDescriptor\s+(\d+)\s+\d+\s+R/)?.[1];
    const descriptor = descriptorRef ? objects.get(Number(descriptorRef)) : undefined;
    const ascent = descriptor?.match(/\/Ascent\s+([-+]?(?:\d+\.\d+|\d+|\.\d+))/)?.[1];
    const descent = descriptor?.match(/\/Descent\s+([-+]?(?:\d+\.\d+|\d+|\.\d+))/)?.[1];
    fonts.push({
      objectNumber,
      baseFont,
      firstChar,
      widths,
      ascent: ascent == null ? undefined : Number(ascent) / 1000,
      descent: descent == null ? undefined : -Math.abs(Number(descent) / 1000)
    });
  }
  return fonts;
}

function parseInfoMetadata(raw: string, objects: Map<number, string>): Record<string, unknown> {
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
  if (bytes[0] === 0xfe && bytes[1] === 0xff) {
    let out = "";
    for (let i = 2; i < bytes.length - 1; i += 2) out += String.fromCharCode((bytes[i] << 8) | bytes[i + 1]);
    return out;
  }
  if (bytes[0] === 0xff && bytes[1] === 0xfe) {
    let out = "";
    for (let i = 2; i < bytes.length - 1; i += 2) out += String.fromCharCode(bytes[i] | (bytes[i + 1] << 8));
    return out;
  }
  return String.fromCharCode(...bytes);
}

function decodePdfLiteralValue(value: string): string {
  return decodePdfStringBytes([...value].map((char) => char.charCodeAt(0) & 0xff));
}

function readPdfLiteralString(text: string, key: string): string | null {
  const start = text.search(new RegExp(`/${key}\\s*\\(`));
  if (start < 0) return null;
  return readPdfLiteralStringAt(text, text.indexOf("(", start));
}

function readPdfLiteralStringAt(text: string, start: number): string | null {
  let i = start;
  if (i < 0) return null;
  i += 1;
  let depth = 1;
  let out = "";
  for (; i < text.length; i += 1) {
    const char = text[i];
    if (char === "\\") {
      const next = text[++i];
      if (next == null) break;
      if (/[0-7]/.test(next)) {
        let octal = next;
        for (let count = 0; count < 2 && /[0-7]/.test(text[i + 1] ?? ""); count += 1) octal += text[++i];
        out += String.fromCharCode(Number.parseInt(octal, 8));
      } else {
        out += ({ n: "\n", r: "\r", t: "\t", b: "\b", f: "\f" } as Record<string, string>)[next] ?? next;
      }
    } else if (char === "(") {
      depth += 1;
      out += char;
    } else if (char === ")") {
      depth -= 1;
      if (depth === 0) return decodePdfLiteralValue(out);
      out += char;
    } else {
      out += char;
    }
  }
  return null;
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
    let best = fontObjName
      ? (fontRecords.find((record) => record.baseFont === fontObjName && (record.ascent != null || record.descent != null)) ??
        fontRecords.find((record) => record.baseFont === fontObjName))
      : undefined;
    let bestScore = Number.POSITIVE_INFINITY;
    if (!best) {
      for (const record of fontRecords) {
        if (used.has(record.objectNumber)) continue;
        let score = 0;
        let count = 0;
        for (let code = 0; code < 256; code += 1) {
          const raw = record.widths[code - record.firstChar];
          const actual = fontObj?.widths?.[code];
          if (raw != null && actual != null && (raw !== 0 || actual !== 0)) {
            score += Math.abs(raw - actual);
            count += 1;
          }
        }
        if (count === 0) score += 10000;
        if (fontObj?.isSerifFont && !/Times|Serif|Roman/i.test(record.baseFont)) score += 1000;
        if (!fontObj?.isSerifFont && /Times|Serif|Roman/i.test(record.baseFont)) score += 1000;
        if (score < bestScore) {
          bestScore = score;
          best = record;
        }
      }
    }
    if (best) used.add(best.objectNumber);
    const style = styles[id] ?? {};
    const bboxAscent = Array.isArray(fontObj?.bbox) && typeof fontObj.bbox[3] === "number" ? Number(fontObj.bbox[3]) / 1000 : undefined;
    const bboxDescent = Array.isArray(fontObj?.bbox) && typeof fontObj.bbox[1] === "number" ? Number(fontObj.bbox[1]) / 1000 : undefined;
    const fontname = pythonBytesName(STANDARD_FONT_ALIASES[best?.baseFont ?? ""] ?? STANDARD_FONT_ALIASES[fontObjName ?? ""] ?? fontObjName ?? best?.baseFont ?? id);
    const standardDescent = STANDARD_DESCENTS[fontname];
    const useBBoxMetrics = !best && /^(Arial|ArialMT|Arial-BoldMT|Arial-ItalicMT|Arial-BoldItalicMT)$/.test(fontObjName ?? "");
    mapped.set(id, {
      fontname,
      ascent: cleanNumber(firstFinite(0.8, useBBoxMetrics ? bboxAscent : undefined, best?.ascent, fontObj?.ascent, style.ascent)),
      descent: cleanNumber(firstFinite(-0.2, standardDescent, useBBoxMetrics ? bboxDescent : undefined, best?.descent, fontObj?.descent, style.descent)),
      fontMatrix0: Number(fontObj?.fontMatrix?.[0] ?? 0.001),
      vertical: Boolean(style.vertical ?? fontObj?.vertical)
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
      current = { points: [point], closed: false, hasCurve: false, lastOp: op };
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
        current.closed = true;
        if (current.fromRect) current.forceCurve = true;
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
  if (path.fromRect) return true;
  void inferFromGeometry;
  const pts = [...path.points];
  if (path.closed) {
    while (pts.length > 1) {
      const first = pts[0];
      const last = pts[pts.length - 1];
      if (first[0] !== last[0] || first[1] !== last[1]) break;
      pts.pop();
    }
  }
  if (!path.closed || path.hasCurve || pts.length !== 4) return false;
  const xs = new Set(pts.map((p) => cleanNumber(p[0])));
  const ys = new Set(pts.map((p) => cleanNumber(p[1])));
  return xs.size === 2 && ys.size === 2;
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

function extractPageObjects(
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
): Pick<PdfPlumberPageImpl, "chars" | "rects" | "lines" | "curves" | "images"> {
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
  const markedContent: Array<{ tag: string | null; mcid: number | null; suppressed?: boolean }> = [];
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
  const partialRawPaths = pathOps.length > 0 && pathOps.length !== constructPathTotal;
  let usedPartialRawCurve = false;
  const markedExtras = () => {
    const current = [...markedContent].reverse().find((item) => !item.suppressed && item.tag !== "OC");
    return {
      mcid: current?.mcid ?? null,
      tag: current?.tag ?? null
    };
  };
  const colorFromHint = (fallback: unknown, queue: ColorOp[], currentColorSpace: string): { color: unknown; colorSpace: string | null; fromHint: boolean } => {
    const hint = queue.shift();
    if (hint?.pattern) return { color: hint.pattern, colorSpace: "Pattern", fromHint: true };
    if (hint?.components.length === 1) return { color: [cleanNumber(hint.components[0])], colorSpace: hint.colorSpace, fromHint: true };
    if (hint) return { color: hint.components.map(cleanNumber), colorSpace: hint.colorSpace, fromHint: true };
    if (!preferRgbFallback && currentColorSpace === "DeviceGray" && typeof fallback === "string" && /^#([0-9a-f]{2})\1\1$/i.test(fallback)) {
      const component = parseInt(fallback.slice(1, 3), 16);
      return { color: [component === 128 ? 0.5 : cleanNumber(component / 255)], colorSpace: "DeviceGray", fromHint: false };
    }
    if (typeof fallback === "string" && /^#[0-9a-f]{6}$/i.test(fallback)) return { color: rgbColor(fallback), colorSpace: "DeviceRGB", fromHint: false };
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
      case pdfjs.OPS.setLineWidth:
        state.lineWidth = Number(args?.[0] ?? state.lineWidth) * lineWidthScale(state.ctm);
        state.lineWidthSet = true;
        break;
      case pdfjs.OPS.setDash:
        state.dash = args ? [args[0] ?? [], args[1] ?? 0] : null;
        break;
      case pdfjs.OPS.beginMarkedContent:
        markedContent.push({ tag: decodePdfNameUtf8(args?.[0]?.name ?? String(args?.[0] ?? "")), mcid: null });
        break;
      case pdfjs.OPS.beginMarkedContentProps:
        markedContent.push({ tag: decodePdfNameUtf8(args?.[0]?.name ?? String(args?.[0] ?? "")), mcid: typeof args?.[1] === "number" ? args[1] : null });
        break;
      case pdfjs.OPS.endMarkedContent:
        {
          const ended = markedContent.pop();
          if (ended?.mcid === null && markedContent.length) markedContent[markedContent.length - 1].suppressed = true;
        }
        break;
      case pdfjs.OPS.setFillColorSpace:
        state.fillColorSpace = colorSpaceName(args?.[0]);
        break;
      case pdfjs.OPS.setStrokeColorSpace:
        state.strokeColorSpace = colorSpaceName(args?.[0]);
        break;
      case pdfjs.OPS.setFillRGBColor: {
        const hinted = colorFromHint(args?.[0], fillColorOps, state.fillColorSpace);
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
          const rawText = glyph.unicode === "\r" ? "(cid:13)" : (glyph.unicode ?? "");
          const text = unicodeNorm ? rawText.normalize(unicodeNorm) : rawText;
          const advance = text === "\n" && Number(glyph.originalCharCode ?? 0) <= 2 ? 0 : Number(glyph.width ?? 0) * state.fontSize * font.fontMatrix0;
          const verticalMetric = Array.isArray(glyph.vmetric) ? (glyph.vmetric as number[]) : null;
          const verticalX = Number(verticalMetric?.[1] ?? 500) * state.fontSize * font.fontMatrix0;
          const verticalY = Number(verticalMetric?.[2] ?? 880) * state.fontSize * font.fontMatrix0;
          const xStart = font.vertical ? state.x + x - verticalX : state.x + x * textHScale;
          const xEnd = font.vertical ? xStart + advance : state.x + (x + advance) * textHScale;
          const yStart = font.vertical ? state.y + state.textRise - verticalY : state.y + state.textRise + font.descent * state.fontSize;
          const yEnd = yStart + state.fontSize;
          const rawBBox = bboxFromPoints([
            contentPoint(applyMatrix([xStart, yStart], matrix)),
            contentPoint(applyMatrix([xStart, yEnd], matrix)),
            contentPoint(applyMatrix([xEnd, yStart], matrix)),
            contentPoint(applyMatrix([xEnd, yEnd], matrix))
          ]);
          const originX = font.vertical ? state.x + x : xStart;
          const originY = state.y + state.textRise;
          const [matrixE, matrixF] = contentPoint(applyMatrix([originX, originY], matrix));
          const glyphMatrix = cleanMatrix(matrixToPageMatrix([matrix[0], matrix[1], matrix[2], matrix[3], matrixE, matrixF], pageWidth, pageHeight, pageRotate));
          const upright =
            pageRotate === 90 || pageRotate === 270
              ? Math.abs(matrix[0]) < 1e-6 && Math.abs(matrix[3]) < 1e-6
              : matrix[0] * matrix[3] * textHScale > 0 && matrix[1] * matrix[2] <= 0;
          glyphMatrix[4] = cleanNumber(glyphMatrix[4] - pageX0);
          glyphMatrix[5] = cleanNumber(glyphMatrix[5] + pageTop);
          const obj = rectFromPdfBBox(rawBBox, pageWidth, pageHeight, pageRotate, pageNumber, "char", doctopOffset, {
            text,
            fontname: font.fontname,
            adv: font.vertical && verticalMetric ? cleanNumber(Number(verticalMetric[0]) * state.fontSize * font.fontMatrix0) : Math.abs(advance * textHScale),
            upright,
            matrix: glyphMatrix,
            ncs: state.fillColorSpace,
            non_stroking_color: colorValue(state.fillColor),
            stroking_color: colorValue(state.strokeColor),
            ...markedExtras()
          }, coordOffset);
          obj.size = font.vertical ? obj.width : obj.height;
          chars.push(obj);
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
        for (const path of paths) {
          const operandPoints = rawPath ? path.points : path.points.map(snapPathOperandPoint);
          const transformed = operandPoints.map((point) => contentPoint(applyMatrix(point, state.ctm)));
          const inferRectFromGeometry = rawPath === undefined;
          if (isStroke && !isFill) {
            const rawBBox = pathBBox(transformed);
            if (path.closed || path.hasCurve || transformed.length > 2) {
              const lineEndpoints = !path.hasCurve ? collinearPathEndpoints(transformed) : null;
              if (lineEndpoints) {
                const rawLineBBox = pathBBox(lineEndpoints);
                lines.push(rectFromPdfBBox(rawLineBBox, pageWidth, pageHeight, pageRotate, pageNumber, "line", doctopOffset, path.closed ? vectorExtras : lineExtras, coordOffset));
              } else if (isAxisAlignedRect({ ...path, points: transformed }, inferRectFromGeometry)) {
                rects.push(
                  rectFromPdfBBox(rawBBox, pageWidth, pageHeight, pageRotate, pageNumber, "rect", doctopOffset, {
                    ...vectorExtras,
                    linewidth: isStroke || state.lineWidthSet ? linewidth : 0
                  }, coordOffset)
                );
              } else {
                curves.push(
                  rectFromPdfBBox(rawBBox, pageWidth, pageHeight, pageRotate, pageNumber, "curve", doctopOffset, {
                    pts: transformed.map((point) => pointToPageCoords(point, pageWidth, pageHeight, pageRotate).map(cleanNumber)),
                    ...vectorExtras
                  }, coordOffset)
                );
              }
            } else {
              if (transformed.length === 1 && paths.length === 1) {
                const rawPointBBox = pathBBox(transformed);
                curves.push(
                  rectFromPdfBBox(rawPointBBox, pageWidth, pageHeight, pageRotate, pageNumber, "curve", doctopOffset, {
                    pts: transformed.map((point) => pointToPageCoords(point, pageWidth, pageHeight, pageRotate).map(cleanNumber)),
                    ...vectorExtras
                  }, coordOffset)
                );
              }
              for (let p = 0; p < transformed.length - 1; p += 1) {
                const rawLineBBox = pathBBox([transformed[p], transformed[p + 1]]);
                lines.push(rectFromPdfBBox(rawLineBBox, pageWidth, pageHeight, pageRotate, pageNumber, "line", doctopOffset, lineExtras, coordOffset));
              }
            }
          } else if (isFill) {
            const rawBBox = pathBBox(transformed);
            if (isAxisAlignedRect({ ...path, points: transformed }, inferRectFromGeometry)) {
              rects.push(
                rectFromPdfBBox(rawBBox, pageWidth, pageHeight, pageRotate, pageNumber, "rect", doctopOffset, {
                  ...vectorExtras,
                  linewidth: isStroke || state.lineWidthSet ? linewidth : 0
                }, coordOffset)
              );
            } else if (transformed.length > 1) {
              const curve = rectFromPdfBBox(rawBBox, pageWidth, pageHeight, pageRotate, pageNumber, "curve", doctopOffset, {
                pts: transformed.map((point) => pointToPageCoords(point, pageWidth, pageHeight, pageRotate).map(cleanNumber)),
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
        const resource = imageResources[imageIndex++];
        const srcWidth = Number(resource?.width ?? args?.[1] ?? args?.[0]?.width ?? 0);
        const srcHeight = Number(resource?.height ?? args?.[2] ?? args?.[0]?.height ?? 0);
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

function annotationToObject(
  annotation: any,
  pageNumber: number,
  pageWidth: number,
  pageHeight: number,
  pageRotate: number,
  doctopOffset: number,
  objects?: Map<number, string>
): PDFObject {
  const objectNumber = Number.parseInt(String(annotation.id ?? ""), 10);
  const rawRect = annotation.subtype === "Text" && Number.isFinite(objectNumber) ? extractRawBox(objects?.get(objectNumber), "Rect") : null;
  const rect = rawRect ?? (annotation.rect as number[]);
  const rawContents = annotation.contentsObj?.str ?? annotation.contents ?? null;
  const isTinyPopup = annotation.subtype === "Popup" && Math.abs(Number(rect[2]) - Number(rect[0])) <= 2 && Math.abs(Number(rect[3]) - Number(rect[1])) <= 2;
  return rectFromPdfBBox([rect[0], rect[1], rect[2], rect[3]], pageWidth, pageHeight, pageRotate, pageNumber, "annot", doctopOffset, {
    uri: annotation.unsafeUrl ?? annotation.url ?? null,
    title: annotation.subtype === "Popup" ? null : (annotation.titleObj?.str ?? annotation.title ?? null),
    contents: (annotation.subtype === "Popup" && !isTinyPopup) || ((annotation.subtype === "Link" || (annotation.subtype === "Stamp" && pageRotate !== 0)) && rawContents === "") ? null : rawContents
  });
}

function applyLimitSlice(words: PDFObject[], options: Record<string, unknown>): PDFObject[] {
  let out = words;
  if (Array.isArray(options.slice)) {
    const [start, end] = options.slice.map(Number);
    out = out.slice(start, end);
  }
  if (typeof options.limit === "number") out = out.slice(0, options.limit);
  return out;
}

function layoutSortChars(chars: PDFObject[]): PDFObject[] {
  return [...chars].sort((a, b) => Number(a.top) - Number(b.top) || Number(a.x0) - Number(b.x0));
}

function layoutClusterSortChars(chars: PDFObject[]): PDFObject[] {
  const lineKey = (char: PDFObject): number => {
    const matrix = char.matrix;
    return char.upright !== false && Array.isArray(matrix) && Number.isFinite(Number(matrix[5])) ? -Number(matrix[5]) : Number(char.top);
  };
  return clusterObjectsSimple([...chars].sort((a, b) => lineKey(a) - lineKey(b) || Number(a.x0) - Number(b.x0)), lineKey, 3).flatMap((cluster) =>
    [...cluster].sort((a, b) => Number(a.x0) - Number(b.x0) || Number(a.top) - Number(b.top))
  );
}

function layoutVerticalSortChars(chars: PDFObject[]): PDFObject[] {
  const columnKey = (char: PDFObject): number => -Number(char.x1 ?? char.x0 ?? 0);
  return clusterObjectsSimple([...chars].sort((a, b) => columnKey(a) - columnKey(b) || Number(a.top) - Number(b.top)), columnKey, 1).flatMap((cluster) =>
    [...cluster].sort((a, b) => Number(b.top) - Number(a.top) || Number(b.x1) - Number(a.x1))
  );
}

function objectFromChars(chars: PDFObject[], objectType: string, pageHeight: number, doctopOffset: number, text?: string): PDFObject {
  const [x0, top, x1, bottom] = objectsToBBox(chars);
  return cleanObject({
    object_type: objectType,
    page_number: chars[0]?.page_number ?? 0,
    text: text ?? chars.map((char) => char.text ?? "").join("") + "\n",
    x0,
    x1,
    y0: pageHeight - bottom,
    y1: pageHeight - top,
    top,
    bottom,
    doctop: top + doctopOffset,
    width: x1 - x0,
    height: bottom - top
  });
}

type LayoutLineType = "textlinehorizontal" | "textlinevertical";

interface LayoutLine {
  type: LayoutLineType;
  chars: PDFObject[];
  text: string;
  obj: PDFObject;
}

interface LayoutBox {
  type: "textboxhorizontal" | "textboxvertical";
  lines: LayoutLine[];
  obj: PDFObject;
}

function laparamNumber(laparams: Record<string, unknown>, key: string, fallback: number): number {
  const value = Number(laparams[key] ?? fallback);
  return Number.isFinite(value) ? value : fallback;
}

function horizontalDistance(a: PDFObject, b: PDFObject): number {
  if (Number(b.x0) <= Number(a.x1) && Number(a.x0) <= Number(b.x1)) return 0;
  return Math.min(Math.abs(Number(a.x0) - Number(b.x1)), Math.abs(Number(a.x1) - Number(b.x0)));
}

function verticalDistance(a: PDFObject, b: PDFObject): number {
  if (Number(b.y0) <= Number(a.y1) && Number(a.y0) <= Number(b.y1)) return 0;
  return Math.min(Math.abs(Number(a.y0) - Number(b.y1)), Math.abs(Number(a.y1) - Number(b.y0)));
}

function horizontalOverlap(a: PDFObject, b: PDFObject): number {
  if (!(Number(b.x0) <= Number(a.x1) && Number(a.x0) <= Number(b.x1))) return 0;
  return Math.min(Math.abs(Number(a.x0) - Number(b.x1)), Math.abs(Number(a.x1) - Number(b.x0)));
}

function verticalOverlap(a: PDFObject, b: PDFObject): number {
  if (!(Number(b.y0) <= Number(a.y1) && Number(a.y0) <= Number(b.y1))) return 0;
  return Math.min(Math.abs(Number(a.y0) - Number(b.y1)), Math.abs(Number(a.y1) - Number(b.y0)));
}

function buildLineText(chars: PDFObject[], type: LayoutLineType, wordMargin: number): string {
  let text = "";
  let boundary = type === "textlinehorizontal" ? Number.POSITIVE_INFINITY : Number.NEGATIVE_INFINITY;
  for (const char of chars) {
    const margin = wordMargin * Math.max(Number(char.width), Number(char.height));
    if (type === "textlinehorizontal") {
      if (boundary < Number(char.x0) - margin) text += " ";
      boundary = Number(char.x1);
    } else {
      if (Number(char.y1) + margin < boundary) text += " ";
      boundary = Number(char.y0);
    }
    text += String(char.text ?? "");
  }
  return `${text}\n`;
}

function makeLayoutLine(chars: PDFObject[], type: LayoutLineType, pageHeight: number, doctopOffset: number, wordMargin: number): LayoutLine {
  const text = buildLineText(chars, type, wordMargin);
  return {
    type,
    chars,
    text,
    obj: objectFromChars(chars, type, pageHeight, doctopOffset, text)
  };
}

function groupLayoutChars(chars: PDFObject[], pageHeight: number, doctopOffset: number, laparams: Record<string, unknown>): LayoutLine[] {
  const lineOverlap = laparamNumber(laparams, "line_overlap", 0.5);
  const charMargin = laparamNumber(laparams, "char_margin", 2);
  const wordMargin = laparamNumber(laparams, "word_margin", 0.1);
  const detectVertical = Boolean(laparams.detect_vertical);
  const lines: LayoutLine[] = [];
  let previous: PDFObject | null = null;
  let current: { type: LayoutLineType; chars: PDFObject[] } | null = null;

  for (const char of chars) {
    if (previous) {
      const halign =
        verticalOverlap(previous, char) > Math.min(Number(previous.height), Number(char.height)) * lineOverlap &&
        horizontalDistance(previous, char) < Math.max(Number(previous.width), Number(char.width)) * charMargin;
      const valign =
        detectVertical &&
        horizontalOverlap(previous, char) > Math.min(Number(previous.width), Number(char.width)) * lineOverlap &&
        verticalDistance(previous, char) < Math.max(Number(previous.height), Number(char.height)) * charMargin;

      if ((halign && current?.type === "textlinehorizontal") || (valign && current?.type === "textlinevertical")) {
        current.chars.push(char);
      } else if (current) {
        lines.push(makeLayoutLine(current.chars, current.type, pageHeight, doctopOffset, wordMargin));
        current = null;
      } else if (valign && !halign) {
        current = { type: "textlinevertical", chars: [previous, char] };
      } else if (halign && !valign) {
        current = { type: "textlinehorizontal", chars: [previous, char] };
      } else {
        lines.push(makeLayoutLine([previous], "textlinehorizontal", pageHeight, doctopOffset, wordMargin));
      }
    }
    previous = char;
  }

  if (previous) {
    if (current) lines.push(makeLayoutLine(current.chars, current.type, pageHeight, doctopOffset, wordMargin));
    else lines.push(makeLayoutLine([previous], "textlinehorizontal", pageHeight, doctopOffset, wordMargin));
  }
  return lines;
}

function lineObjectsOverlap(a: PDFObject, b: PDFObject): boolean {
  return Number(b.x0) <= Number(a.x1) && Number(a.x0) <= Number(b.x1) && Number(b.y0) <= Number(a.y1) && Number(a.y0) <= Number(b.y1);
}

function findLineNeighbors(line: LayoutLine, lines: LayoutLine[], ratio: number): LayoutLine[] {
  const obj = line.obj;
  if (line.type === "textlinehorizontal") {
    const d = ratio * Number(obj.height);
    const query = { x0: obj.x0, x1: obj.x1, y0: Number(obj.y0) - d, y1: Number(obj.y1) + d } as PDFObject;
    return lines.filter((other) => {
      const candidate = other.obj;
      if (other.type !== "textlinehorizontal" || !lineObjectsOverlap(candidate, query)) return false;
      return (
        Math.abs(Number(candidate.height) - Number(obj.height)) <= d &&
        (Math.abs(Number(candidate.x0) - Number(obj.x0)) <= d ||
          Math.abs(Number(candidate.x1) - Number(obj.x1)) <= d ||
          Math.abs((Number(candidate.x0) + Number(candidate.x1)) / 2 - (Number(obj.x0) + Number(obj.x1)) / 2) <= d)
      );
    });
  }

  const d = ratio * Number(obj.width);
  const query = { x0: Number(obj.x0) - d, x1: Number(obj.x1) + d, y0: obj.y0, y1: obj.y1 } as PDFObject;
  return lines.filter((other) => {
    const candidate = other.obj;
    if (other.type !== "textlinevertical" || !lineObjectsOverlap(candidate, query)) return false;
    return (
      Math.abs(Number(candidate.width) - Number(obj.width)) <= d &&
      (Math.abs(Number(candidate.y0) - Number(obj.y0)) <= d ||
        Math.abs(Number(candidate.y1) - Number(obj.y1)) <= d ||
        Math.abs((Number(candidate.y0) + Number(candidate.y1)) / 2 - (Number(obj.y0) + Number(obj.y1)) / 2) <= d)
    );
  });
}

function makeLayoutBox(lines: LayoutLine[], type: "textboxhorizontal" | "textboxvertical", pageHeight: number, doctopOffset: number): LayoutBox {
  const ordered =
    type === "textboxhorizontal" ? [...lines].sort((a, b) => Number(a.obj.top) - Number(b.obj.top)) : [...lines].sort((a, b) => Number(b.obj.x1) - Number(a.obj.x1));
  const chars = ordered.flatMap((line) => line.chars);
  const text = ordered.map((line) => line.text).join("");
  return {
    type,
    lines: ordered,
    obj: objectFromChars(chars, type, pageHeight, doctopOffset, text)
  };
}

function groupLayoutLines(lines: LayoutLine[], pageHeight: number, doctopOffset: number, laparams: Record<string, unknown>): LayoutBox[] {
  const lineMargin = laparamNumber(laparams, "line_margin", 0.5);
  const boxesByLine = new Map<LayoutLine, LayoutBox>();

  for (const line of lines) {
    const members: LayoutLine[] = [line];
    for (const neighbor of findLineNeighbors(line, lines, lineMargin)) {
      members.push(neighbor);
      const existing = boxesByLine.get(neighbor);
      if (existing) {
        for (const existingLine of existing.lines) members.push(existingLine);
      }
    }
    const unique = Array.from(new Set(members));
    const type = line.type === "textlinevertical" ? "textboxvertical" : "textboxhorizontal";
    const box = makeLayoutBox(unique, type, pageHeight, doctopOffset);
    for (const member of unique) boxesByLine.set(member, box);
  }

  const boxes: LayoutBox[] = [];
  const seen = new Set<LayoutBox>();
  for (const line of lines) {
    const box = boxesByLine.get(line);
    if (box && !seen.has(box) && String(box.obj.text ?? "").trim()) {
      seen.add(box);
      boxes.push(box);
    }
  }
  return boxes;
}

function sortLayoutBoxes(boxes: LayoutBox[], laparams: Record<string, unknown>): LayoutBox[] {
  const boxesFlowValue = laparams.boxes_flow;
  if (boxesFlowValue === null) {
    return [...boxes].sort((a, b) => {
      if (a.type !== b.type) return a.type === "textboxvertical" ? -1 : 1;
      if (a.type === "textboxvertical") return Number(b.obj.x1) - Number(a.obj.x1) || Number(b.obj.y0) - Number(a.obj.y0);
      return Number(b.obj.y0) - Number(a.obj.y0) || Number(a.obj.x0) - Number(b.obj.x0);
    });
  }

  const boxesFlow = laparamNumber(laparams, "boxes_flow", 0.5);
  type LayoutNode = {
    id: number;
    kind: "box" | "group";
    groupType?: "lrtb" | "tbrl";
    box?: LayoutBox;
    children?: LayoutNode[];
    x0: number;
    y0: number;
    x1: number;
    y1: number;
    width: number;
    height: number;
  };

  let nextId = 1;
  const nodeFromBox = (box: LayoutBox): LayoutNode => {
    const x0 = Number(box.obj.x0);
    const y0 = Number(box.obj.y0);
    const x1 = Number(box.obj.x1);
    const y1 = Number(box.obj.y1);
    return { id: nextId++, kind: "box", box, x0, y0, x1, y1, width: x1 - x0, height: y1 - y0 };
  };
  const nodeFromChildren = (children: LayoutNode[], groupType: "lrtb" | "tbrl"): LayoutNode => {
    const x0 = Math.min(...children.map((child) => child.x0));
    const y0 = Math.min(...children.map((child) => child.y0));
    const x1 = Math.max(...children.map((child) => child.x1));
    const y1 = Math.max(...children.map((child) => child.y1));
    return { id: nextId++, kind: "group", groupType, children, x0, y0, x1, y1, width: x1 - x0, height: y1 - y0 };
  };
  const isTbrl = (node: LayoutNode): boolean => node.groupType === "tbrl" || node.box?.type === "textboxvertical";
  const distance = (a: LayoutNode, b: LayoutNode): number => {
    const x0 = Math.min(a.x0, b.x0);
    const y0 = Math.min(a.y0, b.y0);
    const x1 = Math.max(a.x1, b.x1);
    const y1 = Math.max(a.y1, b.y1);
    return (x1 - x0) * (y1 - y0) - a.width * a.height - b.width * b.height;
  };
  const overlaps = (node: LayoutNode, bbox: BBox): boolean => node.x1 >= bbox[0] && node.x0 <= bbox[2] && node.y1 >= bbox[1] && node.y0 <= bbox[3];
  const hasAnyBetween = (plane: LayoutNode[], a: LayoutNode, b: LayoutNode): boolean => {
    const bbox: BBox = [Math.min(a.x0, b.x0), Math.min(a.y0, b.y0), Math.max(a.x1, b.x1), Math.max(a.y1, b.y1)];
    return plane.some((node) => node !== a && node !== b && overlaps(node, bbox));
  };
  const analyze = (node: LayoutNode, out: LayoutBox[]): void => {
    if (node.kind === "box") {
      if (node.box) out.push(node.box);
      return;
    }
    const children = [...(node.children ?? [])];
    if (node.groupType === "tbrl") {
      children.sort((a, b) => (-(1 + boxesFlow) * (a.x0 + a.x1) - (1 - boxesFlow) * a.y1) - (-(1 + boxesFlow) * (b.x0 + b.x1) - (1 - boxesFlow) * b.y1));
    } else {
      children.sort((a, b) => ((1 - boxesFlow) * a.x0 - (1 + boxesFlow) * (a.y0 + a.y1)) - ((1 - boxesFlow) * b.x0 - (1 + boxesFlow) * (b.y0 + b.y1)));
    }
    for (const child of children) analyze(child, out);
  };

  const nodes = boxes.map(nodeFromBox);
  let plane = [...nodes];
  const queue: Array<{ skipIsAny: boolean; distance: number; a: LayoutNode; b: LayoutNode; order: number }> = [];
  let order = 0;
  for (let i = 0; i < nodes.length; i += 1) {
    for (let j = i + 1; j < nodes.length; j += 1) {
      queue.push({ skipIsAny: false, distance: distance(nodes[i], nodes[j]), a: nodes[i], b: nodes[j], order: order++ });
    }
  }

  while (queue.length) {
    queue.sort((a, b) => Number(a.skipIsAny) - Number(b.skipIsAny) || a.distance - b.distance || a.a.id - b.a.id || a.b.id - b.b.id || a.order - b.order);
    const item = queue.shift();
    if (!item || !plane.includes(item.a) || !plane.includes(item.b)) continue;
    if (!item.skipIsAny && hasAnyBetween(plane, item.a, item.b)) {
      queue.push({ ...item, skipIsAny: true, order: order++ });
      continue;
    }
    const group = nodeFromChildren([item.a, item.b], isTbrl(item.a) || isTbrl(item.b) ? "tbrl" : "lrtb");
    plane = plane.filter((node) => node !== item.a && node !== item.b);
    for (const other of plane) queue.push({ skipIsAny: false, distance: distance(group, other), a: group, b: other, order: order++ });
    plane.push(group);
  }

  const out: LayoutBox[] = [];
  for (const node of plane) analyze(node, out);
  return out;
}

function buildLayoutObjects(chars: PDFObject[], images: PDFObject[], pageHeight: number, doctopOffset: number, laparams: Record<string, unknown>): { chars: PDFObject[]; objects: Record<string, PDFObject[]> } {
  const allLines = groupLayoutChars(chars, pageHeight, doctopOffset, laparams);
  const textLines = allLines.filter((line) => String(line.text).trim());
  const rawIndex = new Map<PDFObject, number>(chars.map((char, index) => [char, index]));
  const emptyLines = allLines
    .filter((line) => !String(line.text).trim())
    .sort((a, b) => (rawIndex.get(a.chars[0]) ?? 0) - (rawIndex.get(b.chars[0]) ?? 0));
  const boxes = sortLayoutBoxes(groupLayoutLines(textLines, pageHeight, doctopOffset, laparams), laparams);

  const horizontalBoxes = boxes.filter((box) => box.type === "textboxhorizontal");
  const verticalBoxes = boxes.filter((box) => box.type === "textboxvertical");
  const orderedLines = boxes.flatMap((box) => box.lines);
  const emptyChars = new Set(emptyLines.flatMap((line) => line.chars));
  const layoutChars = [...orderedLines.flatMap((line) => line.chars), ...chars.filter((char) => emptyChars.has(char))];
  const objectLines = [...orderedLines, ...emptyLines];

  return {
    chars: layoutChars.length === chars.length ? layoutChars : chars,
    objects: {
      textlinehorizontal: objectLines.filter((line) => line.type === "textlinehorizontal").map((line) => line.obj),
      textboxhorizontal: horizontalBoxes.map((box) => box.obj),
      textlinevertical: objectLines.filter((line) => line.type === "textlinevertical").map((line) => line.obj),
      textboxvertical: verticalBoxes.map((box) => box.obj),
      figure: images.map((image) => cleanObject({ ...image, object_type: "figure" }))
    }
  };
}

export class Table {
  constructor(
    readonly page: PdfPlumberPageImpl,
    readonly cells: BBox[]
  ) {}

  get bbox(): BBox {
    return [
      Math.min(...this.cells.map((c) => c[0])),
      Math.min(...this.cells.map((c) => c[1])),
      Math.max(...this.cells.map((c) => c[2])),
      Math.max(...this.cells.map((c) => c[3]))
    ];
  }

  private groups(axis: 0 | 1): Array<Array<BBox | null>> {
    const antiaxis = axis === 0 ? 1 : 0;
    const sorted = [...this.cells].sort((a, b) => a[antiaxis] - b[antiaxis] || a[axis] - b[axis]);
    const xs = Array.from(new Set(this.cells.map((cell) => cell[axis]))).sort((a, b) => a - b);
    const rows: Array<Array<BBox | null>> = [];
    let previous: number | undefined;
    let rowMap = new Map<number, BBox>();
    const flush = () => {
      if (rowMap.size) rows.push(xs.map((x) => rowMap.get(x) ?? null));
      rowMap = new Map();
    };
    for (const cell of sorted) {
      if (previous !== undefined && cell[antiaxis] !== previous) flush();
      rowMap.set(cell[axis], cell);
      previous = cell[antiaxis];
    }
    flush();
    return rows;
  }

  get rows(): Array<Array<BBox | null>> {
    return this.groups(0);
  }

  extract(options: Record<string, unknown> = {}): Array<Array<string | null>> {
    const charInBBox = (char: PDFObject, bbox: BBox): boolean => {
      const vMid = (Number(char.top) + Number(char.bottom)) / 2;
      const hMid = (Number(char.x0) + Number(char.x1)) / 2;
      return hMid >= bbox[0] && hMid < bbox[2] && vMid >= bbox[1] && vMid < bbox[3];
    };
    return this.rows.map((row) => {
      const rowBBox = objectsToBBox(row.filter((cell): cell is BBox => cell !== null).map((cell) => ({ object_type: "cell", page_number: this.page.page_number, x0: cell[0], top: cell[1], x1: cell[2], bottom: cell[3] })));
      const rowChars = this.page.chars.filter((char) => charInBBox(char, rowBBox));
      return row.map((cell) => {
        if (!cell) return null;
        const cellChars = rowChars.filter((char) => charInBBox(char, cell));
        if (!cellChars.length) return "";
        const textOptions = { ...options };
        if ("layout" in textOptions) {
          textOptions.layout_width = cell[2] - cell[0];
          textOptions.layout_height = cell[3] - cell[1];
          textOptions.layout_bbox = cell;
        }
        return extractTextFromChars(cellChars, textOptions);
      });
    });
  }
}

interface TableSettings {
  vertical_strategy: string;
  horizontal_strategy: string;
  explicit_vertical_lines: Array<PDFObject | number>;
  explicit_horizontal_lines: Array<PDFObject | number>;
  snap_x_tolerance: number;
  snap_y_tolerance: number;
  join_x_tolerance: number;
  join_y_tolerance: number;
  edge_min_length: number;
  edge_min_length_prefilter: number;
  min_words_vertical: number;
  min_words_horizontal: number;
  intersection_x_tolerance: number;
  intersection_y_tolerance: number;
  text_settings: Record<string, unknown>;
}

function resolveTableSettings(options: Record<string, unknown> = {}): TableSettings {
  const snapTolerance = Number(options.snap_tolerance ?? 3);
  const joinTolerance = Number(options.join_tolerance ?? 3);
  const intersectionTolerance = Number(options.intersection_tolerance ?? 3);
  const textSettings: Record<string, unknown> = {};
  const core: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(options)) {
    if (key.startsWith("text_")) textSettings[key.slice(5)] = value;
    else core[key] = value;
  }
  textSettings.x_tolerance ??= textSettings.tolerance ?? 3;
  textSettings.y_tolerance ??= textSettings.tolerance ?? 3;
  delete textSettings.tolerance;
  return {
    vertical_strategy: String(core.vertical_strategy ?? "lines"),
    horizontal_strategy: String(core.horizontal_strategy ?? "lines"),
    explicit_vertical_lines: Array.isArray(core.explicit_vertical_lines) ? (core.explicit_vertical_lines as Array<PDFObject | number>) : [],
    explicit_horizontal_lines: Array.isArray(core.explicit_horizontal_lines) ? (core.explicit_horizontal_lines as Array<PDFObject | number>) : [],
    snap_x_tolerance: Number(core.snap_x_tolerance ?? snapTolerance),
    snap_y_tolerance: Number(core.snap_y_tolerance ?? snapTolerance),
    join_x_tolerance: Number(core.join_x_tolerance ?? joinTolerance),
    join_y_tolerance: Number(core.join_y_tolerance ?? joinTolerance),
    edge_min_length: Number(core.edge_min_length ?? 3),
    edge_min_length_prefilter: Number(core.edge_min_length_prefilter ?? 1),
    min_words_vertical: Number(core.min_words_vertical ?? 3),
    min_words_horizontal: Number(core.min_words_horizontal ?? 1),
    intersection_x_tolerance: Number(core.intersection_x_tolerance ?? intersectionTolerance),
    intersection_y_tolerance: Number(core.intersection_y_tolerance ?? intersectionTolerance),
    text_settings: textSettings
  };
}

function snapEdges(edges: PDFObject[], xTolerance: number, yTolerance: number): PDFObject[] {
  const v = edges.filter((edge) => edge.orientation === "v");
  const h = edges.filter((edge) => edge.orientation === "h");
  return [...snapObjects(v, "x0", xTolerance), ...snapObjects(h, "top", yTolerance)];
}

function joinEdgeGroup(edges: PDFObject[], orientation: "h" | "v", tolerance: number): PDFObject[] {
  const minProp = orientation === "h" ? "x0" : "top";
  const maxProp = orientation === "h" ? "x1" : "bottom";
  const sorted = [...edges].sort((a, b) => Number(a[minProp]) - Number(b[minProp]));
  const joined = sorted.slice(0, 1);
  for (const edge of sorted.slice(1)) {
    const last = joined[joined.length - 1];
    if (Number(edge[minProp]) <= Number(last[maxProp]) + tolerance) {
      if (Number(edge[maxProp]) > Number(last[maxProp])) joined[joined.length - 1] = resizeObject(last, maxProp, Number(edge[maxProp]));
    } else joined.push(edge);
  }
  return joined;
}

function mergeEdges(edges: PDFObject[], settings: TableSettings): PDFObject[] {
  let merged = edges;
  if (settings.snap_x_tolerance > 0 || settings.snap_y_tolerance > 0) merged = snapEdges(merged, settings.snap_x_tolerance, settings.snap_y_tolerance);
  const sorted = [...merged].sort((a, b) => String(a.orientation).localeCompare(String(b.orientation)) || (a.orientation === "h" ? Number(a.top) - Number(b.top) : Number(a.x0) - Number(b.x0)));
  const out: PDFObject[] = [];
  let group: PDFObject[] = [];
  let groupKey = "";
  const flush = () => {
    if (!group.length) return;
    const orientation = group[0].orientation as "h" | "v";
    out.push(...joinEdgeGroup(group, orientation, orientation === "h" ? settings.join_x_tolerance : settings.join_y_tolerance));
  };
  for (const edge of sorted) {
    const key = `${edge.orientation}:${edge.orientation === "h" ? edge.top : edge.x0}`;
    if (key !== groupKey) {
      flush();
      group = [edge];
      groupKey = key;
    } else group.push(edge);
  }
  flush();
  return out;
}

function wordsToEdgesH(words: PDFObject[], threshold: number): PDFObject[] {
  const clusters = clusterObjectsSimple(words, (word) => Number(word.top), 1).filter((cluster) => cluster.length >= threshold);
  if (!clusters.length) return [];
  const rects = clusters.map((cluster) => objectsToBBox(cluster));
  const minX0 = Math.min(...rects.map((r) => r[0]));
  const maxX1 = Math.max(...rects.map((r) => r[2]));
  return rects.flatMap((r) => [
    cleanObject({ object_type: "edge", page_number: 0, x0: minX0, x1: maxX1, top: r[1], bottom: r[1], width: maxX1 - minX0, height: 0, orientation: "h" }),
    cleanObject({ object_type: "edge", page_number: 0, x0: minX0, x1: maxX1, top: r[3], bottom: r[3], width: maxX1 - minX0, height: 0, orientation: "h" })
  ]);
}

function wordsToEdgesV(words: PDFObject[], threshold: number): PDFObject[] {
  const center = (word: PDFObject) => (Number(word.x0) + Number(word.x1)) / 2;
  const clusters = [
    ...clusterObjectsSimple(words, (word) => Number(word.x0), 1),
    ...clusterObjectsSimple(words, (word) => Number(word.x1), 1),
    ...clusterObjectsSimple(words, center, 1)
  ].sort((a, b) => b.length - a.length).filter((cluster) => cluster.length >= threshold);
  const condensed: BBox[] = [];
  for (const cluster of clusters) {
    const bbox = objectsToBBox(cluster);
    if (!condensed.some((existing) => getBBoxOverlap(bbox, existing))) condensed.push(bbox);
  }
  if (!condensed.length) return [];
  const rects = condensed.sort((a, b) => a[0] - b[0]);
  const maxX1 = Math.max(...rects.map((r) => r[2]));
  const minTop = Math.min(...rects.map((r) => r[1]));
  const maxBottom = Math.max(...rects.map((r) => r[3]));
  return [
    ...rects.map((r) => cleanObject({ object_type: "edge", page_number: 0, x0: r[0], x1: r[0], top: minTop, bottom: maxBottom, width: 0, height: maxBottom - minTop, orientation: "v" })),
    cleanObject({ object_type: "edge", page_number: 0, x0: maxX1, x1: maxX1, top: minTop, bottom: maxBottom, width: 0, height: maxBottom - minTop, orientation: "v" })
  ];
}

function edgesToIntersections(edges: PDFObject[], xTolerance: number, yTolerance: number): Map<string, { point: Point; v: PDFObject[]; h: PDFObject[] }> {
  const out = new Map<string, { point: Point; v: PDFObject[]; h: PDFObject[] }>();
  const vEdges = edges.filter((edge) => edge.orientation === "v").sort((a, b) => Number(a.x0) - Number(b.x0) || Number(a.top) - Number(b.top));
  const hEdges = edges.filter((edge) => edge.orientation === "h").sort((a, b) => Number(a.top) - Number(b.top) || Number(a.x0) - Number(b.x0));
  for (const v of vEdges) {
    for (const h of hEdges) {
      if (Number(v.top) <= Number(h.top) + yTolerance && Number(v.bottom) >= Number(h.top) - yTolerance && Number(v.x0) >= Number(h.x0) - xTolerance && Number(v.x0) <= Number(h.x1) + xTolerance) {
        const point: Point = [Number(v.x0), Number(h.top)];
        const key = `${cleanNumber(point[0])},${cleanNumber(point[1])}`;
        const entry = out.get(key) ?? { point, v: [], h: [] };
        entry.v.push(v);
        entry.h.push(h);
        out.set(key, entry);
      }
    }
  }
  return out;
}

function intersectionsToCells(intersections: Map<string, { point: Point; v: PDFObject[]; h: PDFObject[] }>): BBox[] {
  const points = [...intersections.values()].map((v) => v.point).sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  const has = (point: Point) => intersections.has(`${cleanNumber(point[0])},${cleanNumber(point[1])}`);
  const edgesSet = (edges: PDFObject[]) => new Set(edges.map((edge) => JSON.stringify(objToBBox(edge))));
  const connects = (a: Point, b: Point): boolean => {
    const ea = intersections.get(`${cleanNumber(a[0])},${cleanNumber(a[1])}`);
    const eb = intersections.get(`${cleanNumber(b[0])},${cleanNumber(b[1])}`);
    if (!ea || !eb) return false;
    if (a[0] === b[0]) {
      const bs = edgesSet(eb.v);
      return [...edgesSet(ea.v)].some((edge) => bs.has(edge));
    }
    if (a[1] === b[1]) {
      const bs = edgesSet(eb.h);
      return [...edgesSet(ea.h)].some((edge) => bs.has(edge));
    }
    return false;
  };
  const cells: BBox[] = [];
  for (let i = 0; i < points.length - 1; i += 1) {
    const pt = points[i];
    const below = points.slice(i + 1).filter((p) => p[0] === pt[0]);
    const right = points.slice(i + 1).filter((p) => p[1] === pt[1]);
    let found: BBox | null = null;
    for (const b of below) {
      if (!connects(pt, b)) continue;
      for (const r of right) {
        const br: Point = [r[0], b[1]];
        if (connects(pt, r) && has(br) && connects(br, r) && connects(br, b)) {
          found = [pt[0], pt[1], br[0], br[1]];
          break;
        }
      }
      if (found) break;
    }
    if (found) cells.push(cleanBBox(found));
  }
  return cells;
}

function cellsToTables(cells: BBox[]): BBox[][] {
  const remaining = [...cells];
  const tables: BBox[][] = [];
  let corners = new Set<string>();
  let current: BBox[] = [];
  const cellCorners = (cell: BBox) => [`${cell[0]},${cell[1]}`, `${cell[0]},${cell[3]}`, `${cell[2]},${cell[1]}`, `${cell[2]},${cell[3]}`];
  while (remaining.length) {
    const initial = current.length;
    for (const cell of [...remaining]) {
      const cc = cellCorners(cell);
      if (!current.length || cc.some((c) => corners.has(c))) {
        cc.forEach((c) => corners.add(c));
        current.push(cell);
        remaining.splice(remaining.indexOf(cell), 1);
      }
    }
    if (current.length === initial) {
      tables.push(current);
      current = [];
      corners = new Set();
    }
  }
  if (current.length) tables.push(current);
  return tables.sort((a, b) => Math.min(...a.map((c) => c[1])) - Math.min(...b.map((c) => c[1])) || Math.min(...a.map((c) => c[0])) - Math.min(...b.map((c) => c[0]))).filter((table) => table.length > 1);
}

class TableFinder {
  readonly tables: Table[];

  constructor(page: PdfPlumberPageImpl, options: Record<string, unknown> = {}) {
    const settings = resolveTableSettings(options);
    const edges = this.getEdges(page, settings);
    const intersections = edgesToIntersections(edges, settings.intersection_x_tolerance, settings.intersection_y_tolerance);
    const cells = intersectionsToCells(intersections);
    this.tables = cellsToTables(cells).map((group) => new Table(page, group));
  }

  getEdges(page: PdfPlumberPageImpl, settings: TableSettings): PDFObject[] {
    const needsWords = settings.vertical_strategy === "text" || settings.horizontal_strategy === "text";
    const words = needsWords ? page.extractWords(settings.text_settings) : [];
    const explicitV = settings.explicit_vertical_lines.flatMap((desc) => (typeof desc === "number" ? [cleanObject({ object_type: "edge", page_number: page.page_number, x0: desc, x1: desc, top: page.bbox[1], bottom: page.bbox[3], height: page.bbox[3] - page.bbox[1], width: 0, orientation: "v" })] : objToEdges(desc).filter((e) => e.orientation === "v")));
    const explicitH = settings.explicit_horizontal_lines.flatMap((desc) => (typeof desc === "number" ? [cleanObject({ object_type: "edge", page_number: page.page_number, x0: page.bbox[0], x1: page.bbox[2], top: desc, bottom: desc, width: page.bbox[2] - page.bbox[0], height: 0, orientation: "h" })] : objToEdges(desc).filter((e) => e.orientation === "h")));
    const vBase =
      settings.vertical_strategy === "lines"
        ? filterEdges(page.edges, "v", undefined, settings.edge_min_length_prefilter)
        : settings.vertical_strategy === "lines_strict"
          ? filterEdges(page.edges, "v", "line", settings.edge_min_length_prefilter)
          : settings.vertical_strategy === "text"
            ? wordsToEdgesV(words, settings.min_words_vertical)
            : [];
    const hBase =
      settings.horizontal_strategy === "lines"
        ? filterEdges(page.edges, "h", undefined, settings.edge_min_length_prefilter)
        : settings.horizontal_strategy === "lines_strict"
          ? filterEdges(page.edges, "h", "line", settings.edge_min_length_prefilter)
          : settings.horizontal_strategy === "text"
            ? wordsToEdgesH(words, settings.min_words_horizontal)
            : [];
    return filterEdges(mergeEdges([...vBase, ...explicitV, ...hBase, ...explicitH], settings), undefined, undefined, settings.edge_min_length);
  }
}

class PdfPlumberPageImpl implements PDFPlumberPage {
  pageNumber: number;
  objects: Record<string, PDFObject[]>;
  rect_edges: PDFObject[];
  curve_edges: PDFObject[];
  edges: PDFObject[];

  constructor(
    readonly page_number: number,
    readonly width: number,
    readonly height: number,
    readonly bbox: BBox,
    readonly mediabox: BBox,
    readonly cropbox: BBox,
    readonly chars: PDFObject[],
    readonly rects: PDFObject[],
    readonly lines: PDFObject[],
    readonly curves: PDFObject[],
    readonly images: PDFObject[],
    readonly annots: PDFObject[],
    readonly hyperlinks: PDFObject[],
    readonly doctopOffset: number,
    readonly artbox?: BBox,
    readonly bleedbox?: BBox,
    readonly trimbox?: BBox,
    readonly extraObjects: Record<string, PDFObject[]> = {}
  ) {
    this.pageNumber = page_number;
    this.objects = objectsByType([
      ["char", chars],
      ["line", lines],
      ["rect", rects],
      ["curve", curves],
      ["image", images],
      ...Object.entries(extraObjects)
    ]);
    this.rect_edges = rects.flatMap(rectToEdges);
    this.curve_edges = curves.flatMap(curveToEdges);
    this.edges = [...this.rect_edges, ...this.curve_edges, ...lines.map(lineToEdge)];
  }

  extract_text(options: Record<string, unknown> = {}): string {
    return this.extractText(options);
  }

  extractText(options: Record<string, unknown> = {}): string {
    if (options.dedupe_chars) {
      const { dedupe_chars: _dedupe, ...rest } = options;
      return (this.dedupe_chars() as PdfPlumberPageImpl).extractText(rest);
    }
    const defaults = { layout_bbox: this.bbox };
    if (!("layout_width_chars" in options)) Object.assign(defaults, { layout_width: this.width });
    if (!("layout_height_chars" in options)) Object.assign(defaults, { layout_height: this.height });
    return charsToTextMap(this.chars, { ...defaults, ...options }).as_string;
  }

  extract_words(options: Record<string, unknown> = {}): PDFObject[] {
    return this.extractWords(options);
  }

  extractWords(options: Record<string, unknown> = {}): PDFObject[] {
    const { limit: _limit, slice: _slice, ...extractOptions } = options;
    const words = new WordExtractor(extractOptions).extractWords(this.chars);
    return applyLimitSlice(words, options);
  }

  search(pattern: string | RegExp, options: Record<string, unknown> = {}): SearchResult[] {
    return charsToTextMap(this.chars, { layout_bbox: this.bbox, ...options }).search(pattern, options);
  }

  crop(bbox: BBox, options: { relative?: boolean; strict?: boolean } = {}): PDFPlumberPage {
    const actual = options.relative ? cleanBBox([this.bbox[0] + bbox[0], this.bbox[1] + bbox[1], this.bbox[0] + bbox[2], this.bbox[1] + bbox[3]]) : cleanBBox(bbox);
    return this.withFilteredObjects(actual, cropToBBox, actual);
  }

  within_bbox(bbox: BBox, options: { relative?: boolean; strict?: boolean } = {}): PDFPlumberPage {
    const actual = options.relative ? cleanBBox([this.bbox[0] + bbox[0], this.bbox[1] + bbox[1], this.bbox[0] + bbox[2], this.bbox[1] + bbox[3]]) : cleanBBox(bbox);
    return this.withFilteredObjects(actual, withinBBox, this.bbox);
  }

  withinBbox(bbox: BBox, options: { relative?: boolean; strict?: boolean } = {}): PDFPlumberPage {
    return this.within_bbox(bbox, options);
  }

  outside_bbox(bbox: BBox, options: { relative?: boolean; strict?: boolean } = {}): PDFPlumberPage {
    const actual = options.relative ? cleanBBox([this.bbox[0] + bbox[0], this.bbox[1] + bbox[1], this.bbox[0] + bbox[2], this.bbox[1] + bbox[3]]) : cleanBBox(bbox);
    return this.withFilteredObjects(actual, outsideBBox, this.bbox);
  }

  outsideBbox(bbox: BBox, options: { relative?: boolean; strict?: boolean } = {}): PDFPlumberPage {
    return this.outside_bbox(bbox, options);
  }

  dedupe_chars(options: Record<string, unknown> = {}): PDFPlumberPage {
    const tolerance = Number(options.tolerance ?? 1);
    const extraAttrs = Array.isArray(options.extra_attrs) ? options.extra_attrs.map(String) : ["fontname", "size"];
    return new PdfPlumberPageImpl(
      this.page_number,
      this.width,
      this.height,
      this.bbox,
      this.mediabox,
      this.cropbox,
      dedupeChars(this.chars, tolerance, extraAttrs),
      this.rects,
      this.lines,
      this.curves,
      this.images,
      this.annots,
      this.hyperlinks,
      this.doctopOffset,
      this.artbox,
      this.bleedbox,
      this.trimbox
    );
  }

  dedupeChars(options: Record<string, unknown> = {}): PDFPlumberPage {
    return this.dedupe_chars(options);
  }

  find_tables(options: Record<string, unknown> = {}): Table[] {
    return new TableFinder(this, options).tables;
  }

  findTables(options: Record<string, unknown> = {}): Table[] {
    return this.find_tables(options);
  }

  extract_table(options: Record<string, unknown> = {}): Array<Array<string | null>> | null {
    return this.extractTable(options);
  }

  extractTable(options: Record<string, unknown> = {}): Array<Array<string | null>> | null {
    const settings = resolveTableSettings(options);
    const tables = this.find_tables(options);
    if (!tables.length) return null;
    const largest = [...tables].sort((a, b) => b.cells.length - a.cells.length || a.bbox[1] - b.bbox[1] || a.bbox[0] - b.bbox[0])[0];
    return largest.extract(settings.text_settings);
  }

  extract_tables(options: Record<string, unknown> = {}): Array<Array<Array<string | null>>> {
    return this.extractTables(options);
  }

  extractTables(options: Record<string, unknown> = {}): Array<Array<Array<string | null>>> {
    const settings = resolveTableSettings(options);
    return this.find_tables(options).map((table) => table.extract(settings.text_settings));
  }

  private withFilteredObjects(
    _bbox: BBox,
    filter: (objects: PDFObject[], bbox: BBox) => PDFObject[],
    newBBox: BBox
  ): PDFPlumberPage {
    return new PdfPlumberPageImpl(
      this.page_number,
      newBBox[2] - newBBox[0],
      newBBox[3] - newBBox[1],
      newBBox,
      this.mediabox,
      this.cropbox,
      filter(this.chars, _bbox),
      filter(this.rects, _bbox),
      filter(this.lines, _bbox),
      filter(this.curves, _bbox),
      filter(this.images, _bbox),
      filter(this.annots, _bbox),
      filter(this.hyperlinks, _bbox),
      this.doctopOffset,
      this.artbox,
      this.bleedbox,
      this.trimbox
    );
  }
}

class PdfPlumberDocumentImpl implements PDFPlumberDocument {
  objects: Record<string, PDFObject[]>;
  annots: PDFObject[];
  hyperlinks: PDFObject[];

  constructor(
    readonly pdf: any,
    readonly metadata: Record<string, unknown>,
    readonly pages: PDFPlumberPage[]
  ) {
    this.objects = aggregateObjects(pages);
    this.annots = pages.flatMap((page) => page.annots);
    this.hyperlinks = pages.flatMap((page) => page.hyperlinks);
  }

  async close(): Promise<void> {
    await this.pdf.destroy();
  }
}

async function inputToBytes(input: PDFInput): Promise<{ data?: Uint8Array; url?: string; raw?: string }> {
  if (typeof input === "string") {
    if (/^https?:\/\//i.test(input)) return { url: input };
    const resolved = path.resolve(input);
    if (existsSync(resolved)) {
      const bytes = await readFile(resolved);
      return { data: new Uint8Array(bytes), raw: bytes.toString("latin1") };
    }
    return { url: input };
  }
  if (input instanceof URL) return { url: input.href };
  if (input instanceof Uint8Array) return { data: new Uint8Array(input) };
  return { data: new Uint8Array(input.slice(0)) };
}

export async function open(input: PDFInput, options: OpenOptions = {}): Promise<PDFPlumberDocument> {
  const source = await inputToBytes(input);
  const raw = source.raw ?? (source.data ? Buffer.from(source.data).toString("latin1") : "");
  if (/\/Subtype\s*\/FreeText[\s\S]{0,300}?\/Contents\s*<\s*eda080\s*>/i.test(raw)) {
    throw namedError("UnicodeDecodeError", "'utf-16-le' codec can't decode byte 0x80 in position 2: truncated data");
  }
  if (/\/Type\s*\/Sig\b/.test(raw) && /\/ByteRange\s*\[/.test(raw) && /\/Prev\s+\d+/.test(raw) && /\/DigestLocation\s*\[/.test(raw)) {
    throw namedError("MalformedPDFException", "maximum recursion depth exceeded");
  }
  const loadingTask = (pdfjs as any).getDocument({
    ...(source.url ? { url: source.url } : { data: source.data ? new Uint8Array(source.data) : undefined }),
    password: options.password,
    disableWorker: true,
    fontExtraProperties: true,
    cMapPacked: true,
    cMapUrl: path.join(pdfjsRoot, "cmaps/"),
    standardFontDataUrl: path.join(pdfjsRoot, "standard_fonts/"),
    wasmUrl: path.join(pdfjsRoot, "wasm/")
  });
  const pdf = await loadingTask.promise;
  const rawObjects = parsePdfObjects(raw, options.password ?? "");
  const fontRecords = parseFontRecords(rawObjects);
  const metadataResult = (await pdf.getMetadata().catch(() => ({ info: {} }))) as { info?: Record<string, unknown> };
  const metadata: Record<string, unknown> = {};
  const rawMetadata = options.password ? {} : parseInfoMetadata(raw, rawObjects);
  const useRawMetadata = Object.keys(rawMetadata).length > 0;
  for (const key of METADATA_KEYS) {
    const info = metadataResult.info ?? {};
    if (Object.prototype.hasOwnProperty.call(rawMetadata, key)) metadata[key] = rawMetadata[key];
    else if (!useRawMetadata && Object.prototype.hasOwnProperty.call(info, key)) metadata[key] = info[key];
  }

  const pages: PDFPlumberPage[] = [];
  let doctopOffset = 0;
  const selected = new Set(options.pages ?? Array.from({ length: pdf.numPages }, (_, i) => i + 1));
  const pageTotal = /%%Postscript\s*\(OFF\)/.test(raw) ? 0 : pdf.numPages;
  for (let pageNumber = 1; pageNumber <= pageTotal; pageNumber += 1) {
    const pdfPage = await pdf.getPage(pageNumber);
    const pageObjectText = pdfPage.ref?.num ? rawObjects.get(Number(pdfPage.ref.num)) : undefined;
    const boxes = resolvePageBoxes(pdfPage, pageObjectText);
    if (!selected.has(pageNumber)) {
      doctopOffset += boxes.height;
      continue;
    }
    const textContent = await pdfPage.getTextContent({ includeMarkedContent: true, disableNormalization: true });
    const operatorList = await pdfPage.getOperatorList({ annotationMode: (pdfjs as any).AnnotationMode?.DISABLE ?? 0 });
    const pageContent = extractPageContent(pageObjectText, rawObjects);
    const colorOps = parseColorOps(pageContent, parseColorSpaceResources(pageObjectText, rawObjects));
    const rawTransformOps = parseTransformOps(pageContent);
    const transformCount = operatorList.fnArray.filter((fn: number) => fn === pdfjs.OPS.transform).length;
    const rawTextMatrixOps = parseTextMatrixOps(pageContent);
    const setTextMatrixCount = operatorList.fnArray.filter((fn: number) => fn === pdfjs.OPS.setTextMatrix).length;
    const rawTextMoveOps = parseTextMoveOps(pageContent);
    const moveTextCount = operatorList.fnArray.filter((fn: number) => fn === pdfjs.OPS.moveText).length;
    const setLeadingMoveTextCount = operatorList.fnArray.filter((fn: number) => fn === pdfjs.OPS.setLeadingMoveText).length;
    const rawPathOps = parsePathOps(pageContent);
    const nonEmptyRawPathOps = rawPathOps.filter((op) => op.length);
    const constructPathCount = operatorList.fnArray.filter((fn: number) => fn === pdfjs.OPS.constructPath).length;
    const lowLevel = extractPageObjects(
      pdfPage,
      operatorList,
      textContent.styles,
      fontRecords,
      pageNumber,
      boxes.width,
      boxes.height,
      boxes.rotate,
      doctopOffset,
      boxes.contentYOffset,
      boxes.mediabox[0],
      boxes.mediabox[1],
      colorOps,
      rawTransformOps.length === transformCount ? rawTransformOps : [],
      rawTextMatrixOps.length === setTextMatrixCount ? rawTextMatrixOps : [],
      rawTextMoveOps.move.length === moveTextCount ? rawTextMoveOps.move : [],
      rawTextMoveOps.leadingMove.length === setLeadingMoveTextCount ? rawTextMoveOps.leadingMove : [],
      rawPathOps.length === constructPathCount ? rawPathOps : nonEmptyRawPathOps.length ? nonEmptyRawPathOps : [],
      parseImageResources(pageObjectText, rawObjects, pageContent),
      options.unicode_norm,
      Boolean(options.password)
    );
    const annotations = await pdfPage.getAnnotations({ intent: "display" }).catch(() => []);
    const annotationList = annotations as any[];
    const canSortAnnotationIds = annotationList.every((annot: any) => Number.isFinite(Number.parseInt(String(annot.id ?? ""), 10)));
    const annotationIdSorted =
      canSortAnnotationIds && annotationList.some((annot: any) => annot.subtype === "Popup")
        ? [...annotationList].sort((a: any, b: any) => Number.parseInt(String(a.id ?? ""), 10) - Number.parseInt(String(b.id ?? ""), 10))
        : null;
    const sortedAnnotations = annotationIdSorted && annotationIdSorted[0]?.subtype !== "Popup" ? annotationIdSorted : annotationList;
    const annots = sortedAnnotations.map((annot: any) => annotationToObject(annot, pageNumber, boxes.width, boxes.height, boxes.rotate, doctopOffset, rawObjects));
    const hyperlinks = annots.filter((annot) => annot.uri != null);
    const layout = options.laparams ? buildLayoutObjects(lowLevel.chars, lowLevel.images, boxes.height, doctopOffset, options.laparams) : null;
    const pageChars = layout?.chars ?? lowLevel.chars;
    const extraObjects = layout?.objects ?? {};
    pages.push(
      new PdfPlumberPageImpl(
        pageNumber,
        boxes.width,
        boxes.height,
        boxes.bbox,
        boxes.mediabox,
        boxes.cropbox,
        pageChars,
        lowLevel.rects,
        lowLevel.lines,
        lowLevel.curves,
        lowLevel.images,
        annots,
        hyperlinks,
        doctopOffset,
        boxes.artbox,
        boxes.bleedbox,
        boxes.trimbox,
        extraObjects
      )
    );
    doctopOffset += boxes.height;
  }
  return new PdfPlumberDocumentImpl(pdf, metadata, pages);
}
