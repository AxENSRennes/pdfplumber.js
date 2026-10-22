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
  decodeResponse,
  encodeRequest,
  responseValue,
  type CallOptions,
  type Core,
  type CoreCall,
} from "../core/runtime.js";

/** Messages from the worker. */
export type WorkerReply =
  | { id: number; response: Uint8Array }
  | { id: number; error: { name: string; message: string } }
  | { id: number; ready: true };

/** Messages to the worker. */
export type WorkerRequest =
  | { type: "init"; id: number; module: WebAssembly.Module; cmapBase: string | null }
  | { type: "call"; id: number; request: Uint8Array; data?: Uint8Array };

export class WorkerCore implements Core {
  private next = 1;
  private readonly waiting = new Map<number, (r: WorkerReply) => void>();
  private dead: unknown = undefined;
  private readonly serial = new Serial();
  private typedValue: unknown = undefined;

  private constructor(private readonly worker: Worker) {
    worker.onmessage = (e: MessageEvent<WorkerReply>) => {
      const resolve = this.waiting.get(e.data.id);
      this.waiting.delete(e.data.id);
      resolve?.(e.data);
    };
    worker.onerror = (e) => {
      this.dead = e.message;
      for (const resolve of this.waiting.values()) resolve({ id: 0, error: { name: "CoreCrash", message: e.message } });
      this.waiting.clear();
    };
  }

  static async start(worker: Worker, module: WebAssembly.Module, cmapBase: string | null): Promise<WorkerCore> {
    const core = new WorkerCore(worker);
    const r = await core.post({ type: "init", id: 0, module, cmapBase });
    if ("error" in r) throw new Error(r.error.message);
    return core;
  }

  get crashed(): boolean {
    return this.dead !== undefined;
  }

  get lastTyped(): unknown {
    return this.typedValue;
  }

  /** Stops the worker. */
  terminate(): void {
    this.worker.terminate();
    this.dead = "terminated";
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
    const request = encodeRequest(op, handle, args, options.minLevel ?? 0);
    const r = await this.post({ type: "call", id: 0, request, data: options.data }, [request.buffer]);
    if ("error" in r) {
      if (r.error.name === "CoreCrash") {
        this.dead = r.error.message;
        throw new CoreCrash(r.error.message);
      }
      const e = new Error(r.error.message);
      e.name = r.error.name;
      throw e;
    }
    if (!("response" in r)) throw new Error("unexpected worker reply");
    const response = decodeResponse(r.response);
    this.typedValue = response.typed;
    if (response.diagnostics.length || response.dropped)
      options.onDiagnostics?.(response.diagnostics, response.dropped);
    return responseValue(response, op);
  }
}
