/**
 * Worker mode, worker side (`dist/browser/worker.js`): owns the WebAssembly
 * instance, runs encoded requests to their end (fetching the CMaps they
 * miss) and posts the final response bytes back (see client.ts). Messages
 * run one after the other, in the order they arrive: a request waiting for
 * a CMap holds the core (`Serial`), as in the page.
 */

import { Serial, WasmCore } from "../core/runtime.js";
import { CoreCrash } from "../errors.js";
import { fetchCMapSource } from "../platform/platform.js";
import type { WorkerReply, WorkerRequest } from "./client.js";

let core: Promise<WasmCore> | undefined;
const serial = new Serial();

const scope = globalThis as unknown as {
  onmessage: ((e: MessageEvent<WorkerRequest>) => void) | null;
  postMessage(m: WorkerReply, transfer?: Transferable[]): void;
};

async function handle(m: WorkerRequest): Promise<void> {
  try {
    if (m.type === "init") {
      core = WasmCore.create(m.module, m.cmapBase === null ? null : fetchCMapSource(new URL(m.cmapBase)));
      await core;
      scope.postMessage({ id: m.id, ready: true });
      return;
    }
    if (!core) throw new Error("worker not initialized");
    const response = await (await core).runToEnd(m.request, m.data);
    scope.postMessage({ id: m.id, response }, [response.buffer]);
  } catch (err) {
    const name = err instanceof CoreCrash ? "CoreCrash" : err instanceof Error ? err.name : "Error";
    scope.postMessage({ id: m.id, error: { name, message: err instanceof Error ? err.message : String(err) } });
  }
}

scope.onmessage = (e: MessageEvent<WorkerRequest>) => {
  void serial.run(() => handle(e.data));
};
