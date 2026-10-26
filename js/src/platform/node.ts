/** Node: files through `node:fs`, HTTP(S) through `fetch`. */

import { readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";

import type { CMapSource } from "../core/runtime.js";
import { CMAP_NAMES } from "../generated/cmap-names.js";
import type { PDFInput, RuntimeOptions } from "../types.js";
import { cmapFileName, fetchBytes, fetchCMapSource, loaderCMapSource, memoryInput, type Platform } from "./platform.js";

/** `dist/` from `dist/src/platform/node.js` (the package), or from
 * `js/src/platform/node.ts` (the repository, sources run by the tests). */
function distDir(): URL {
  const packaged = new URL("../../", import.meta.url);
  if (existsSync(fileURLToPath(new URL("pdfplumber.wasm", packaged)))) return packaged;
  return new URL("../../../dist/", import.meta.url);
}

async function readUrlOrPath(src: string | URL): Promise<Uint8Array> {
  const url = src instanceof URL ? src : /^[a-z][a-z0-9+.-]*:\/\//i.test(src) ? new URL(src) : pathToFileURL(src);
  if (url.protocol === "file:") return new Uint8Array(await readFile(fileURLToPath(url)));
  if (url.protocol === "http:" || url.protocol === "https:") return fetchBytes(url);
  throw new Error(`unsupported URL scheme: ${url.protocol}`);
}

/** A CMap source reading files from a directory. */
function dirCMapSource(dir: URL): CMapSource {
  return {
    names: CMAP_NAMES,
    async load(name: string) {
      return new Uint8Array(await readFile(fileURLToPath(new URL(cmapFileName(name), dir))));
    },
  };
}

export const nodePlatform: Platform = {
  name: "node",

  async compileWasm(source: RuntimeOptions["wasm"]) {
    if (source instanceof WebAssembly.Module) return source;
    let bytes: Uint8Array;
    if (source instanceof Uint8Array) bytes = source;
    else if (source instanceof ArrayBuffer) bytes = new Uint8Array(source);
    else bytes = await readUrlOrPath(source ?? new URL("pdfplumber.wasm", distDir()));
    return WebAssembly.compile(bytes as Uint8Array<ArrayBuffer>);
  },

  cmapSource(option: RuntimeOptions["cmaps"]) {
    if (option === false) return null;
    if (option?.load) return loaderCMapSource(option.load);
    const base = option?.baseUrl;
    if (base === undefined) return dirCMapSource(new URL("cmaps/", distDir()));
    const url =
      base instanceof URL ? base : /^[a-z][a-z0-9+.-]*:\/\//i.test(base) ? new URL(base) : pathToFileURL(base);
    const dir = url.href.endsWith("/") ? url : new URL(`${url.href}/`);
    return dir.protocol === "file:" ? dirCMapSource(dir) : fetchCMapSource(dir);
  },

  filePath(input: PDFInput) {
    if (input instanceof URL) return input.protocol === "file:" ? fileURLToPath(input) : undefined;
    if (typeof input !== "string") return undefined;
    if (!/^[a-z][a-z0-9+.-]*:\/\//i.test(input)) return input;
    return input.toLowerCase().startsWith("file:") ? fileURLToPath(input) : undefined;
  },

  async readInput(input: PDFInput) {
    const bytes = await memoryInput(input);
    if (bytes) return bytes;
    if (typeof input === "string" || input instanceof URL) return readUrlOrPath(input);
    throw new TypeError("unsupported input: expected a path, a URL, bytes or a Blob");
  },
};
