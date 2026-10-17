/**
 * What differs between Node and browsers: loading the WebAssembly module,
 * reading CMap files, adapting inputs, workers.
 */

import type { CMapSource, Core } from "../core/runtime.js";
import { CMAP_NAMES } from "../generated/cmap-names.js";
import type { PDFInput, RuntimeOptions } from "../types.js";

export interface Platform {
  readonly name: "node" | "browser";
  /** Compiles the module given by the options, or the default one. */
  compileWasm(source: RuntimeOptions["wasm"]): Promise<WebAssembly.Module>;
  /** The CMap source of the options. */
  cmapSource(option: RuntimeOptions["cmaps"]): CMapSource | null;
  /** The bytes of an input. */
  readInput(input: PDFInput): Promise<Uint8Array>;
  /** A core in a worker, when the platform has workers. */
  workerCore?(module: WebAssembly.Module, options: RuntimeOptions): Promise<Core>;
}

let current: Platform | undefined;

export function setPlatform(p: Platform): void {
  current = p;
}

export function platform(): Platform {
  if (!current) throw new Error("pdfplumber-js: no platform (import the package entry point, not its modules)");
  return current;
}

/** The file name of a CMap under the CMap directory. */
export function cmapFileName(name: string): string {
  return `${encodeURIComponent(name)}.pdfcmap.zlib`;
}

/** A CMap source reading `<base><name>.pdfcmap.zlib` with `fetch`. */
export function fetchCMapSource(base: URL): CMapSource {
  return {
    names: CMAP_NAMES,
    async load(name: string): Promise<Uint8Array> {
      const url = new URL(cmapFileName(name), base);
      const r = await fetch(url);
      if (!r.ok) throw new Error(`failed to fetch CMap ${name} from ${url.href}: HTTP ${r.status}`);
      return new Uint8Array(await r.arrayBuffer());
    },
  };
}

/** A CMap source from a loader function. */
export function loaderCMapSource(load: (name: string) => Promise<Uint8Array>): CMapSource {
  return { names: CMAP_NAMES, load };
}

/** The bytes of an in-memory input, or `undefined`. */
export async function memoryInput(input: PDFInput): Promise<Uint8Array | undefined> {
  if (input instanceof Uint8Array) return input;
  if (input instanceof ArrayBuffer) return new Uint8Array(input);
  if (ArrayBuffer.isView(input)) return new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
  if (typeof Blob !== "undefined" && input instanceof Blob) return new Uint8Array(await input.arrayBuffer());
  return undefined;
}

/** The body of an HTTP(S) URL. */
export async function fetchBytes(url: URL): Promise<Uint8Array> {
  let r: Response;
  try {
    r = await fetch(url);
  } catch (e) {
    throw new Error(`failed to fetch ${url.href}: ${e instanceof Error ? e.message : String(e)}`, { cause: e });
  }
  if (!r.ok) throw new Error(`failed to fetch ${url.href}: HTTP ${r.status}`);
  return new Uint8Array(await r.arrayBuffer());
}
