import type { BBox, PDFObject, PDFPlumberPage, SearchResult } from "./types.js";
import { Table, TableFinder, resolveTableSettings } from "./table.js";
import { charsToTextMap, dedupeChars, WordExtractor } from "./text.js";
import { applyLimitSlice } from "./layout.js";
import {
  cleanBBox,
  cropToBBox,
  curveToEdges,
  lineToEdge,
  objectsByType,
  rectToEdges,
  outsideBBox,
  withinBBox
} from "./utils.js";

export class PdfPlumberPageImpl implements PDFPlumberPage {
  pageNumber: number;
  objects: Record<string, PDFObject[]>;
  rect_edges: PDFObject[];
  curve_edges: PDFObject[];
  edges: PDFObject[];
  private readonly _annots: PDFObject[];
  private readonly _hyperlinks: PDFObject[];

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
    annots: PDFObject[],
    hyperlinks: PDFObject[],
    readonly doctopOffset: number,
    readonly artbox?: BBox,
    readonly bleedbox?: BBox,
    readonly trimbox?: BBox,
    readonly extraObjects: Record<string, PDFObject[]> = {},
    private readonly annotsError: Error | null = null
  ) {
    this.pageNumber = page_number;
    this._annots = annots;
    this._hyperlinks = hyperlinks;
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

  get annots(): PDFObject[] {
    if (this.annotsError) throw this.annotsError;
    return this._annots;
  }

  get hyperlinks(): PDFObject[] {
    if (this.annotsError) throw this.annotsError;
    return this._hyperlinks;
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
      this.trimbox,
      this.extraObjects,
      this.annotsError
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
