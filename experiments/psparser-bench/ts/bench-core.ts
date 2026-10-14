// Environment-agnostic benchmark: same code runs in Node and in browsers.
import { readCorpus } from "./canon.ts";
import { PDFContentParser, PSEOF, PyError, type PSValue } from "./psparser.ts";
import * as v1 from "./psparser-v1.ts";
import { loadWasmParser } from "./wasm.ts";

export interface BenchResult {
  env: string;
  mb: number;
  objects: number;
  wasmInstantiateMs: number;
  modes: Record<string, { medianMs: number; runsMs: number[]; mbPerS: number; mObjPerS: number }>;
}

/**
 * Identical consumer for every mode: like the interpreter's operand stack, each
 * object is stored into a JS array, so every mode pays for real JS values.
 */
const sinkArgs: unknown[] = new Array(64);
let sinkN = 0;
function sink(obj: unknown): void {
  sinkArgs[sinkN & 63] = obj;
  sinkN += 1;
}

function tsCount(streams: Uint8Array[]): number {
  let parser: PDFContentParser;
  try {
    parser = new PDFContentParser(streams);
  } catch (e) {
    if (e instanceof PSEOF) return 0;
    throw e;
  }
  let n = 0;
  for (;;) {
    try {
      sink(parser.nextobject());
      n += 1;
    } catch (e) {
      if (e instanceof PSEOF || e instanceof PyError) return n;
      throw e;
    }
  }
}

function tsV1Count(streams: Uint8Array[]): number {
  let parser: v1.PDFContentParser;
  try {
    parser = new v1.PDFContentParser(streams);
  } catch (e) {
    if (e instanceof v1.PSEOF) return 0;
    throw e;
  }
  let n = 0;
  for (;;) {
    try {
      sink(parser.nextobject());
      n += 1;
    } catch (e) {
      if (e instanceof v1.PSEOF || e instanceof v1.PyError) return n;
      throw e;
    }
  }
}

export async function runBench(env: string, corpusBytes: Uint8Array, wasmBytes: ArrayBuffer, runs = 5, now: () => number = () => performance.now()): Promise<BenchResult> {
  const jobs = readCorpus(corpusBytes);
  const bytes = jobs.reduce((a, j) => a + j.reduce((b, s) => b + s.length, 0), 0);

  const t0 = now();
  const wasm = await loadWasmParser(wasmBytes);
  const wasmInstantiateMs = now() - t0;

  const modes: Record<string, (s: Uint8Array[]) => number> = {
    "ts v1 (premier jet)": tsV1Count,
    "ts v2 (optimise)": tsCount,
    "wasm + objets JS": (s) => wasm.parseJobDecodedCount(s, sink),
    "wasm seul (rien ne revient au JS)": wasm.parseJobCountOnly
  };

  // Sanity: every mode sees the same object count.
  const counts = Object.entries(modes).map(([name, fn]) => [name, jobs.reduce((a, j) => a + fn(j), 0)] as const);
  if (new Set(counts.map(([, c]) => c)).size !== 1) throw new Error("object counts differ: " + JSON.stringify(counts));

  const runsMs: Record<string, number[]> = Object.fromEntries(Object.keys(modes).map((k) => [k, []]));
  const names = Object.keys(modes);
  for (let r = 0; r < runs; r += 1) {
    // rotate the order each run so no mode always runs right after another
    for (let k = 0; k < names.length; k += 1) {
      const name = names[(k + r) % names.length];
      const fn = modes[name];
      const start = now();
      let total = 0;
      for (const job of jobs) total += fn(job);
      runsMs[name].push(now() - start);
      if (total !== counts[0][1]) throw new Error("count drift");
    }
  }

  const result: BenchResult = { env, mb: bytes / 1e6, objects: counts[0][1], wasmInstantiateMs, modes: {} };
  for (const name of names) {
    const sorted = [...runsMs[name]].sort((a, b) => a - b);
    const medianMs = sorted[Math.floor(sorted.length / 2)];
    result.modes[name] = {
      medianMs,
      runsMs: runsMs[name].map((x) => Math.round(x)),
      mbPerS: bytes / 1e6 / (medianMs / 1000),
      mObjPerS: counts[0][1] / 1e6 / (medianMs / 1000)
    };
  }
  return result;
}
