// The result-encoding benchmark (docs/js-runtime.md, "Format des
// résultats"): the core encodes the chars and edges of real pages (the
// corpus written by crates/wasm/examples/bench_corpus.rs), in the binary
// PJB1 format and in JSON; JavaScript decodes them into the same objects.
// Runtime-agnostic: `lib` is the internal module (dist/src/internal.js in
// Node, dist/browser/internal.js in browsers).
//
// Each pass covers every page, one call per page (a page's chars is the
// typical result), then the whole corpus in one call. "core" is a pass that
// runs the requests without decoding (Rust encoding, envelope, copies),
// "total" a pass that also decodes; "decode" is their difference. Times are
// medians over the passes, in ms.

function deepEqual(a, b) {
  const stack = [[a, b]];
  while (stack.length) {
    const [x, y] = stack.pop();
    if (typeof x === "number" && typeof y === "number") {
      if (!Object.is(x, y)) return false;
      continue;
    }
    if (x === y) continue;
    if (typeof x !== "object" || typeof y !== "object" || x === null || y === null) return false;
    if (Array.isArray(x) !== Array.isArray(y)) return false;
    const kx = Object.keys(x);
    const ky = Object.keys(y);
    if (kx.length !== ky.length || kx.some((k, i) => k !== ky[i])) return false;
    for (const k of kx) stack.push([x[k], y[k]]);
  }
  return true;
}

const median = (xs) => {
  const s = [...xs].sort((p, q) => p - q);
  return s[Math.floor(s.length / 2)];
};

export async function runEncodingBench(lib, corpus, { passes = 5, log = () => {} } = {}) {
  const core = await lib.getCore();
  core.configure();
  const loaded = core.callRaw("bench_load", 0, null, { data: corpus });
  if (loaded.status !== 0) throw new Error(`bench_load failed: ${JSON.stringify(loaded.value)}`);
  const handle = loaded.value;
  const count = core.callRaw("bench_encode", handle, ["binary", null]).value.length;

  const request = (format, index) => lib.encodeRequest("bench_encode", handle, [format, index], 0);
  const results = {};
  for (const format of ["binary", "json"]) {
    const perPage = Array.from({ length: count }, (_, i) => request(format, i));
    const whole = request(format, null);
    let bytes = 0;
    const runPass = (requests, decode) => {
      const t0 = performance.now();
      let sink = 0;
      for (const req of requests) {
        const view = core.run(req);
        if (decode) sink += lib.decodeResponse(view).payloadBytes;
        core.release();
      }
      const t = performance.now() - t0;
      if (decode) bytes = sink;
      return t;
    };
    // warm-up (JIT, memory growth)
    runPass(perPage, true);
    runPass([whole], true);
    const m = { pagesCore: [], pagesTotal: [], wholeCore: [], wholeTotal: [] };
    for (let p = 0; p < passes; p++) {
      m.pagesCore.push(runPass(perPage, false));
      m.pagesTotal.push(runPass(perPage, true));
      m.wholeCore.push(runPass([whole], false));
      m.wholeTotal.push(runPass([whole], true));
      log(`${format} pass ${p + 1}/${passes}`);
    }
    const pagesBytes = bytes;
    runPass([whole], true);
    results[format] = {
      pages: { core: median(m.pagesCore), total: median(m.pagesTotal), bytes: pagesBytes },
      whole: { core: median(m.wholeCore), total: median(m.wholeTotal), bytes },
    };
    for (const k of ["pages", "whole"]) results[format][k].decode = results[format][k].total - results[format][k].core;
  }
  // both formats decode to the same objects
  let same = 0;
  for (let i = 0; i < count; i++) {
    const a = core.callRaw("bench_encode", handle, ["binary", i]).value;
    const b = core.callRaw("bench_encode", handle, ["json", i]).value;
    if (deepEqual(a, b)) same++;
  }
  core.callRaw("free", handle, null);
  return { pages: count, identical: same, results };
}
