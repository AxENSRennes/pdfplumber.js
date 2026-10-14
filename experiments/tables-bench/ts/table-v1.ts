// Port of pdfplumber table.py (TableFinder with "lines"/"lines_strict" strategies)
// and the utils it relies on (clustering.py, geometry.py), output-identical to Python.
//
// Same algorithm and iteration orders as upstream; data structures are chosen for
// V8 (edge classes, numeric maps, point indices instead of tuple sets). Where
// upstream is quadratic only because of list scans (find_smallest_cell's
// `rest` filters), the same candidates are enumerated in the same order through
// indexes.

export type Orientation = "v" | "h" | null;

/** Bits of Edge.ints: which coordinates were Python ints (only x0/top feed sum()). */
export const INT_X0 = 1;
export const INT_TOP = 2;

export class Edge {
  constructor(
    public x0: number,
    public top: number,
    public x1: number,
    public bottom: number,
    public width: number,
    public height: number,
    public orientation: Orientation,
    public object_type: string,
    public ints = 0
  ) {}
}

export interface TableSettings {
  snap_x_tolerance: number;
  snap_y_tolerance: number;
  join_x_tolerance: number;
  join_y_tolerance: number;
  intersection_x_tolerance: number;
  intersection_y_tolerance: number;
  edge_min_length: number;
  edge_min_length_prefilter: number;
  vertical_strategy?: "lines" | "lines_strict";
  horizontal_strategy?: "lines" | "lines_strict";
}

export type BBox = [number, number, number, number];

export interface Table {
  cells: BBox[];
  bbox: BBox;
  rows: Array<Array<BBox | null>>;
}

export interface Intersection {
  x: number;
  y: number;
  v: Edge[];
  h: Edge[];
}

export interface TableFinderResult {
  edges: Edge[];
  intersections: Intersection[]; // sorted by (x, y), like sorted(intersections)
  cells: BBox[];
  tables: Table[];
}

// ---------------------------------------------------------------------------
// Python semantics helpers

/**
 * Python 3.12 sum(values) with start=0: exact int accumulation, then
 * Neumaier-compensated float accumulation where later ints are added plainly.
 */
export function pySum(values: number[], isInt: boolean[]): number {
  const n = values.length;
  let i = 0;
  let iResult = 0;
  for (; i < n && isInt[i]; i += 1) iResult += values[i];
  if (i === n) return iResult;
  let f = iResult + values[i];
  let c = 0;
  for (i += 1; i < n; i += 1) {
    const x = values[i];
    if (isInt[i]) {
      f += x;
      continue;
    }
    const t = f + x;
    if (Math.abs(f) >= Math.abs(x)) c += f - t + x;
    else c += x - t + f;
    f = t;
  }
  if (c !== 0 && Number.isFinite(c)) f += c;
  return f;
}

/** Python min()/max() keep the first of equal items (matters for -0.0). */
function pyMin(a: number, b: number): number {
  return b < a ? b : a;
}
function pyMax(a: number, b: number): number {
  return b > a ? b : a;
}

// ---------------------------------------------------------------------------
// utils.clustering / utils.geometry

/** cluster_objects(xs, key, tolerance): groups in cluster order, input order inside. */
function clusterObjects(xs: Edge[], key: (e: Edge) => number, tolerance: number): Edge[][] {
  const values = xs.map(key);
  // make_cluster_dict(cluster_list(list(set(values)), tolerance))
  const uniq = [...new Set(values)].sort((a, b) => a - b);
  const clusterOf = new Map<number, number>();
  let cluster = 0;
  if (tolerance === 0 || uniq.length < 2) {
    uniq.forEach((v, i) => clusterOf.set(v, i));
  } else {
    let last = uniq[0];
    clusterOf.set(last, 0);
    for (let i = 1; i < uniq.length; i += 1) {
      const x = uniq[i];
      if (!(x <= last + tolerance)) cluster += 1;
      clusterOf.set(x, cluster);
      last = x;
    }
  }
  const groups: Edge[][] = [];
  const nClusters = tolerance === 0 || uniq.length < 2 ? uniq.length : cluster + 1;
  for (let k = 0; k < nClusters; k += 1) groups.push([]);
  for (let i = 0; i < xs.length; i += 1) groups[clusterOf.get(values[i])!].push(xs[i]);
  return groups.filter((g) => g.length > 0);
}

/** snap_objects(objs, "x0" | "top", tolerance) for edges. */
function snapObjects(objs: Edge[], attr: "x0" | "top", tolerance: number): Edge[] {
  const intBit = attr === "x0" ? INT_X0 : INT_TOP;
  const out: Edge[] = [];
  for (const group of clusterObjects(objs, (e) => e[attr], tolerance)) {
    const vals = group.map((e) => e[attr]);
    const ints = group.map((e) => (e.ints & intBit) !== 0);
    const avg = pySum(vals, ints) / group.length;
    for (const e of group) {
      const value = avg - e[attr];
      // move_object: the moved coordinates become floats
      if (attr === "x0") out.push(new Edge(e.x0 + value, e.top, e.x1 + value, e.bottom, e.width, e.height, e.orientation, e.object_type, e.ints & ~INT_X0));
      else out.push(new Edge(e.x0, e.top + value, e.x1, e.bottom + value, e.width, e.height, e.orientation, e.object_type, e.ints & ~INT_TOP));
    }
  }
  return out;
}

function filterEdges(edges: Edge[], orientation: Orientation | undefined, edgeType: string | undefined, minLength: number): Edge[] {
  return edges.filter((e) => {
    if (edgeType !== undefined && e.object_type !== edgeType) return false;
    if (orientation !== undefined && e.orientation !== orientation) return false;
    return (e.orientation === "v" ? e.height : e.width) >= minLength;
  });
}

// ---------------------------------------------------------------------------
// table.py

function snapEdges(edges: Edge[], xTol: number, yTol: number): Edge[] {
  const v: Edge[] = [];
  const h: Edge[] = [];
  for (const e of edges) (e.orientation === "v" ? v : h).push(e);
  return snapObjects(v, "x0", xTol).concat(snapObjects(h, "top", yTol));
}

function joinEdgeGroup(edges: Edge[], orientation: "h" | "v", tolerance: number): Edge[] {
  const sorted = orientation === "h" ? [...edges].sort((a, b) => a.x0 - b.x0) : [...edges].sort((a, b) => a.top - b.top);
  const joined = [sorted[0]];
  for (let i = 1; i < sorted.length; i += 1) {
    const e = sorted[i];
    const last = joined[joined.length - 1];
    if (orientation === "h") {
      if (e.x0 <= last.x1 + tolerance) {
        // resize_object(last, "x1", e.x1)
        if (e.x1 > last.x1) joined[joined.length - 1] = new Edge(last.x0, last.top, e.x1, last.bottom, e.x1 - last.x0, last.height, last.orientation, last.object_type, last.ints);
      } else joined.push(e);
    } else if (e.top <= last.bottom + tolerance) {
      // resize_object(last, "bottom", e.bottom)
      if (e.bottom > last.bottom) {
        const diff = e.bottom - last.bottom;
        joined[joined.length - 1] = new Edge(last.x0, last.top, last.x1, e.bottom, last.width, last.height + diff, last.orientation, last.object_type, last.ints);
      }
    } else joined.push(e);
  }
  return joined;
}

function mergeEdges(edges: Edge[], s: TableSettings): Edge[] {
  if (s.snap_x_tolerance > 0 || s.snap_y_tolerance > 0) edges = snapEdges(edges, s.snap_x_tolerance, s.snap_y_tolerance);
  // sorted(edges, key=get_group) then groupby: ("h", top) groups before ("v", x0) groups
  const key = (e: Edge) => (e.orientation === "h" ? e.top : e.x0);
  const sorted = [...edges].sort((a, b) => {
    if (a.orientation !== b.orientation) return a.orientation === "h" ? -1 : 1;
    return key(a) - key(b);
  });
  const out: Edge[] = [];
  let start = 0;
  for (let i = 1; i <= sorted.length; i += 1) {
    if (i < sorted.length && sorted[i].orientation === sorted[start].orientation && key(sorted[i]) === key(sorted[start])) continue;
    const o = sorted[start].orientation as "h" | "v";
    for (const e of joinEdgeGroup(sorted.slice(start, i), o, o === "h" ? s.join_x_tolerance : s.join_y_tolerance)) out.push(e);
    start = i;
  }
  return out;
}

function getEdges(pageEdges: Edge[], s: TableSettings): Edge[] {
  const vType = s.vertical_strategy === "lines_strict" ? "line" : undefined;
  const hType = s.horizontal_strategy === "lines_strict" ? "line" : undefined;
  const v = filterEdges(pageEdges, "v", vType, s.edge_min_length_prefilter);
  const h = filterEdges(pageEdges, "h", hType, s.edge_min_length_prefilter);
  const merged = mergeEdges(v.concat(h), s);
  return filterEdges(merged, undefined, undefined, s.edge_min_length);
}

function edgesToIntersections(edges: Edge[], xTol: number, yTol: number): Intersection[] {
  const v = edges.filter((e) => e.orientation === "v").sort((a, b) => a.x0 - b.x0 || a.top - b.top);
  const h = edges.filter((e) => e.orientation === "h").sort((a, b) => a.top - b.top || a.x0 - b.x0);
  const byX = new Map<number, Map<number, Intersection>>();
  const all: Intersection[] = [];
  for (const ve of v) {
    for (const he of h) {
      if (ve.top <= he.top + yTol && ve.bottom >= he.top - yTol && ve.x0 >= he.x0 - xTol && ve.x0 <= he.x1 + xTol) {
        let col = byX.get(ve.x0);
        if (!col) byX.set(ve.x0, (col = new Map()));
        let point = col.get(he.top);
        if (!point) {
          point = { x: ve.x0, y: he.top, v: [], h: [] };
          col.set(he.top, point);
          all.push(point);
        }
        point.v.push(ve);
        point.h.push(he);
      }
    }
  }
  return all.sort((a, b) => a.x - b.x || a.y - b.y);
}

interface CellWithCorners {
  bbox: BBox;
  corners: [number, number, number, number]; // point indices
}

function intersectionsToCells(points: Intersection[]): CellWithCorners[] {
  const n = points.length;
  // edges_to_set(...) compares edges by bbox value: intern bboxes to ids
  const bboxIds = new Map<string, number>();
  const bboxId = (e: Edge) => {
    const k = `${e.x0}|${e.top}|${e.x1}|${e.bottom}`;
    let id = bboxIds.get(k);
    if (id === undefined) bboxIds.set(k, (id = bboxIds.size));
    return id;
  };
  const vIds = points.map((p) => p.v.map(bboxId));
  const hIds = points.map((p) => p.h.map(bboxId));
  const share = (a: number[], b: number[]) => {
    for (const x of a) for (const y of b) if (x === y) return true;
    return false;
  };
  const connects = (i: number, j: number) => (points[i].x === points[j].x && share(vIds[i], vIds[j])) || (points[i].y === points[j].y && share(hIds[i], hIds[j]));

  const index = new Map<number, Map<number, number>>();
  const byY = new Map<number, number[]>(); // point indices with that y, ascending (= ascending x)
  points.forEach((p, i) => {
    let col = index.get(p.x);
    if (!col) index.set(p.x, (col = new Map()));
    col.set(p.y, i);
    let row = byY.get(p.y);
    if (!row) byY.set(p.y, (row = []));
    row.push(i);
  });

  const cells: CellWithCorners[] = [];
  for (let i = 0; i < n - 1; i += 1) {
    const pt = points[i];
    const row = byY.get(pt.y)!;
    const rowStart = row.indexOf(i) + 1;
    search: for (let b = i + 1; b < n && points[b].x === pt.x; b += 1) {
      if (!connects(i, b)) continue;
      for (let r = rowStart; r < row.length; r += 1) {
        const ri = row[r];
        if (!connects(i, ri)) continue;
        const br = index.get(points[ri].x)?.get(points[b].y);
        if (br !== undefined && connects(br, ri) && connects(br, b)) {
          cells.push({ bbox: [pt.x, pt.y, points[ri].x, points[b].y], corners: [i, b, ri, br] });
          break search;
        }
      }
    }
  }
  return cells;
}

function cellsToTables(cells: CellWithCorners[], nPoints: number): CellWithCorners[][] {
  const mark = new Int32Array(nPoints); // generation-stamped corner set
  let gen = 1;
  let remaining = cells;
  let current: CellWithCorners[] = [];
  const tables: CellWithCorners[][] = [];
  while (remaining.length) {
    const initial = current.length;
    const next: CellWithCorners[] = [];
    for (const cell of remaining) {
      const c = cell.corners;
      if (current.length === 0 || mark[c[0]] === gen || mark[c[1]] === gen || mark[c[2]] === gen || mark[c[3]] === gen) {
        mark[c[0]] = mark[c[1]] = mark[c[2]] = mark[c[3]] = gen;
        current.push(cell);
      } else next.push(cell);
    }
    remaining = next;
    if (current.length === initial) {
      tables.push(current);
      current = [];
      gen += 1;
    }
  }
  if (current.length) tables.push(current);
  const keyed = tables.map((t) => {
    let top = t[0].bbox[1];
    let x0 = t[0].bbox[0];
    for (const c of t) if (c.bbox[1] < top || (c.bbox[1] === top && c.bbox[0] < x0)) [top, x0] = [c.bbox[1], c.bbox[0]];
    return { t, top, x0 };
  });
  keyed.sort((a, b) => a.top - b.top || a.x0 - b.x0);
  return keyed.filter((k) => k.t.length > 1).map((k) => k.t);
}

function tableRows(cells: BBox[]): Array<Array<BBox | null>> {
  const sorted = [...cells].sort((a, b) => a[1] - b[1] || a[0] - b[0]);
  const xs = [...new Set(cells.map((c) => c[0]))].sort((a, b) => a - b);
  const rows: Array<Array<BBox | null>> = [];
  let start = 0;
  for (let i = 1; i <= sorted.length; i += 1) {
    if (i < sorted.length && sorted[i][1] === sorted[start][1]) continue;
    const xdict = new Map<number, BBox>();
    for (let k = start; k < i; k += 1) xdict.set(sorted[k][0], sorted[k]);
    rows.push(xs.map((x) => xdict.get(x) ?? null));
    start = i;
  }
  return rows;
}

/** TableFinder(page, settings) followed by `rows` on every table. */
export function findTables(pageEdges: Edge[], settings: TableSettings): TableFinderResult {
  const edges = getEdges(pageEdges, settings);
  const intersections = edgesToIntersections(edges, settings.intersection_x_tolerance, settings.intersection_y_tolerance);
  const cells = intersectionsToCells(intersections);
  const tables = cellsToTables(cells, intersections.length).map((group) => {
    const tc = group.map((c) => c.bbox);
    let [x0, top, x1, bottom] = tc[0];
    for (const c of tc) {
      x0 = pyMin(x0, c[0]);
      top = pyMin(top, c[1]);
      x1 = pyMax(x1, c[2]);
      bottom = pyMax(bottom, c[3]);
    }
    return { cells: tc, bbox: [x0, top, x1, bottom] as BBox, rows: tableRows(tc) };
  });
  return { edges, intersections, cells: cells.map((c) => c.bbox), tables };
}
