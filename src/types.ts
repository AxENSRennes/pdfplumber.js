import type { Table } from "./table.js";

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

export type Dir = "ttb" | "btt" | "ltr" | "rtl";
export type Matrix = [number, number, number, number, number, number];
export type Point = [number, number];
export type MutableBBox = [number, number, number, number];

export interface PdfJsGlyph {
  originalCharCode?: number;
  unicode?: string;
  width?: number;
  isSpace?: boolean;
  isInFont?: boolean;
  vmetric?: number[];
}

export interface FontRecord {
  objectNumber: number;
  baseFont: string;
  subtype?: string;
  hasToUnicode?: boolean;
  symbolic?: boolean;
  charSet?: string[];
  firstChar: number;
  widths: number[];
  ascent?: number;
  descent?: number;
}

export interface MappedFont {
  fontname: string;
  ascent: number;
  descent: number;
  fontMatrix0: number;
  vertical: boolean;
  cidFallback: boolean;
}

export interface PageBoxes {
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

export interface GraphicsState {
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

export interface ParsedPath {
  points: Point[];
  closed: boolean;
  hasCurve: boolean;
  lastOp: number;
  fromRect?: boolean;
  forceCurve?: boolean;
}

export interface ColorOp {
  target: "fill" | "stroke";
  components: number[];
  colorSpace: string;
  pattern?: string;
}

export interface ImageResource {
  name: string;
  width?: number;
  height?: number;
  bits?: number;
  colorspace?: unknown[];
}

export interface PdfEncryption {
  encryptRef: number | null;
  key: Uint8Array;
  keyLength: number;
  streamMethod: "RC4" | "Identity" | "Unsupported";
  encryptMetadata: boolean;
}
