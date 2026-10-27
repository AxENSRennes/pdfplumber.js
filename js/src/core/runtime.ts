/**
 * The WebAssembly core: instantiation, the ABI of crates/wasm/src/abi.rs,
 * requests and responses (crates/wasm/src/ops.rs), and the lazy loading of
 * predefined CMaps (docs/js-runtime.md).
 */

import { CoreCrash, fromPython, type PythonErrorInfo } from "../errors.js";
import { PageRef, PDFStream, type StreamSource } from "../pdfobjects.js";
import { project } from "./typed.js";
import type { WasmModule } from "../types.js";
import {
  decodeBinary,
  decodeJson,
  encodeValue,
  withRefMakers,
  type PageRefMaker,
  type RefCheck,
  type RefMakers,
  type StreamRefMaker,
} from "./wire.js";

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
   * ones): those of its final run, Python's, once. A run that missed a
   * CMap is void, and reports none (crates/wasm/src/ops.rs). */
  onDiagnostics?: (entries: Diagnostic[], dropped: number) => void;
  /** The document whose job makes the request (its `PDFPlumberDocument`):
   * the streams of the result read their data in its jobs, and reject
   * with `DocumentReleased` once it is released (`StreamRefs`). */
  owner?: StreamOwner;
}

/** What a stream by reference reads its data through: its document. */
export interface StreamOwner {
  job<T>(f: (call: CoreCall) => Promise<T>): Promise<T>;
  callOptions?(page?: number): CallOptions;
}

/** One request, inside a job (`Core.exclusive`). */
export type CoreCall = (op: string, handle: number, args: unknown, options?: CallOptions) => Promise<unknown>;

/** What `open()` and the pages call: a core in this thread or in a worker.
 *
 * Requests run one at a time, in the order they are made (FIFO), each to
 * its end (the re-runs after a void call included) before the next starts:
 * the core is one Python process running one statement after the other
 * (docs/js-runtime.md, "API Layer", "Jobs And Order"). A job
 * (`exclusive`) holds the core for several requests and the JavaScript
 * between them (`search` runs its `RegExp` between two requests,
 * `filter` its function): no other request runs in between. */
export interface Core {
  /** One request, queued after every request and job made before it. */
  call(op: string, handle: number, args: unknown, options?: CallOptions): Promise<unknown>;
  /** A job: `f` runs when every earlier request and job has ended, with
   * a `call` that runs at once (`f` must not use `Core.call`, which would
   * wait for `f` itself). */
  exclusive<T>(f: (call: CoreCall) => Promise<T>): Promise<T>;
  readonly crashed: boolean;
  /** The typed value (core/typed.ts) of the last result, when the core
   * writes the typed format (`set_format("typed")`: the harness of
   * js/trace/); `undefined` otherwise. A `free` (a finalizer's, which can
   * run between a member's request and the read of its value) leaves it
   * as it is: it has no result anyone reads. */
  readonly lastTyped: unknown;
  /** Stops a core that holds more than references (a worker); a core in
   * the page is freed with its last reference. */
  terminate?(): void;
}

/** The document that decoded each `PageRef` of a result (the owner of the
 * request: `CallOptions.owner`): `pdf.page(ref)` takes the references of
 * its own document only (`pageRefOwner`). */
const pageRefOwners = new WeakMap<PageRef, StreamOwner | undefined>();

/** The document whose request decoded `ref` (`undefined`: a reference of
 * the internal API, of another decoding, or made by hand). */
export function pageRefOwner(ref: PageRef): StreamOwner | undefined {
  return pageRefOwners.get(ref);
}

type Live = WeakRef<PDFStream> | PDFStream;

/** The core whose results gave each object by reference (`StreamRefs`):
 * an argument of another core's request cannot hold it
 * (`StreamRefs.owns`). */
const origins = new WeakMap<object, StreamRefs>();

/** The objects by reference of a core's results (wire.ts `withRefMakers`):
 * one `PDFStream` per stream while it is alive (a handle names one stream
 * of one document as long as the document lives: the core frees its stream
 * handles with it), reading its data in its document's jobs
 * (`CallOptions.owner`), or as plain requests of the core (the internal
 * API, which has no document object); and the `PageRef`s, each noted as
 * its document's (`pageRefOwner`).
 *
 * The streams are kept by document, in a `WeakMap` keyed by the document:
 * nothing here keeps a document alive (a stream does, while the caller
 * keeps the stream), so an unreachable document is still released by its
 * `FinalizationRegistry` entry, its stream handles with it. A dead entry is
 * replaced when its handle comes again, and swept as the map grows. */
export class StreamRefs {
  readonly #byOwner = new WeakMap<StreamOwner, Map<number, Live>>();
  /** The streams of the requests without an owner (the internal API). */
  readonly #unowned = new Map<number, Live>();
  constructor(private readonly core: Core) {}

  /** Whether an argument of this core's requests may hold `ref`: an object
   * by reference this core's results gave, or one no core gave (made by
   * hand, as the tests of the internal API make the pages they name), not
   * one of another core (its handle names another object here, or none). */
  owns(ref: object): boolean {
    const origin = origins.get(ref);
    return origin === undefined || origin === this;
  }

  #streams(owner: StreamOwner | undefined): Map<number, Live> {
    if (owner === undefined) return this.#unowned;
    let m = this.#byOwner.get(owner);
    if (!m) this.#byOwner.set(owner, (m = new Map()));
    return m;
  }

  /** The makers of the objects by reference of a response to a request
   * made with `options`. */
  makers(options: CallOptions): RefMakers {
    const owner = options.owner;
    const stream: StreamRefMaker = (handle, objid, genno, attrs) => {
      const live = this.#streams(owner);
      const known = live.get(handle);
      if (known) {
        const s = known instanceof PDFStream ? known : known.deref();
        if (s) return s;
      }
      const source: StreamSource = owner
        ? { read: (op, h) => owner.job((call) => call(op, h, null, owner.callOptions?.())) }
        : { read: (op, h) => this.core.call(op, h, null) };
      const s = PDFStream.reference(objid, genno, attrs, handle, source);
      origins.set(s, this);
      if (typeof WeakRef !== "function") {
        live.set(handle, s);
        return s;
      }
      if (live.size >= 1024 && !known) sweep(live);
      live.set(handle, new WeakRef(s));
      return s;
    };
    const page: PageRefMaker = (handle, pageNumber) => {
      const ref = new PageRef(handle, pageNumber);
      origins.set(ref, this);
      pageRefOwners.set(ref, owner);
      return ref;
    };
    return { stream, page };
  }
}

/** Drops the dead entries of a map of streams once it has doubled since
 * the last sweep (amortized: each insertion pays a constant share). */
function sweep(live: Map<number, Live>): void {
  const sizes = sweptSizes.get(live) ?? 0;
  if (live.size < 2 * sizes) return;
  for (const [h, r] of live) if (!(r instanceof PDFStream) && r.deref() === undefined) live.delete(h);
  sweptSizes.set(live, Math.max(live.size, 512));
}
const sweptSizes = new WeakMap<Map<number, Live>, number>();

/** Decodes a response with the objects by reference of `refs`. */
export function decodeWithRefs(u8: Uint8Array, refs: StreamRefs, options: CallOptions): Response {
  return withRefMakers(refs.makers(options), () => decodeResponse(u8));
}

/** The FIFO of a core's requests and jobs (see `Core`). */
export class Serial {
  private tail: Promise<unknown> = Promise.resolve();

  run<T>(f: () => Promise<T>): Promise<T> {
    const result = this.tail.then(f, f);
    // the next job waits for this one, whatever its outcome
    this.tail = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }
}

export const STATUS_OK = 0;
export const STATUS_ERROR = 1;
export const STATUS_NEED_CMAPS = 2;

/** The payload format of a typed result (crates/wasm/src/ops.rs). */
export const FORMAT_TYPED = 2;


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
  /** The value (public: for a typed payload, the projection of `typed`). */
  value: unknown;
  /** The typed value of a payload in the typed format (`undefined`
   * otherwise). */
  typed?: unknown;
  diagnostics: Diagnostic[];
  dropped: number;
  /** Bytes of the payload (the benchmark). */
  payloadBytes: number;
  /** The history of the core's documents is long (flags bit 1,
   * crates/wasm/src/ops.rs `FLAG_HISTORY_LONG`): handing over every CMap
   * bounds it (no call can be void any more). */
  historyLong: boolean;
}

/** Whether a response asks for every CMap (`Response.historyLong`). */
export function historyLong(u8: Uint8Array): boolean {
  return (u8[6] & 2) !== 0;
}

/** The request block of an operation (`accept`: the objects by reference
 * its arguments may hold, those of the core it goes to: `StreamRefs.owns`). */
export function encodeRequest(op: string, handle: number, args: unknown, minLevel: number, accept?: RefCheck): Uint8Array {
  return encodeValue([op, handle, args ?? null, minLevel], accept);
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
  const typed = format === FORMAT_TYPED ? decodeBinary(payload, 0, true) : undefined;
  const value = format === FORMAT_TYPED ? project(typed) : format === 1 ? decodeJson(payload, fixup) : decodeBinary(payload);
  const diag = decodeBinary(u8.subarray(diagOff, diagOff + diagLen)) as {
    entries: [string, number, string][];
    dropped: number;
  };
  return {
    status,
    value,
    typed,
    diagnostics: diag.entries.map(([source, level, message]) => ({ source, level, message })),
    dropped: diag.dropped,
    payloadBytes: payloadLen,
    historyLong: historyLong(u8),
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
  static async create(module: WasmModule, cmaps: CMapSource | null): Promise<WasmCore> {
    const instance = await WebAssembly.instantiate(module as WebAssembly.Module, {});
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

  /** The size of the module's linear memory, in bytes (it never shrinks). */
  memoryBytes(): number {
    return this.ex.memory.buffer.byteLength;
  }

  /** Runs `f`, which calls the module's exports. Any exception out of it
   * leaves the module's state undefined (a trap is a
   * `WebAssembly.RuntimeError`, but a stack overflow is a `RangeError` in
   * V8 and JavaScriptCore and an `InternalError` in SpiderMonkey, thrown
   * while Rust holds borrows it never releases): the core is dead. */
  private guard<T>(f: () => T): T {
    if (this.dead !== undefined) throw new CoreCrash(this.dead);
    try {
      return f();
    } catch (e) {
      this.dead = e;
      throw new CoreCrash(e);
    }
  }

  /** Copies `bytes` into a buffer of the module. Addresses and lengths
   * of wasm32 are `i32` values, which JavaScript reads signed: `>>> 0`
   * reads them unsigned (an address past 2 GiB is not negative). */
  private put(bytes: Uint8Array): number {
    const ptr = this.ex.pp_alloc(bytes.length) >>> 0;
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
      return new Uint8Array(this.ex.memory.buffer, this.ex.pp_result_ptr() >>> 0, this.ex.pp_result_len() >>> 0);
    });
  }

  /** Drops the last response in the module. */
  release(): void {
    this.guard(() => this.ex.pp_result_free());
  }

  /** The streams and pages of the results (`StreamRefs`). */
  private readonly refs = new StreamRefs(this);
  /** The objects by reference an argument may hold: this core's. */
  private readonly accept: RefCheck = (ref) => this.refs.owns(ref);

  /** One request, no CMap loading. */
  callRaw(op: string, handle: number, args: unknown, options: CallOptions = {}): Response {
    const view = this.run(encodeRequest(op, handle, args, options.minLevel ?? 0, this.accept), options.data);
    try {
      return decodeWithRefs(view, this.refs, options);
    } finally {
      this.release();
    }
  }

  /** Hands the CMap names to the core, and has it write the streams of
   * its results by reference (`set_stream_refs`), once. */
  configure(): void {
    if (this.configured) return;
    this.configured = true;
    responseValue(this.callRaw("set_stream_refs", 0, [true]), "set_stream_refs");
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

  /** The calls that were void (a CMap missing), since the core started. */
  voidCalls = 0;

  /** Hands over every CMap not handed over yet: what a response whose
   * history is long asks for (`Response.historyLong`). The files that
   * cannot be fetched are marked unavailable in the core
   * (`cmap_unavailable`): the history then goes, and no later call asks
   * for every CMap again (each would fetch the missing files once more); a
   * call that needs one of them still fetches it (`loadMissing`). */
  async loadAll(): Promise<void> {
    if (!this.cmaps) return;
    const names = this.cmaps.names.filter((n) => !this.added.has(n));
    await Promise.all(names.map((n) => this.addCMap(n).catch(() => undefined)));
    const failed = names.filter((n) => !this.added.has(n));
    if (failed.length) responseValue(this.callRaw("cmap_unavailable", 0, failed), "cmap_unavailable");
  }

  /** Loads the CMaps a void call missed; refuses a name already handed
   * over (the core would loop). */
  async loadMissing(names: string[]): Promise<void> {
    const again = names.filter((n) => this.added.has(n));
    if (again.length) throw new Error(`the core asked again for CMaps it has: ${again.join(", ")}`);
    await Promise.all(names.map((n) => this.addCMap(n)));
  }

  /** Runs a request until it does not miss CMaps; returns its final
   * response, still undecoded beyond its status, as a copy (the void runs
   * report nothing). */
  async runToEnd(request: Uint8Array, data?: Uint8Array): Promise<Uint8Array> {
    this.configure();
    for (;;) {
      const view = this.run(request, data);
      if (responseStatus(view) !== STATUS_NEED_CMAPS) {
        const response = view.slice();
        this.release();
        if (historyLong(response)) await this.loadAll();
        return response;
      }
      const r = decodeResponse(view);
      this.release();
      this.voidCalls++;
      await this.loadMissing(r.value as string[]);
    }
  }

  private readonly serial = new Serial();
  private typedValue: unknown = undefined;

  /** The typed value of the last final response (`Core.lastTyped`). */
  get lastTyped(): unknown {
    return this.typedValue;
  }

  /** A request, queued (`Core.call`), re-run after loading the CMaps it
   * misses; resolves to its value or rejects with the Python exception it
   * raised. Only the final run's diagnostics are forwarded. */
  call(op: string, handle: number, args: unknown, options: CallOptions = {}): Promise<unknown> {
    return this.serial.run(() => this.callNow(op, handle, args, options));
  }

  /** `Core.exclusive` */
  exclusive<T>(f: (call: CoreCall) => Promise<T>): Promise<T> {
    return this.serial.run(() => f((op, handle, args, options) => this.callNow(op, handle, args, options)));
  }

  private async callNow(op: string, handle: number, args: unknown, options: CallOptions = {}): Promise<unknown> {
    this.configure();
    for (;;) {
      const r = this.callRaw(op, handle, args, options);
      if (r.status === STATUS_NEED_CMAPS) {
        this.voidCalls++;
        await this.loadMissing(r.value as string[]);
        continue;
      }
      if (op !== "free") this.typedValue = r.typed;
      if (r.diagnostics.length || r.dropped) options.onDiagnostics?.(r.diagnostics, r.dropped);
      if (r.historyLong) await this.loadAll();
      return responseValue(r, op);
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
