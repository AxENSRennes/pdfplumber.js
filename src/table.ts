import type { BBox, PDFObject, PDFPlumberPage, Point } from "./types.js";
import { extractTextFromChars } from "./text.js";
import {
  cleanBBox,
  cleanNumber,
  cleanObject,
  clusterObjectsSimple,
  filterEdges,
  getBBoxOverlap,
  objToBBox,
  objToEdges,
  objectsToBBox,
  resizeObject,
  snapObjects
} from "./utils.js";

type TablePage = Pick<PDFPlumberPage, "page_number" | "bbox" | "chars" | "edges"> & {
  extractWords(options?: Record<string, unknown>): PDFObject[];
};

const TEXT_EDGE_CLUSTER_TOLERANCE = 1 - 1e-12;
const TEXT_VERTICAL_EDGE_CLUSTER_TOLERANCE = 1;

export class TableAxisGroup {
  [index: number]: BBox | null;

  constructor(readonly cells: Array<BBox | null>) {
    cells.forEach((cell, index) => {
      this[index] = cell;
    });
  }

  get length(): number {
    return this.cells.length;
  }
}

export class Table {
  constructor(
    readonly page: TablePage,
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

  private rowCells(): Array<Array<BBox | null>> {
    return this.groups(0);
  }

  get rows(): TableAxisGroup[] {
    return this.rowCells().map((cells) => new TableAxisGroup(cells));
  }

  get columns(): TableAxisGroup[] {
    return this.groups(1).map((cells) => new TableAxisGroup(cells));
  }

  extract(options: Record<string, unknown> = {}): Array<Array<string | null>> {
    const charInBBox = (char: PDFObject, bbox: BBox): boolean => {
      const vMid = (Number(char.top) + Number(char.bottom)) / 2;
      const hMid = (Number(char.x0) + Number(char.x1)) / 2;
      return hMid >= bbox[0] && hMid < bbox[2] && vMid >= bbox[1] && vMid < bbox[3];
    };
    return this.rowCells().map((row) => {
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

export interface TableSettings {
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

export function resolveTableSettings(options: Record<string, unknown> = {}): TableSettings {
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
  const clusters = clusterObjectsSimple(words, (word) => rawObjBBox(word)[1], TEXT_EDGE_CLUSTER_TOLERANCE).filter((cluster) => cluster.length >= threshold);
  if (!clusters.length) return [];
  const rects = clusters.map((cluster) => objectsToRawBBox(cluster));
  const minX0 = Math.min(...rects.map((r) => r[0]));
  const maxX1 = Math.max(...rects.map((r) => r[2]));
  return rects.flatMap((r) => [
    cleanObject({ object_type: "edge", page_number: 0, x0: minX0, x1: maxX1, top: r[1], bottom: r[1], width: maxX1 - minX0, height: 0, orientation: "h" }),
    cleanObject({ object_type: "edge", page_number: 0, x0: minX0, x1: maxX1, top: r[3], bottom: r[3], width: maxX1 - minX0, height: 0, orientation: "h" })
  ]);
}

function wordsToEdgesV(words: PDFObject[], threshold: number): PDFObject[] {
  const center = (word: PDFObject) => {
    const bbox = rawObjBBox(word);
    return (bbox[0] + bbox[2]) / 2;
  };
  const clusters = [
    ...clusterObjectsSimple(words, (word) => rawObjBBox(word)[0], TEXT_VERTICAL_EDGE_CLUSTER_TOLERANCE),
    ...clusterObjectsSimple(words, (word) => rawObjBBox(word)[2], TEXT_VERTICAL_EDGE_CLUSTER_TOLERANCE),
    ...clusterObjectsSimple(words, center, TEXT_VERTICAL_EDGE_CLUSTER_TOLERANCE)
  ].sort((a, b) => b.length - a.length).filter((cluster) => cluster.length >= threshold);
  const condensed: BBox[] = [];
  for (const cluster of clusters) {
    const bbox = objectsToRawBBox(cluster);
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

function rawObjBBox(obj: PDFObject): BBox {
  const raw = (obj as unknown as { __pdfplumber_raw_bbox?: unknown }).__pdfplumber_raw_bbox;
  return Array.isArray(raw) && raw.length === 4 && raw.every((value) => typeof value === "number" && Number.isFinite(value))
    ? (raw as unknown as BBox)
    : objToBBox(obj);
}

function objectsToRawBBox(objects: PDFObject[]): BBox {
  const boxes = objects.map(rawObjBBox);
  return [
    Math.min(...boxes.map((box) => box[0])),
    Math.min(...boxes.map((box) => box[1])),
    Math.max(...boxes.map((box) => box[2])),
    Math.max(...boxes.map((box) => box[3]))
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

export class TableFinder {
  readonly tables: Table[];

  constructor(page: TablePage, options: Record<string, unknown> = {}) {
    const settings = resolveTableSettings(options);
    const edges = this.getEdges(page, settings);
    const intersections = edgesToIntersections(edges, settings.intersection_x_tolerance, settings.intersection_y_tolerance);
    const cells = intersectionsToCells(intersections);
    this.tables = cellsToTables(cells).map((group) => new Table(page, group));
  }

  getEdges(page: TablePage, settings: TableSettings): PDFObject[] {
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
