// Environment-agnostic table-finding benchmark (Node and browsers).
import { readCorpus, type Job } from "./canon.ts";
import { findTables, type Table } from "./table.ts";
import { findTables as findTablesV1 } from "./table-v1.ts";
import { loadWasmTables } from "./wasm.ts";

export interface ModeResult {
  medianMs: number;
  runsMs: number[];
}
export interface BenchResult {
  env: string;
  wasmInstantiateMs: number;
  corpora: Record<string, { jobs: number; edges: number; tables: number; modes: Record<string, ModeResult> }>;
}

let sinkTables: Table[] = [];

export async function runBench(env: string, corpora: Record<string, Uint8Array>, wasmBytes: ArrayBuffer, runs = 5, now = () => performance.now()): Promise<BenchResult> {
  const t0 = now();
  const wasm = await loadWasmTables(wasmBytes);
  const result: BenchResult = { env, wasmInstantiateMs: now() - t0, corpora: {} };

  for (const [name, bytes] of Object.entries(corpora)) {
    const jobs: Job[] = readCorpus(bytes);
    const modes: Record<string, (j: Job) => Table[]> = {
      "ts v1 (premier jet)": (j) => findTablesV1(j.edges, j.settings).tables,
      ts: (j) => findTables(j.edges, j.settings).tables,
      "wasm hybride (objets JS -> wasm -> objets JS)": (j) => wasm.tablesFromObjects(j.edges, j.settings),
      "wasm coeur (bords deja en memoire wasm)": (j) => wasm.tablesFromRaw(j.raw)
    };
    // Sanity: every mode returns the same tables.
    let nTables = 0;
    for (const job of jobs) {
      const ref = JSON.stringify(modes.ts(job));
      for (const fn of Object.values(modes)) if (JSON.stringify(fn(job)) !== ref) throw new Error(`${name}: modes disagree`);
      nTables += modes.ts(job).length;
    }
    const names = Object.keys(modes);
    const runsMs: Record<string, number[]> = Object.fromEntries(names.map((k) => [k, []]));
    for (let r = 0; r < runs; r += 1) {
      for (let k = 0; k < names.length; k += 1) {
        const mode = names[(k + r) % names.length];
        const fn = modes[mode];
        const start = now();
        for (const job of jobs) sinkTables = fn(job);
        runsMs[mode].push(now() - start);
      }
    }
    const modeResults: Record<string, ModeResult> = {};
    for (const mode of names) {
      const sorted = [...runsMs[mode]].sort((a, b) => a - b);
      modeResults[mode] = { medianMs: sorted[Math.floor(sorted.length / 2)], runsMs: runsMs[mode].map((x) => Math.round(x)) };
    }
    result.corpora[name] = { jobs: jobs.length, edges: jobs.reduce((a, j) => a + j.edges.length, 0), tables: nTables, modes: modeResults };
  }
  void sinkTables;
  return result;
}
