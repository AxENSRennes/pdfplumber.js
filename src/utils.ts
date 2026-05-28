import type { BBox, Dir, Matrix, MutableBBox, PDFObject, PDFPlumberPage, Point } from "./types.js";

export function cleanNumber(value: number): number {
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

export function cleanBBox(bbox: readonly number[]): BBox {
  return [cleanNumber(bbox[0]), cleanNumber(bbox[1]), cleanNumber(bbox[2]), cleanNumber(bbox[3])];
}

export function cleanMatrix(matrix: Matrix): Matrix {
  return matrix.map((value) => {
    const rounded = Number(value.toFixed(6));
    return Object.is(rounded, -0) ? 0 : rounded;
  }) as Matrix;
}

export function firstFinite(fallback: number, ...values: unknown[]): number {
  for (const value of values) {
    if (typeof value === "number" && Number.isFinite(value)) return value;
  }
  return fallback;
}

export function safeFloatLikePdfminer(value: unknown): number | null {
  if (value == null) return null;
  if (typeof value === "string" && value.trim() === "") return null;
  try {
    const number = Number(value);
    return Number.isFinite(number) ? number : null;
  } catch {
    return null;
  }
}

export function safeRectListLikePdfminer(value: unknown): MutableBBox | null {
  if (value == null || typeof (value as Iterable<unknown>)[Symbol.iterator] !== "function") return null;
  const values = Array.from(value as Iterable<unknown>).slice(0, 4);
  if (values.length !== 4) return null;
  const rect = values.map(safeFloatLikePdfminer);
  if (rect.some((item) => item == null)) return null;
  return rect as MutableBBox;
}

export function shortenStrLikePdfminer(value: string, size: number): string {
  if (size < 7) return value.slice(0, size);
  if (value.length <= size) return value;
  const length = Math.floor((size - 5) / 2);
  return `${value.slice(0, length)} ... ${value.slice(-length)}`;
}

export function formatIntAlphaLikePdfminer(value: number): string {
  if (!Number.isInteger(value) || value <= 0) throw new RangeError("formatIntAlphaLikePdfminer expects a positive integer");
  const letters = "abcdefghijklmnopqrstuvwxyz";
  let current = value;
  let result = "";
  while (current !== 0) {
    const next = Math.floor((current - 1) / letters.length);
    const remainder = (current - 1) % letters.length;
    result = `${letters[remainder]}${result}`;
    current = next;
  }
  return result;
}

export function formatIntRomanLikePdfminer(value: number): string {
  if (!Number.isInteger(value) || value <= 0 || value >= 4000) throw new RangeError("formatIntRomanLikePdfminer expects an integer from 1 to 3999");
  const ones = ["i", "x", "c", "m"];
  const fives = ["v", "l", "d"];
  let current = value;
  let index = 0;
  let result = "";
  while (current !== 0) {
    const remainder = current % 10;
    current = Math.floor(current / 10);
    let part = "";
    if (remainder === 9) part = `${ones[index]}${ones[index + 1]}`;
    else if (remainder === 4) part = `${ones[index]}${fives[index]}`;
    else {
      const overFive = remainder >= 5;
      part = `${overFive ? fives[index] : ""}${ones[index].repeat(overFive ? remainder - 5 : remainder)}`;
    }
    result = `${part}${result}`;
    index += 1;
  }
  return result;
}

function discreteRangeLikePdfminer(v0: number, v1: number, size: number): number[] {
  const out: number[] = [];
  for (let value = Math.floor(Math.trunc(v0) / size); value < Math.floor(Math.trunc(v1 + size) / size); value += 1) out.push(value);
  return out;
}

export interface PlaneObjectLikePdfminer {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

export class PlaneLikePdfminer<T extends PlaneObjectLikePdfminer> {
  private readonly seq: T[] = [];
  private readonly objs = new Set<T>();
  private readonly grid = new Map<string, T[]>();
  private readonly x0: number;
  private readonly y0: number;
  private readonly x1: number;
  private readonly y1: number;

  constructor(bbox: MutableBBox, private readonly gridsize = 50) {
    [this.x0, this.y0, this.x1, this.y1] = bbox;
  }

  get size(): number {
    return this.objs.size;
  }

  has(obj: T): boolean {
    return this.objs.has(obj);
  }

  values(): T[] {
    return this.seq.filter((obj) => this.objs.has(obj));
  }

  add(obj: T): void {
    for (const key of this.getRange([obj.x0, obj.y0, obj.x1, obj.y1])) {
      const bucket = this.grid.get(key);
      if (bucket) bucket.push(obj);
      else this.grid.set(key, [obj]);
    }
    this.seq.push(obj);
    this.objs.add(obj);
  }

  remove(obj: T): void {
    for (const key of this.getRange([obj.x0, obj.y0, obj.x1, obj.y1])) {
      const bucket = this.grid.get(key);
      const index = bucket?.indexOf(obj) ?? -1;
      if (bucket && index >= 0) bucket.splice(index, 1);
    }
    this.objs.delete(obj);
  }

  find(bbox: MutableBBox): T[] {
    const [x0, y0, x1, y1] = bbox;
    const done = new Set<T>();
    const out: T[] = [];
    for (const key of this.getRange(bbox)) {
      const bucket = this.grid.get(key);
      if (!bucket) continue;
      for (const obj of bucket) {
        if (done.has(obj)) continue;
        done.add(obj);
        if (obj.x1 <= x0 || x1 <= obj.x0 || obj.y1 <= y0 || y1 <= obj.y0) continue;
        out.push(obj);
      }
    }
    return out;
  }

  private getRange(bbox: MutableBBox): string[] {
    let [x0, y0, x1, y1] = bbox;
    if (x1 <= this.x0 || this.x1 <= x0 || y1 <= this.y0 || this.y1 <= y0) return [];
    x0 = Math.max(this.x0, x0);
    y0 = Math.max(this.y0, y0);
    x1 = Math.min(this.x1, x1);
    y1 = Math.min(this.y1, y1);
    const out: string[] = [];
    for (const gridY of discreteRangeLikePdfminer(y0, y1, this.gridsize)) {
      for (const gridX of discreteRangeLikePdfminer(x0, x1, this.gridsize)) out.push(`${gridX},${gridY}`);
    }
    return out;
  }
}

export function snapPdfCoordinate(value: number): number {
  if (!Number.isFinite(value)) return value;
  if (Math.abs(value) < 1) return value;
  for (let digits = 0; digits <= 4; digits += 1) {
    const rounded = Number(value.toFixed(digits));
    const tolerance = digits <= 2 ? 0.000075 : 0.00003;
    if (Math.abs(value - rounded) <= tolerance) return rounded;
  }
  return value;
}

export function snapPdfPathCoordinate(value: number): number {
  if (!Number.isFinite(value)) return value;
  if (Math.abs(value) < 1) return value;
  for (let digits = 0; digits <= 3; digits += 1) {
    const rounded = Number(value.toFixed(digits));
    const tolerance = digits <= 2 ? 0.000075 : 0.000035;
    if (Math.abs(value - rounded) <= tolerance) return rounded;
  }
  return value;
}

export function snapPdfOperand(value: number): number {
  if (!Number.isFinite(value)) return value;
  for (let digits = 0; digits <= 4; digits += 1) {
    const rounded = Number(value.toFixed(digits));
    const tolerance = digits <= 2 ? 0.000075 : 0.00003;
    if (Math.abs(value - rounded) <= tolerance) return rounded;
  }
  return value;
}

export function snapRawTextMatrixOperand(value: number): number {
  if (!Number.isFinite(value)) return value;
  for (let digits = 0; digits <= 3; digits += 1) {
    const rounded = Number(value.toFixed(digits));
    const tolerance = digits <= 2 ? 0.000075 : 0.00003;
    if (Math.abs(value - rounded) <= tolerance) return rounded;
  }
  return value;
}

export function snapPdfExtent(value: number): number {
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

export function snapPdfPathExtent(value: number): number {
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

export function cloneMatrix(matrix: Matrix): Matrix {
  return [...matrix] as Matrix;
}

export function multiplyMatrix(a: Matrix, b: Matrix): Matrix {
  return [
    a[0] * b[0] + a[2] * b[1],
    a[1] * b[0] + a[3] * b[1],
    a[0] * b[2] + a[2] * b[3],
    a[1] * b[2] + a[3] * b[3],
    a[0] * b[4] + a[2] * b[5] + a[4],
    a[1] * b[4] + a[3] * b[5] + a[5]
  ];
}

export function lineWidthScale(matrix: Matrix): number {
  const scale = Math.hypot(matrix[0], matrix[1]);
  return scale || 1;
}

export function applyMatrix(point: Point, matrix: Matrix, snap = false): Point {
  const [x, y] = point;
  const transformed: Point = [x * matrix[0] + y * matrix[2] + matrix[4], x * matrix[1] + y * matrix[3] + matrix[5]];
  return snap ? [snapPdfCoordinate(transformed[0]), snapPdfCoordinate(transformed[1])] : transformed;
}

export function translateMatrixLikePdfminer(matrix: Matrix, point: Point): Matrix {
  const [x, y] = point;
  return [matrix[0], matrix[1], matrix[2], matrix[3], x * matrix[0] + y * matrix[2] + matrix[4], x * matrix[1] + y * matrix[3] + matrix[5]];
}

export function bboxFromPoints(points: Point[]): MutableBBox {
  const xs = points.map((p) => p[0]);
  const ys = points.map((p) => p[1]);
  return [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)];
}

export function applyMatrixRectLikePdfminer(matrix: Matrix, rect: MutableBBox): MutableBBox {
  return bboxFromPoints([
    applyMatrix([rect[0], rect[1]], matrix),
    applyMatrix([rect[2], rect[1]], matrix),
    applyMatrix([rect[2], rect[3]], matrix),
    applyMatrix([rect[0], rect[3]], matrix)
  ]);
}

export function snapPathBBox(bbox: MutableBBox): MutableBBox {
  const x0 = snapPdfPathCoordinate(bbox[0]);
  const y0 = snapPdfPathCoordinate(bbox[1]);
  return [x0, y0, x0 + snapPdfPathExtent(bbox[2] - bbox[0]), y0 + snapPdfPathExtent(bbox[3] - bbox[1])];
}

export function pathBBoxHasMeaningfulPrecision(bbox: MutableBBox): boolean {
  return bbox.some((value) => Number.isFinite(value) && Math.abs(value) >= 1 && Math.abs(value - Number(value.toFixed(2))) > 0.000075);
}

export function snapPathOperandPoint(point: Point): Point {
  return [snapPdfPathCoordinate(point[0]), snapPdfPathCoordinate(point[1])];
}

export function pointToPageCoords(point: Point, pageWidth: number, pageHeight: number, rotate: number): Point {
  const normalized = ((rotate % 360) + 360) % 360;
  const [x, y] = point;
  if (normalized === 90) return [y, x];
  if (normalized === 180) return [pageWidth - x, y];
  if (normalized === 270) return [pageWidth - y, pageHeight - x];
  return [x, pageHeight - y];
}

export function bboxToPageBBox(raw: MutableBBox, pageWidth: number, pageHeight: number, rotate: number): MutableBBox {
  return bboxFromPoints([
    pointToPageCoords([raw[0], raw[1]], pageWidth, pageHeight, rotate),
    pointToPageCoords([raw[0], raw[3]], pageWidth, pageHeight, rotate),
    pointToPageCoords([raw[2], raw[1]], pageWidth, pageHeight, rotate),
    pointToPageCoords([raw[2], raw[3]], pageWidth, pageHeight, rotate)
  ]);
}

export function matrixToPageMatrix(matrix: Matrix, pageWidth: number, pageHeight: number, rotate: number): Matrix {
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

export function rectFromPdfBBox(
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
  const preferPageYFromTop = Boolean(extras.__pdfplumber_page_y_from_top);
  const outputExtras = { ...extras };
  delete outputExtras.__pdfplumber_page_y_from_top;
  const [rawX0, rawTop, rawX1, rawBottom] = bboxToPageBBox(raw, pageWidth, pageHeight, pageRotate);
  const x0 = liftPostHalfThousandthDrift(softenHalfThousandth(softenHalfMicro(rawX0)));
  const top = liftPostHalfThousandthDrift(softenHalfThousandth(softenHalfMicro(rawTop)));
  const x1 = liftPostHalfThousandthDrift(softenHalfThousandth(softenHalfMicro(rawX1)));
  const bottom = liftPostHalfThousandthDrift(softenHalfThousandth(softenHalfMicro(rawBottom)));
  const [rawY0, rawY1] =
    ((pageRotate % 360) + 360) % 360 === 0 && !preferPageYFromTop
      ? [raw[1] + coordOffset[1], raw[3] + coordOffset[1]]
      : [pageHeight - bottom + coordOffset[1], pageHeight - top + coordOffset[1]];
  const y0 = liftPostHalfThousandthDrift(softenHalfThousandth(softenHalfMicro(rawY0)));
  const y1 = liftPostHalfThousandthDrift(softenHalfThousandth(softenHalfMicro(rawY1)));
  const obj = {
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
    ...outputExtras
  };
  Object.defineProperty(obj, "__pdfplumber_raw_bbox", {
    value: [rawX0, rawTop, rawX1, rawBottom],
    enumerable: false,
    configurable: true
  });
  return obj;
}

export function rectFromPageBBox(raw: MutableBBox, pageHeight: number, pageNumber: number, objectType: string, doctopOffset: number, extras: Record<string, unknown> = {}): PDFObject {
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

export function cleanObject<T extends Record<string, unknown>>(obj: T): T {
  const out: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(obj)) {
    if (typeof value === "number") out[key] = cleanNumber(value);
    else if (Array.isArray(value)) out[key] = value.map((v) => (typeof v === "number" ? cleanNumber(v) : v));
    else out[key] = value;
  }
  return out as T;
}

export function softenHalfMicro(value: number): number {
  if (!Number.isFinite(value)) return value;
  const scaled = Math.abs(value) * 1_000_000;
  const fraction = scaled - Math.floor(scaled);
  if (Math.abs(fraction - 0.5) > 1e-7) return value;
  return value - Math.sign(value || 1) * 1e-9;
}

export function softenHalfThousandth(value: number): number {
  if (!Number.isFinite(value)) return value;
  const scaled = Math.abs(value) * 1_000;
  const fraction = scaled - Math.floor(scaled);
  if (Math.abs(fraction - 0.5) > 1e-11) return value;
  return value - Math.sign(value || 1) * 1e-9;
}

export function liftPostHalfThousandthDrift(value: number): number {
  if (!Number.isFinite(value)) return value;
  const scaled = Math.abs(value) * 1_000;
  const fraction = scaled - Math.floor(scaled);
  if (fraction <= 0.500499000005 || fraction >= 0.5 + 0.0005) return value;
  return value + Math.sign(value || 1) * 2e-9;
}

export function colorValue(value: unknown): unknown {
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

export function graphicColorValue(value: unknown, colorSpace: string): unknown {
  if (colorSpace === "DeviceCMYK" && Array.isArray(value) && value.length === 1 && typeof value[0] === "number") {
    return [0, 0, 0, cleanNumber(value[0])];
  }
  if (colorSpace === "DeviceCMYK" && typeof value === "number") return [0, 0, 0, cleanNumber(value)];
  if (Array.isArray(value) && value.length === 1 && typeof value[0] === "number") return cleanNumber(value[0]);
  if (colorSpace === "DeviceGray" && Array.isArray(value) && value.length === 1 && typeof value[0] === "number") return cleanNumber(value[0]);
  if (colorSpace === "DeviceGray" && typeof value === "string" && /^#[0-9a-f]{6}$/i.test(value)) {
    const rgb = rgbColor(value);
    if (Math.abs(rgb[0] - rgb[1]) < 1e-9 && Math.abs(rgb[1] - rgb[2]) < 1e-9) return rgb[0];
  }
  return colorValue(value);
}

export function colorSpaceName(value: unknown): string {
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

export function pythonBytesName(value: string): string {
  if (!/[^\x20-\x7e]/.test(value)) return value;
  if ([...value].some((char) => char.charCodeAt(0) > 0xff)) return value;
  const bytes = Uint8Array.from([...value].map((char) => char.charCodeAt(0) & 0xff));
  const decoded = new TextDecoder("utf-8", { fatal: false }).decode(bytes);
  if (decoded.includes("\uFFFD") === false && /[^\x20-\x7e]/.test(decoded)) return decoded;
  return [...value].map((char) => {
    const code = char.charCodeAt(0) & 0xff;
    if (code === 0x5c) return "\\\\";
    if (code === 0x27) return "\\'";
    return code >= 0x20 && code <= 0x7e ? String.fromCharCode(code) : `\\x${code.toString(16).padStart(2, "0")}`;
  }).join("");
}

export function rgbColor(value: unknown): number[] {
  if (typeof value === "string" && /^#[0-9a-f]{6}$/i.test(value)) {
    return [parseInt(value.slice(1, 3), 16) / 255, parseInt(value.slice(3, 5), 16) / 255, parseInt(value.slice(5, 7), 16) / 255].map(cleanNumber);
  }
  return Array.isArray(value) ? value.map(Number).map(cleanNumber) : [0, 0, 0];
}

export function objectsByType(entries: Array<[string, PDFObject[]]>): Record<string, PDFObject[]> {
  const out: Record<string, PDFObject[]> = {};
  for (const [key, value] of entries) {
    if (value.length > 0) out[key] = value;
  }
  return out;
}

export function aggregateObjects(pages: PDFPlumberPage[]): Record<string, PDFObject[]> {
  const out: Record<string, PDFObject[]> = {};
  for (const page of pages) {
    for (const [key, value] of Object.entries(page.objects)) {
      out[key] ??= [];
      out[key].push(...value);
    }
  }
  return out;
}

export function clusterList(xs: number[], tolerance = 0): number[][] {
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

export function clusterObjectsSimple<T>(items: T[], key: (item: T) => number, tolerance: number, preserveOrder = false): T[][] {
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

export function objectsToBBox(objects: PDFObject[]): BBox {
  return [
    Math.min(...objects.map((o) => Number(o.x0))),
    Math.min(...objects.map((o) => Number(o.top))),
    Math.max(...objects.map((o) => Number(o.x1))),
    Math.max(...objects.map((o) => Number(o.bottom)))
  ];
}

export function getBBoxOverlap(a: BBox, b: BBox): BBox | null {
  const left = Math.max(a[0], b[0]);
  const right = Math.min(a[2], b[2]);
  const top = Math.max(a[1], b[1]);
  const bottom = Math.min(a[3], b[3]);
  if (bottom >= top && right >= left && bottom - top + (right - left) > 0) return [left, top, right, bottom];
  return null;
}

export function objToBBox(obj: PDFObject): BBox {
  return [Number(obj.x0), Number(obj.top), Number(obj.x1), Number(obj.bottom)];
}

export function clipObj(obj: PDFObject, bbox: BBox): PDFObject | null {
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

export function cropToBBox(objects: PDFObject[], bbox: BBox): PDFObject[] {
  return objects.map((obj) => clipObj(obj, bbox)).filter((obj): obj is PDFObject => obj !== null);
}

export function withinBBox(objects: PDFObject[], bbox: BBox): PDFObject[] {
  return objects.filter((obj) => {
    const overlap = getBBoxOverlap(objToBBox(obj), bbox);
    return overlap !== null && JSON.stringify(cleanBBox(overlap)) === JSON.stringify(cleanBBox(objToBBox(obj)));
  });
}

export function outsideBBox(objects: PDFObject[], bbox: BBox): PDFObject[] {
  return objects.filter((obj) => getBBoxOverlap(objToBBox(obj), bbox) === null);
}

export function resizeObject(obj: PDFObject, key: "x0" | "x1" | "top" | "bottom", value: number): PDFObject {
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

export function moveObject(obj: PDFObject, axis: "h" | "v", value: number): PDFObject {
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

export function snapObjects(objects: PDFObject[], attr: "x0" | "x1" | "top" | "bottom", tolerance: number): PDFObject[] {
  const axis = attr === "x0" || attr === "x1" ? "h" : "v";
  return clusterObjectsSimple(objects, (obj) => Number(obj[attr]), tolerance).flatMap((cluster) => {
    const avg = cluster.reduce((sum, obj) => sum + Number(obj[attr]), 0) / cluster.length;
    return cluster.map((obj) => moveObject(obj, axis, avg - Number(obj[attr])));
  });
}

export function rectToEdges(rect: PDFObject): PDFObject[] {
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

export function curveToEdges(curve: PDFObject): PDFObject[] {
  const pts = (curve.pts as Point[] | undefined) ?? [];
  const edgePoints = [...pts];
  if (curve.closepath === true && pts.length === 1) {
    edgePoints.push(pts[0]);
  } else if (curve.closepath === true && pts.length > 1) {
    const first = pts[0];
    const last = pts[pts.length - 1];
    if (first[0] !== last[0] || first[1] !== last[1]) edgePoints.push(first);
  }
  const edges: PDFObject[] = [];
  for (let i = 0; i < edgePoints.length - 1; i += 1) {
    const [p0, p1] = [edgePoints[i], edgePoints[i + 1]];
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

export function lineToEdge(line: PDFObject): PDFObject {
  return cleanObject({ ...line, orientation: Number(line.top) === Number(line.bottom) ? "h" : "v" });
}

export function objToEdges(obj: PDFObject): PDFObject[] {
  if (obj.object_type.includes("_edge")) return [obj];
  if (obj.object_type === "line") return [lineToEdge(obj)];
  if (obj.object_type === "rect") return rectToEdges(obj);
  if (obj.object_type === "curve") return curveToEdges(obj);
  return [];
}

export function filterEdges(edges: PDFObject[], orientation?: "h" | "v", edgeType?: string, minLength = 1): PDFObject[] {
  return edges.filter((edge) => {
    const edgeOrientation = edge.orientation as "h" | "v" | undefined;
    const dim = edgeOrientation === "v" ? "height" : "width";
    return (!orientation || edgeOrientation === orientation) && (!edgeType || edge.object_type === edgeType) && Number(edge[dim]) >= minLength;
  });
}

export function getLineClusterKey(lineDir: Dir): (obj: PDFObject) => number {
  return {
    ttb: (x: PDFObject) => Number(x.top),
    btt: (x: PDFObject) => -Number(x.bottom),
    ltr: (x: PDFObject) => Number(x.x0),
    rtl: (x: PDFObject) => -Number(x.x1)
  }[lineDir];
}

export function getCharSortKey(charDir: Dir): (obj: PDFObject) => [number, number] {
  return {
    ttb: (x: PDFObject): [number, number] => [Number(x.top), Number(x.bottom)],
    btt: (x: PDFObject): [number, number] => [-(Number(x.top) + Number(x.height)), -Number(x.top)],
    ltr: (x: PDFObject): [number, number] => [Number(x.x0), Number(x.x0)],
    rtl: (x: PDFObject): [number, number] => [-Number(x.x1), -Number(x.x0)]
  }[charDir];
}

export function bboxOriginKey(dir: Dir, bbox: BBox): number {
  return { ttb: bbox[1], btt: bbox[3], ltr: bbox[0], rtl: bbox[2] }[dir];
}

export function positionKey(dir: Dir, obj: PDFObject): number {
  return { ttb: Number(obj.top), btt: Number(obj.bottom), ltr: Number(obj.x0), rtl: Number(obj.x1) }[dir];
}

export function validateDirections(lineDir: Dir, charDir: Dir): void {
  const valid = new Set(["ltr", "rtl", "ttb", "btt"]);
  if (!valid.has(lineDir) || !valid.has(charDir)) {
    const error = new Error(`Invalid text directions: ${lineDir}, ${charDir}`);
    error.name = "ValueError";
    throw error;
  }
  if (new Set(lineDir).size === new Set([...lineDir, ...charDir]).size && [...lineDir].every((c) => charDir.includes(c))) {
    const error = new Error(`Incompatible text directions: ${lineDir}, ${charDir}`);
    error.name = "ValueError";
    throw error;
  }
}
