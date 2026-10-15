/**
 * The WebAssembly core: instantiation, the ABI of crates/wasm/src/abi.rs,
 * requests and responses (crates/wasm/src/ops.rs), and the lazy loading of
 * predefined CMaps (docs/js-runtime.md).
 */

import { CoreCrash, fromPython, type PythonErrorInfo } from "../errors.js";
import { decodeBinary, decodeJson, encodeValue } from "./wire.js";

/** Where the predefined CMaps come from. */
export interface CMapSource {
  /** The names that exist (pdfminer's `cmap` directory). */
  readonly names: readonly string[];
  /** The file of a CMap: zlib-compressed compact data (or inflated). */
  load(name: string): Promise<Uint8Array>;
}

/** A diagnostic of the core (`logging` call or `warnings.warn`). */
export interface Diagnostic {
  /** Logger name (`pdfminer.pdfdocument`) or warning category. */
  source: string;
  /** Python `logging` level: 10 DEBUG, 20 INFO, 30 WARNING, 40 ERROR. */
  level: number;
  message: string;
}

export interface CallOptions {
  /** A byte buffer handed over with the request (the PDF, a CMap). */
  data?: Uint8Array;
  /** Collect diagnostics at or above this level (0: none). */
  minLevel?: number;
  /** Receives the diagnostics of the call (and the count of the dropped
   * ones), including those of a call re-run after loading CMaps. */
  onDiagnostics?: (entries: Diagnostic[], dropped: number) => void;
}

/** What `open()` and the pages call: a core in this thread or in a worker. */
export interface Core {
  call(op: string, handle: number, args: unknown, options?: CallOptions): Promise<unknown>;
  readonly crashed: boolean;
}

export const STATUS_OK = 0;
export const STATUS_ERROR = 1;
export const STATUS_NEED_CMAPS = 2;

interface Exports {
  memory: WebAssembly.Memory;
  pp_alloc(len: number): number;
  pp_free(ptr: number): void;
  pp_call(request: number, data: number): number;
  pp_result_ptr(): number;
  pp_result_len(): number;
  pp_result_free(): void;
  pp_buffers(): number;
}

/** A decoded response. */
export interface Response {
  status: number;
  value: unknown;
  diagnostics: Diagnostic[];
  dropped: number;
  /** Bytes of the payload (the benchmark). */
  payloadBytes: number;
}

/** The request block of an operation. */
export function encodeRequest(op: string, handle: number, args: unknown, minLevel: number): Uint8Array {
  return encodeValue([op, handle, args ?? null, minLevel]);
}

/** The status of a response. */
export function responseStatus(u8: Uint8Array): number {
  return u8[4];
}

/** Decodes a response (a view of the module's memory, or a copy). */
export function decodeResponse(u8: Uint8Array): Response {
  const dv = new DataView(u8.buffer, u8.byteOffset, u8.byteLength);
  if (u8.length < 24 || dv.getUint32(0, true) !== 0x3152_4a50) throw new Error("malformed response");
  const status = u8[4];
  const format = u8[5];
  const fixup = (u8[6] & 1) !== 0;
  const payloadOff = dv.getUint32(8, true);
  const payloadLen = dv.getUint32(12, true);
  const diagOff = dv.getUint32(16, true);
  const diagLen = dv.getUint32(20, true);
  const payload = u8.subarray(payloadOff, payloadOff + payloadLen);
  const value = format === 1 ? decodeJson(payload, fixup) : decodeBinary(payload);
  const diag = decodeBinary(u8.subarray(diagOff, diagOff + diagLen)) as {
    entries: [string, number, string][];
    dropped: number;
  };
  return {
    status,
    value,
    diagnostics: diag.entries.map(([source, level, message]) => ({ source, level, message })),
    dropped: diag.dropped,
    payloadBytes: payloadLen,
  };
}

/** The value of a final response, or the exception it carries. */
export function responseValue(r: Response, op: string): unknown {
  if (r.status === STATUS_OK) return r.value;
  if (r.status === STATUS_ERROR) throw fromPython(r.value as PythonErrorInfo);
  throw new Error(`${op} needs CMaps: ${(r.value as string[]).join(", ")}`);
}

export class WasmCore implements Core {
  private readonly ex: Exports;
  private dead: unknown = undefined;
  private configured = false;
  private readonly added = new Set<string>();
  private readonly loading = new Map<string, Promise<void>>();

  private constructor(
    instance: WebAssembly.Instance,
    readonly cmaps: CMapSource | null,
  ) {
    this.ex = instance.exports as unknown as Exports;
  }

  /** Instantiates a compiled module (it imports nothing). */
  static async create(module: WebAssembly.Module, cmaps: CMapSource | null): Promise<WasmCore> {
    const instance = await WebAssembly.instantiate(module, {});
    return new WasmCore(instance, cmaps);
  }

  /** Whether the instance crashed (its state is lost). */
  get crashed(): boolean {
    return this.dead !== undefined;
  }

  /** Live buffers of `pp_alloc` (leak checks). */
  buffers(): number {
    return this.ex.pp_buffers();
  }

  private guard<T>(f: () => T): T {
    if (this.dead !== undefined) throw new CoreCrash(this.dead);
    try {
      return f();
    } catch (e) {
      if (e instanceof WebAssembly.RuntimeError) {
        this.dead = e;
        throw new CoreCrash(e);
      }
      throw e;
    }
  }

  private put(bytes: Uint8Array): number {
    const ptr = this.ex.pp_alloc(bytes.length);
    new Uint8Array(this.ex.memory.buffer, ptr, bytes.length).set(bytes);
    return ptr;
  }

  /** Runs an encoded request; returns a view of the response in the
   * module's memory, valid until the next call. */
  run(request: Uint8Array, data?: Uint8Array): Uint8Array {
    return this.guard(() => {
      const reqPtr = this.put(request);
      const dataPtr = data ? this.put(data) : 0;
      this.ex.pp_call(reqPtr, dataPtr);
      return new Uint8Array(this.ex.memory.buffer, this.ex.pp_result_ptr(), this.ex.pp_result_len());
    });
  }

  /** Drops the last response in the module. */
  release(): void {
    this.guard(() => this.ex.pp_result_free());
  }

  /** One request, no CMap loading. */
  callRaw(op: string, handle: number, args: unknown, options: CallOptions = {}): Response {
    const view = this.run(encodeRequest(op, handle, args, options.minLevel ?? 0), options.data);
    try {
      return decodeResponse(view);
    } finally {
      this.release();
    }
  }

  /** Hands the CMap names to the core, once. */
  configure(): void {
    if (this.configured) return;
    this.configured = true;
    const r = this.callRaw("cmap_configure", 0, this.cmaps ? [...this.cmaps.names] : []);
    responseValue(r, "cmap_configure");
  }

  /** Fetches and hands over a CMap (once per name; concurrent callers
   * share the fetch). */
  async addCMap(name: string): Promise<void> {
    if (this.added.has(name)) return;
    let p = this.loading.get(name);
    if (!p) {
      p = (async () => {
        if (!this.cmaps) throw new Error(`no CMap source for ${name}`);
        const data = await this.cmaps.load(name);
        responseValue(this.callRaw("cmap_add", 0, name, { data }), "cmap_add");
        this.added.add(name);
      })();
      this.loading.set(name, p);
      p.catch(() => this.loading.delete(name));
    }
    await p;
  }

  /** Loads the CMaps a void call missed; refuses a name already handed
   * over (the core would loop). */
  async loadMissing(names: string[]): Promise<void> {
    const again = names.filter((n) => this.added.has(n));
    if (again.length) throw new Error(`the core asked again for CMaps it has: ${again.join(", ")}`);
    await Promise.all(names.map((n) => this.addCMap(n)));
  }

  /** Runs a request until it does not miss CMaps; returns its final
   * response, still undecoded beyond its status, as a copy, together with
   * the diagnostics of the void runs. */
  async runToEnd(request: Uint8Array, data?: Uint8Array): Promise<{ response: Uint8Array; earlier: Diagnostic[][] }> {
    this.configure();
    const earlier: Diagnostic[][] = [];
    for (;;) {
      const view = this.run(request, data);
      if (responseStatus(view) !== STATUS_NEED_CMAPS) {
        const response = view.slice();
        this.release();
        return { response, earlier };
      }
      const r = decodeResponse(view);
      this.release();
      earlier.push(r.diagnostics);
      await this.loadMissing(r.value as string[]);
    }
  }

  /** A request, re-run after loading the CMaps it misses; resolves to its
   * value or rejects with the Python exception it raised. */
  async call(op: string, handle: number, args: unknown, options: CallOptions = {}): Promise<unknown> {
    this.configure();
    for (;;) {
      const r = this.callRaw(op, handle, args, options);
      if (r.diagnostics.length || r.dropped) options.onDiagnostics?.(r.diagnostics, r.dropped);
      if (r.status !== STATUS_NEED_CMAPS) return responseValue(r, op);
      await this.loadMissing(r.value as string[]);
    }
  }

  /** `call` for the requests that cannot need a CMap. */
  callSync(op: string, handle: number, args: unknown, options: CallOptions = {}): unknown {
    this.configure();
    const r = this.callRaw(op, handle, args, options);
    if (r.diagnostics.length || r.dropped) options.onDiagnostics?.(r.diagnostics, r.dropped);
    return responseValue(r, op);
  }
}
