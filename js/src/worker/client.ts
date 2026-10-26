/**
 * Worker mode, main-thread side: the requests are encoded here, run (with
 * their CMap reloads) in the worker, and the final response bytes come back
 * to be decoded here, so that results keep their classes (`PSLiteral`...).
 * Requests and jobs are queued here as in the page (`Core`, FIFO); the
 * worker runs the messages it gets one after the other too.
 */

import { CoreCrash } from "../errors.js";
import {
  Serial,
  StreamRefs,
  decodeWithRefs,
  encodeRequest,
  responseValue,
  type CallOptions,
  type Core,
  type CoreCall,
} from "../core/runtime.js";
import type { RefCheck } from "../core/wire.js";
import type { WasmModule } from "../types.js";

/** Messages from the worker. */
export type WorkerReply =
  | { id: number; response: Uint8Array }
  | { id: number; error: { name: string; message: string } }
  | { id: number; ready: true };

/** Messages to the worker. */
export type WorkerRequest =
  | { type: "init"; id: number; module: WasmModule; cmapBase: string | null }
  | { type: "call"; id: number; request: Uint8Array; data?: Uint8Array };

/** What this module uses of a `Worker` (a stand-in in the tests). */
export interface WorkerLike {
  onmessage: ((e: MessageEvent<WorkerReply>) => void) | null;
  onerror: ((e: ErrorEvent) => void) | null;
  postMessage(m: WorkerRequest, transfer: Transferable[]): void;
  terminate(): void;
}

export class WorkerCore implements Core {
  private next = 1;
  private readonly waiting = new Map<number, (r: WorkerReply) => void>();
  private dead: unknown = undefined;
  private readonly serial = new Serial();
  private typedValue: unknown = undefined;
  /** The streams and pages of the results, decoded here (`StreamRefs`). */
  private readonly refs = new StreamRefs(this);
  /** The objects by reference an argument may hold: this core's. */
  private readonly accept: RefCheck = (ref) => this.refs.owns(ref);

  private constructor(
    private readonly worker: WorkerLike,
    /** The script it runs (error messages). */
    private readonly url: string,
  ) {
    worker.onmessage = (e: MessageEvent<WorkerReply>) => {
      const resolve = this.waiting.get(e.data.id);
      this.waiting.delete(e.data.id);
      resolve?.(e.data);
    };
    worker.onerror = (e) => {
      // a script that cannot load (a URL a bundler moved, a module that
      // cannot resolve its imports) gives an event without a message
      this.stop(e.message || `the pdfplumber worker ${this.url} failed: it could not be loaded, or it stopped`);
    };
  }

  /** Starts the core in `worker` (`url`: the script it runs, for the error
   * messages). */
  static async start(worker: WorkerLike, module: WasmModule, cmapBase: string | null, url = "script"): Promise<WorkerCore> {
    const core = new WorkerCore(worker, url);
    const r = await core.post({ type: "init", id: 0, module, cmapBase });
    if ("error" in r) {
      core.terminate();
      throw new Error(r.error.message || `the pdfplumber worker ${url} did not start`);
    }
    return core;
  }

  get crashed(): boolean {
    return this.dead !== undefined;
  }

  get lastTyped(): unknown {
    return this.typedValue;
  }

  /** Stops the worker (its WebAssembly memory goes with it); the requests
   * still waiting for a reply reject with `CoreCrash`. */
  terminate(): void {
    this.stop("terminated");
  }

  /** The core is dead (`reason`): the worker is stopped and every waiting
   * request is answered with a `CoreCrash`. */
  private stop(reason: string): void {
    if (this.dead === undefined) this.dead = reason;
    this.worker.terminate();
    const waiting = [...this.waiting.values()];
    this.waiting.clear();
    for (const resolve of waiting) resolve({ id: 0, error: { name: "CoreCrash", message: String(this.dead) } });
  }

  private post(m: WorkerRequest, transfer: Transferable[] = []): Promise<WorkerReply> {
    if (this.dead !== undefined) return Promise.reject(new CoreCrash(this.dead));
    const id = this.next++;
    m.id = id;
    return new Promise((resolve) => {
      this.waiting.set(id, resolve);
      this.worker.postMessage(m, transfer);
    });
  }

  call(op: string, handle: number, args: unknown, options: CallOptions = {}): Promise<unknown> {
    return this.serial.run(() => this.callNow(op, handle, args, options));
  }

  exclusive<T>(f: (call: CoreCall) => Promise<T>): Promise<T> {
    return this.serial.run(() => f((op, handle, args, options) => this.callNow(op, handle, args, options)));
  }

  private async callNow(op: string, handle: number, args: unknown, options: CallOptions = {}): Promise<unknown> {
    const request = encodeRequest(op, handle, args, options.minLevel ?? 0, this.accept);
    const r = await this.post({ type: "call", id: 0, request, data: options.data }, [request.buffer]);
    if ("error" in r) {
      if (r.error.name === "CoreCrash") {
        // the worker's instance is lost: stop the worker now, not when the
        // garbage collector reclaims it (its memory can be what crashed it:
        // gigabytes)
        this.stop(r.error.message);
        throw new CoreCrash(r.error.message);
      }
      const e = new Error(r.error.message);
      e.name = r.error.name;
      throw e;
    }
    if (!("response" in r)) throw new Error("unexpected worker reply");
    const response = decodeWithRefs(r.response, this.refs, options);
    if (op !== "free") this.typedValue = response.typed;
    if (response.diagnostics.length || response.dropped)
      options.onDiagnostics?.(response.diagnostics, response.dropped);
    return responseValue(response, op);
  }
}
