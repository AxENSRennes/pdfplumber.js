/** Browsers (and workers): everything through `fetch`. */

import type { PDFInput, RuntimeOptions } from "../types.js";
import { fetchBytes, fetchCMapSource, loaderCMapSource, memoryInput, type Platform } from "./platform.js";
import { WorkerCore } from "../worker/client.js";

/** The base URL relative inputs are resolved against. */
function baseUrl(): string {
  const g = globalThis as { document?: { baseURI: string }; location?: { href: string } };
  return g.document?.baseURI ?? g.location?.href ?? import.meta.url;
}

const toUrl = (src: string | URL): URL => (src instanceof URL ? src : new URL(src, baseUrl()));

export const browserPlatform: Platform = {
  name: "browser",

  async compileWasm(source: RuntimeOptions["wasm"]) {
    if (source instanceof WebAssembly.Module) return source;
    if (source instanceof Uint8Array || source instanceof ArrayBuffer)
      return WebAssembly.compile(source as Uint8Array<ArrayBuffer>);
    const url = source === undefined ? new URL("../pdfplumber.wasm", import.meta.url) : toUrl(source);
    const response = await fetch(url);
    if (!response.ok) throw new Error(`failed to fetch ${url.href}: HTTP ${response.status}`);
    if (typeof WebAssembly.compileStreaming === "function" && /wasm/.test(response.headers.get("content-type") ?? ""))
      return WebAssembly.compileStreaming(response);
    return WebAssembly.compile(await response.arrayBuffer());
  },

  cmapSource(option: RuntimeOptions["cmaps"]) {
    if (option === false) return null;
    if (option?.load) return loaderCMapSource(option.load);
    const base = option?.baseUrl === undefined ? new URL("../cmaps/", import.meta.url) : toUrl(option.baseUrl);
    return fetchCMapSource(base.href.endsWith("/") ? base : new URL(`${base.href}/`));
  },

  async readInput(input: PDFInput) {
    const bytes = await memoryInput(input);
    if (bytes) return bytes;
    if (typeof input === "string" || input instanceof URL) return fetchBytes(toUrl(input));
    throw new TypeError("unsupported input: expected bytes, a Blob or a URL");
  },

  async workerCore(module: WebAssembly.Module, options: RuntimeOptions) {
    const url = options.worker === true || options.worker === undefined ? new URL("./worker.js", import.meta.url) : toUrl(options.worker as string | URL);
    if (options.cmaps && options.cmaps.load) throw new Error("worker mode cannot use a CMap loader function: give a baseUrl");
    const cmapBase =
      options.cmaps === false
        ? null
        : options.cmaps?.baseUrl !== undefined
          ? toUrl(options.cmaps.baseUrl).href
          : new URL("../cmaps/", import.meta.url).href;
    return WorkerCore.start(new Worker(url, { type: "module" }), module, cmapBase);
  },
};
