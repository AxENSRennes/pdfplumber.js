// Loader for the Rust/wasm table finder.
import { EDGE_BYTES } from "./canon.ts";
import { INT_TOP, INT_X0, type BBox, type Edge, type Table, type TableSettings } from "./table.ts";

interface Exports {
  memory: WebAssembly.Memory;
  input_buffer(len: number): number;
  out_ptr(): number;
  out_len(): number;
  run_trace(): void;
  run_tables(): number;
}

const ORIENT_CODE = { v: 1, h: 2 } as const;
const TYPE_CODE: Record<string, number> = { line: 0, rect_edge: 1, curve_edge: 2 };

export async function loadWasmTables(wasmBytes: BufferSource) {
  const { instance } = await WebAssembly.instantiate(wasmBytes, {});
  const ex = instance.exports as unknown as Exports;

  /** Copy a job already in corpus layout (as if the Rust parser had produced it). */
  function writeRaw(raw: Uint8Array): void {
    const ptr = ex.input_buffer(raw.length);
    new Uint8Array(ex.memory.buffer, ptr, raw.length).set(raw);
  }

  /** Pack JS edge objects into the job layout (the cost of a JS-parser + wasm-tables hybrid). */
  function writeObjects(edges: Edge[], s: TableSettings): void {
    const size = 68 + EDGE_BYTES * edges.length;
    const ptr = ex.input_buffer(size);
    const view = new DataView(ex.memory.buffer, ptr, size);
    const bytes = new Uint8Array(ex.memory.buffer, ptr, size);
    const settings = [s.snap_x_tolerance, s.snap_y_tolerance, s.join_x_tolerance, s.join_y_tolerance, s.intersection_x_tolerance, s.intersection_y_tolerance, s.edge_min_length, s.edge_min_length_prefilter];
    settings.forEach((v, k) => view.setFloat64(8 * k, v, true));
    view.setUint32(64, edges.length, true);
    let o = 68;
    for (const e of edges) {
      view.setFloat64(o, e.x0, true);
      view.setFloat64(o + 8, e.top, true);
      view.setFloat64(o + 16, e.x1, true);
      view.setFloat64(o + 24, e.bottom, true);
      view.setFloat64(o + 32, e.width, true);
      view.setFloat64(o + 40, e.height, true);
      bytes[o + 48] = (e.ints & INT_X0 ? 1 : 0) | (e.ints & INT_TOP ? 2 : 0);
      bytes[o + 49] = e.orientation === null ? 0 : ORIENT_CODE[e.orientation];
      bytes[o + 50] = TYPE_CODE[e.object_type];
      o += EDGE_BYTES;
    }
  }

  function decodeTables(): Table[] {
    const len = ex.out_len();
    const view = new DataView(ex.memory.buffer, ex.out_ptr(), len);
    let o = 0;
    const u32 = () => {
      const v = view.getUint32(o, true);
      o += 4;
      return v;
    };
    const box = (): BBox => {
      const b: BBox = [view.getFloat64(o, true), view.getFloat64(o + 8, true), view.getFloat64(o + 16, true), view.getFloat64(o + 24, true)];
      o += 32;
      return b;
    };
    const tables: Table[] = [];
    const n = u32();
    for (let t = 0; t < n; t += 1) {
      const nc = u32();
      const cells: BBox[] = [];
      for (let k = 0; k < nc; k += 1) cells.push(box());
      const bbox = box();
      const nr = u32();
      const ncol = u32();
      const rows: Array<Array<BBox | null>> = [];
      for (let r = 0; r < nr; r += 1) {
        const row: Array<BBox | null> = [];
        for (let k = 0; k < ncol; k += 1) {
          const idx = view.getInt32(o, true);
          o += 4;
          row.push(idx < 0 ? null : cells[idx]);
        }
        rows.push(row);
      }
      tables.push({ cells, bbox, rows });
    }
    return tables;
  }

  return {
    trace(raw: Uint8Array): string {
      writeRaw(raw);
      ex.run_trace();
      return new TextDecoder().decode(new Uint8Array(ex.memory.buffer, ex.out_ptr(), ex.out_len()));
    },
    /** Hybrid: JS edge objects in, JS table objects out. */
    tablesFromObjects(edges: Edge[], s: TableSettings): Table[] {
      writeObjects(edges, s);
      ex.run_tables();
      return decodeTables();
    },
    /** Rust core: edges already in Rust's layout, JS table objects out. */
    tablesFromRaw(raw: Uint8Array): Table[] {
      writeRaw(raw);
      ex.run_tables();
      return decodeTables();
    }
  };
}
