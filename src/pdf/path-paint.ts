import type { Point } from "../types.js";
import { cleanNumber } from "../utils.js";

export type PdfminerPathOp =
  | ["m", number, number]
  | ["l", number, number]
  | ["c", number, number, number, number, number, number]
  | ["v", number, number, number, number]
  | ["y", number, number, number, number]
  | ["h"];

export interface PaintedPathLikePdfminer {
  type: "line" | "rect" | "curve";
  pts: Point[];
  original_path: Array<[string, ...Point[]]>;
  closepath: boolean;
  dashing_style?: unknown;
}

interface Subpath {
  points: Point[];
  original_path: Array<[string, ...Point[]]>;
  closed: boolean;
  hasCurve: boolean;
  start: Point;
  trailingMove: boolean;
}

function samePoint(a: Point, b: Point): boolean {
  return cleanNumber(a[0]) === cleanNumber(b[0]) && cleanNumber(a[1]) === cleanNumber(b[1]);
}

function uniquePoints(points: Point[]): Point[] {
  const out: Point[] = [];
  for (const point of points) {
    if (!out.some((candidate) => samePoint(candidate, point))) out.push(point);
  }
  return out;
}

function isAxisAlignedRect(points: Point[], closed: boolean): boolean {
  const pts = [...points];
  const geometricallyClosed = closed || (pts.length > 1 && samePoint(pts[0], pts[pts.length - 1]));
  if (!geometricallyClosed) return false;
  while (pts.length > 1 && samePoint(pts[0], pts[pts.length - 1])) pts.pop();
  if (pts.length !== 4) return false;
  const [[x0, y0], [x1, y1], [x2, y2], [x3, y3]] = pts.map((point) => [cleanNumber(point[0]), cleanNumber(point[1])] as Point);
  return (x0 === x1 && y1 === y2 && x2 === x3 && y3 === y0) || (y0 === y1 && x1 === x2 && y2 === y3 && x3 === x0);
}

function parseSubpaths(path: PdfminerPathOp[]): Subpath[] {
  const out: Subpath[] = [];
  let current: Subpath | null = null;
  for (const op of path) {
    if (op[0] === "m") {
      const point: Point = [op[1], op[2]];
      current = { points: [point], original_path: [["m", point]], closed: false, hasCurve: false, start: point, trailingMove: out.length > 0 };
      out.push(current);
    } else if (op[0] === "l") {
      const point: Point = [op[1], op[2]];
      current?.points.push(point);
      current?.original_path.push(["l", point]);
    } else if (op[0] === "c") {
      const p1: Point = [op[1], op[2]];
      const p2: Point = [op[3], op[4]];
      const p3: Point = [op[5], op[6]];
      current?.points.push(p3);
      current?.original_path.push(["c", p1, p2, p3]);
      if (current) current.hasCurve = true;
    } else if (op[0] === "v") {
      const p1: Point = current?.points.at(-1) ?? [Number.NaN, Number.NaN];
      const p2: Point = [op[1], op[2]];
      const p3: Point = [op[3], op[4]];
      current?.points.push(p3);
      current?.original_path.push(["v", p1, p2, p3]);
      if (current) current.hasCurve = true;
    } else if (op[0] === "y") {
      const p1: Point = [op[1], op[2]];
      const p2: Point = [op[3], op[4]];
      current?.points.push(p2);
      current?.original_path.push(["y", p1, p2, p2]);
      if (current) current.hasCurve = true;
    } else if (op[0] === "h" && current) {
      current.closed = true;
      current.original_path.push(["h"]);
      if (!samePoint(current.points[current.points.length - 1], current.start)) current.points.push(current.start);
    }
  }
  return out;
}

export function paintPathLikePdfminer(path: PdfminerPathOp[], options: { dashing_style?: unknown } = {}): PaintedPathLikePdfminer[] {
  const subpaths = parseSubpaths(path);
  return subpaths
    .filter((subpath) => subpath.points.length >= 2 || subpaths.length === 1)
    .map((subpath): PaintedPathLikePdfminer => {
      const unique = uniquePoints(subpath.points);
      const type: PaintedPathLikePdfminer["type"] = subpath.hasCurve ? "curve" : unique.length === 2 ? "line" : isAxisAlignedRect(subpath.points, subpath.closed) ? "rect" : "curve";
      const base = {
        type,
        pts: type === "curve" && subpath.hasCurve ? [subpath.points[0], subpath.points[subpath.points.length - 1]] : subpath.points,
        original_path: subpath.original_path,
        closepath: subpath.closed
      };
      return options.dashing_style === undefined ? base : { ...base, dashing_style: options.dashing_style };
    });
}
