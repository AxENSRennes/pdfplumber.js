import type { Table } from "./table.js";

export type BBox = readonly [number, number, number, number];
export type Dir = "ttb" | "btt" | "ltr" | "rtl";
export type TableStrategy = "lines" | "lines_strict" | "text" | "explicit";

export type PDFInput = string | URL | ArrayBuffer | Blob | Uint8Array;

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

export interface LimitSliceOptions {
  limit?: number;
  slice?: readonly [number, number];
}

export interface WordOptions extends LimitSliceOptions {
  x_tolerance?: number;
  y_tolerance?: number;
  x_tolerance_ratio?: number | null;
  y_tolerance_ratio?: number | null;
  keep_blank_chars?: boolean;
  use_text_flow?: boolean;
  horizontal_ltr?: boolean;
  vertical_ttb?: boolean;
  line_dir?: Dir;
  char_dir?: Dir;
  line_dir_rotated?: Dir;
  char_dir_rotated?: Dir;
  extra_attrs?: readonly string[];
  split_at_punctuation?: boolean | string;
  expand_ligatures?: boolean;
  return_chars?: boolean;
}

export interface TextOptions extends WordOptions {
  layout?: boolean;
  layout_width?: number;
  layout_height?: number;
  layout_width_chars?: number;
  layout_height_chars?: number;
  layout_bbox?: BBox;
  x_density?: number;
  y_density?: number;
  x_shift?: number;
  y_shift?: number;
  line_dir_render?: Dir;
  char_dir_render?: Dir;
  presorted?: boolean;
}

export interface ExtractTextOptions extends TextOptions {
  dedupe_chars?: boolean;
}

export interface SearchOptions extends TextOptions {
  regex?: boolean;
  case?: boolean;
  main_group?: number;
  return_groups?: boolean;
  return_chars?: boolean;
}

export interface TextLineOptions extends TextOptions {
  strip?: boolean;
  return_chars?: boolean;
}

export interface CropOptions {
  relative?: boolean;
  strict?: boolean;
}

export interface DedupeOptions {
  tolerance?: number;
  extra_attrs?: readonly string[];
}

export type ExplicitTableLine = number | PDFObject;

export interface TableOptions {
  vertical_strategy?: TableStrategy;
  horizontal_strategy?: TableStrategy;
  explicit_vertical_lines?: readonly ExplicitTableLine[];
  explicit_horizontal_lines?: readonly ExplicitTableLine[];
  snap_tolerance?: number;
  snap_x_tolerance?: number;
  snap_y_tolerance?: number;
  join_tolerance?: number;
  join_x_tolerance?: number;
  join_y_tolerance?: number;
  edge_min_length?: number;
  edge_min_length_prefilter?: number;
  min_words_vertical?: number;
  min_words_horizontal?: number;
  intersection_tolerance?: number;
  intersection_x_tolerance?: number;
  intersection_y_tolerance?: number;
  [textOption: `text_${string}`]: unknown;
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
  label: string | null;
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
  textboxhorizontals: PDFObject[];
  textlinehorizontals: PDFObject[];
  textboxverticals: PDFObject[];
  textlineverticals: PDFObject[];
  figures: PDFObject[];
  annots: PDFObject[];
  hyperlinks: PDFObject[];
  rect_edges: PDFObject[];
  curve_edges: PDFObject[];
  edges: PDFObject[];
  horizontal_edges: PDFObject[];
  vertical_edges: PDFObject[];
  extract_text(options?: ExtractTextOptions): string | Promise<string>;
  extractText(options?: ExtractTextOptions): string | Promise<string>;
  extract_words(options?: WordOptions): PDFObject[] | Promise<PDFObject[]>;
  extractWords(options?: WordOptions): PDFObject[] | Promise<PDFObject[]>;
  search(pattern: string | RegExp, options?: SearchOptions): SearchResult[] | Promise<SearchResult[]>;
  extract_text_lines(options?: TextLineOptions): SearchResult[] | Promise<SearchResult[]>;
  extractTextLines(options?: TextLineOptions): SearchResult[] | Promise<SearchResult[]>;
  filter(testFunction: (object: PDFObject) => boolean): PDFPlumberPage;
  crop(bbox: BBox, options?: CropOptions): PDFPlumberPage;
  within_bbox(bbox: BBox, options?: CropOptions): PDFPlumberPage;
  withinBbox(bbox: BBox, options?: CropOptions): PDFPlumberPage;
  outside_bbox(bbox: BBox, options?: CropOptions): PDFPlumberPage;
  outsideBbox(bbox: BBox, options?: CropOptions): PDFPlumberPage;
  dedupe_chars(options?: DedupeOptions): PDFPlumberPage;
  dedupeChars(options?: DedupeOptions): PDFPlumberPage;
  extract_table(options?: TableOptions): (Array<Array<string | null>> | null) | Promise<Array<Array<string | null>> | null>;
  extractTable(options?: TableOptions): (Array<Array<string | null>> | null) | Promise<Array<Array<string | null>> | null>;
  extract_tables(options?: TableOptions): Array<Array<Array<string | null>>> | Promise<Array<Array<Array<string | null>>>>;
  extractTables(options?: TableOptions): Array<Array<Array<string | null>>> | Promise<Array<Array<Array<string | null>>>>;
  find_table(options?: TableOptions): Table | null | Promise<Table | null>;
  findTable(options?: TableOptions): Table | null | Promise<Table | null>;
  find_tables(options?: TableOptions): Table[] | Promise<Table[]>;
  findTables(options?: TableOptions): Table[] | Promise<Table[]>;
}

export interface PDFPlumberDocument {
  metadata: Record<string, unknown>;
  pages: PDFPlumberPage[];
  objects: Record<string, PDFObject[]>;
  annots: PDFObject[];
  hyperlinks: PDFObject[];
  edges: PDFObject[];
  horizontal_edges: PDFObject[];
  vertical_edges: PDFObject[];
  close(): void | Promise<void>;
}

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
  cidCoding?: string;
  encodingName?: string;
  hasToUnicode?: boolean;
  symbolic?: boolean;
  charSet?: string[];
  encodingDifferences?: Record<number, string>;
  embeddedUnicodeMap?: Record<number, string>;
  pageScoped?: boolean;
  firstChar: number;
  widths: number[];
  ascent?: number;
  descent?: number;
  vertical?: boolean;
}

export interface MappedFont {
  fontname: string;
  ascent: number;
  descent: number;
  fontMatrix0: number;
  vertical: boolean;
  cidFallback: boolean;
  fontRecord?: FontRecord;
  hasToUnicode?: boolean;
  missingFile?: boolean;
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
  formMatrixApplied?: boolean;
}

export interface ParsedPath {
  points: Point[];
  operations: Array<[string, ...Point[]]>;
  closed: boolean;
  hasCurve: boolean;
  lastOp: number;
  fromRect?: boolean;
  explicitClose?: boolean;
  trailingMove?: boolean;
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
  imagemask?: boolean | null;
}

export interface PdfEncryption {
  encryptRef: number | null;
  key: Uint8Array;
  keyLength: number;
  streamMethod: "RC4" | "AESV2" | "AESV3" | "Identity" | "Unsupported";
  encryptMetadata: boolean;
}
