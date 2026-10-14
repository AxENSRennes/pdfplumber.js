// Browser entry, bundled by run-browsers.mjs; results land in window.__result.
import { runBench } from "./bench-core.ts";

const w = window as unknown as { __result?: unknown; __error?: string };
try {
  const [corpus, wasm] = await Promise.all([fetch("corpus.bin").then((r) => r.arrayBuffer()), fetch("../rust/psparser.wasm").then((r) => r.arrayBuffer())]);
  w.__result = await runBench(navigator.userAgent, new Uint8Array(corpus), wasm);
} catch (e) {
  w.__error = String(e) + "\n" + String((e as Error)?.stack ?? "");
}
// Browsers launched without automation (no debugger attached) report back over HTTP.
if (location.search.includes("post")) await fetch("/result", { method: "POST", body: JSON.stringify(w.__result ?? { error: w.__error }) });
