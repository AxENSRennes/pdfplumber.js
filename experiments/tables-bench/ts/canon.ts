// Corpus reader and the trace serialization shared with dump_tables.py.
import { Edge, INT_TOP, INT_X0, type Orientation, type TableFinderResult, type TableSettings } from "./table.ts";

export interface Job {
  settings: TableSettings;
  edges: Edge[];
  /** The job's bytes in corpus layout (what the wasm side reads). */
  raw: Uint8Array;
}

const ORIENT: Orientation[] = [null, "v", "h"];
const TYPES = ["line", "rect_edge", "curve_edge"];
export const EDGE_BYTES = 51;

export function readCorpus(data: Uint8Array): Job[] {
  const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
  if (String.fromCharCode(...data.subarray(0, 4)) !== "TBC1") throw new Error("bad corpus");
  const n = view.getUint32(4, true);
  let off = 8;
  const jobs: Job[] = [];
  for (let j = 0; j < n; j += 1) {
    const start = off;
    const s: number[] = [];
    for (let k = 0; k < 8; k += 1) s.push(view.getFloat64(off + 8 * k, true));
    off += 64;
    const ne = view.getUint32(off, true);
    off += 4;
    const edges: Edge[] = [];
    for (let e = 0; e < ne; e += 1) {
      const f = (k: number) => view.getFloat64(off + 8 * k, true);
      const flags = data[off + 48];
      edges.push(new Edge(f(0), f(1), f(2), f(3), f(4), f(5), ORIENT[data[off + 49]], TYPES[data[off + 50]], (flags & 1 ? INT_X0 : 0) | (flags & 2 ? INT_TOP : 0)));
      off += EDGE_BYTES;
    }
    const settings: TableSettings = {
      snap_x_tolerance: s[0],
      snap_y_tolerance: s[1],
      join_x_tolerance: s[2],
      join_y_tolerance: s[3],
      intersection_x_tolerance: s[4],
      intersection_y_tolerance: s[5],
      edge_min_length: s[6],
      edge_min_length_prefilter: s[7]
    };
    jobs.push({ settings, edges, raw: data.subarray(start, off) });
  }
  return jobs;
}

const f64 = new DataView(new ArrayBuffer(8));
const HEX = Array.from({ length: 256 }, (_, i) => i.toString(16).padStart(2, "0"));

export function num(v: number): string {
  if (Number.isSafeInteger(v)) return "d" + String(v === 0 ? 0 : v);
  f64.setFloat64(0, v);
  let s = "f";
  for (let i = 0; i < 8; i += 1) s += HEX[f64.getUint8(i)];
  return s;
}

const bbox = (b: ReadonlyArray<number | null>) => b.map((v) => (v === null ? "-" : num(v))).join(",");

/** Same text as dump_tables.serialize(), followed by the outcome line. */
export function serialize(r: TableFinderResult): string {
  const out: string[] = [];
  for (const e of r.edges) out.push(`E ${bbox([e.x0, e.top, e.x1, e.bottom, e.width, e.height])} ${e.orientation === null ? "None" : e.orientation}`);
  for (const p of r.intersections) out.push(`I ${num(p.x)},${num(p.y)} ${p.v.length} ${p.h.length}`);
  for (const c of r.cells) out.push(`C ${bbox(c)}`);
  for (const t of r.tables) {
    out.push(`T ${bbox(t.bbox)} ${t.cells.length}`);
    for (const c of t.cells) out.push(`  c ${bbox(c)}`);
    for (const row of t.rows) out.push("  r " + row.map((c) => (c === null ? "-" : bbox(c))).join(" "));
  }
  return out.join("\n") + "\n!ok\n";
}
