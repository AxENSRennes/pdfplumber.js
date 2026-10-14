// Browser entry, bundled by run-browsers.mjs; results land in window.__result.
import { runBench } from "./bench-core.ts";

const w = window as unknown as { __result?: unknown; __error?: string };
try {
  const get = (url: string) => fetch(url).then((r) => r.arrayBuffer());
  const [corpus, fuzz, wasm] = await Promise.all([get("corpus.bin"), get("fuzz.bin"), get("../rust/tables.wasm")]);
  w.__result = await runBench(navigator.userAgent, { corpus: new Uint8Array(corpus), fuzz: new Uint8Array(fuzz) }, wasm);
} catch (e) {
  w.__error = String(e) + "\n" + String((e as Error)?.stack ?? "");
}
// Browsers launched without automation report back over HTTP.
if (location.search.includes("post")) await fetch("/result", { method: "POST", body: JSON.stringify(w.__result ?? { error: w.__error }) });
