// The runtime tests, shared by Node (js/test/node/runtime.test.ts, vitest)
// and the browsers (js/test/browser/runtime.spec.ts, Playwright): each test
// is an async function of a context and throws on failure. The runner
// configures the runtime before each test (`configure({})`, or worker mode);
// `only: "direct"` marks the tests that need the core in the same thread
// (CMap loader functions, buffer counts); `todo` the tests that wait for a
// later part of the port.
//
// ctx = {
//   lib,              the package (public API and internal.ts)
//   runtime,          "node" | "browser"
//   oracle(name),     lines of <cache>/unit/js_runtime/<fp>/<name>.jsonl (oracle/unit/js_runtime.py)
//   readFile(rel),    bytes of a file under PDFPLUMBER_ROOT
//   fileInput(rel),   the same file as an input open() reads by itself (path or URL)
//   missingInput(),   an input that does not exist (path or URL)
//   readCMap(name),   the file of a CMap in dist/cmaps/
// }
import { difference, fromTyped } from "./typed.mjs";

/** A valid PDF of one page whose content stream is `size` spaces. */
function pdfWithStream(size) {
  const enc = new TextEncoder();
  const parts = [];
  const offsets = [];
  let length = 0;
  const push = (b) => {
    parts.push(b);
    length += b.length;
  };
  push(enc.encode("%PDF-1.4\n"));
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R >>",
  ];
  objects.forEach((o, i) => {
    offsets.push(length);
    push(enc.encode(`${i + 1} 0 obj\n${o}\nendobj\n`));
  });
  offsets.push(length);
  push(enc.encode(`4 0 obj\n<< /Length ${size} >>\nstream\n`));
  push(new Uint8Array(size).fill(0x20));
  push(enc.encode("\nendstream\nendobj\n"));
  const xref = length;
  const entries = offsets.map((o) => `${String(o).padStart(10, "0")} 00000 n \n`).join("");
  push(enc.encode(`xref\n0 5\n0000000000 65535 f \n${entries}trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`));
  const out = new Uint8Array(length);
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
}

/** The search options `Page.search` takes itself (the others are the text
 * map's). */
const SEARCH_ARGS = ["regex", "case", "main_group", "return_chars", "return_groups"];

/** Replays one session of pages.jsonl (oracle/unit/js_pages_run.py: the
 * statements of a Python script) through the boundary's operations, in a
 * new core (a new Python process); returns the number of cases checked.
 * `search` runs as the public API will: the text, the JavaScript pattern
 * over it (whose spans must be Python's), then the results. */
async function replaySession(lib, input, out, readFile) {
  lib.dropCore();
  const t = await lib.internals();
  const name = fromTyped(input[0], lib);
  const cases = input[1];
  const { results } = fromTyped(out, lib);
  const handles = [];
  const label = (h) => {
    handles.push(h);
    return `#${handles.length - 1}`;
  };
  const relabel = (v) => {
    if (typeof v === "number") return label(v);
    if (Array.isArray(v)) return v.map(relabel);
    if (v && typeof v === "object" && "handle" in v) return { ...v, handle: label(v.handle) };
    return v;
  };
  // the pages a result holds (PageRef): their handles as labels, a handle
  // labelled before keeping its label
  const relabelPages = (v) => {
    if (v instanceof lib.PageRef) {
      let k = handles.indexOf(v.handle);
      if (k < 0) k = handles.push(v.handle) - 1;
      return new lib.PageRef(`#${k}`, v.page_number);
    }
    if (Array.isArray(v)) return v.map(relabelPages);
    if (v && typeof v === "object" && Object.getPrototypeOf(v) === Object.prototype) {
      const o = {};
      for (const k of Object.keys(v)) {
        Object.defineProperty(o, k, { value: relabelPages(v[k]), writable: true, enumerable: true, configurable: true });
      }
      return o;
    }
    return v;
  };
  const makers = new Set([
    "doc_open", "doc_pages", "page_crop", "page_within_bbox", "page_outside_bbox", "page_filter",
    "page_dedupe_chars", "page_find_tables", "page_find_table",
  ]);
  let checked = 0;
  for (let i = 0; i < cases.length; i++) {
    const [opT, targetT, argsT, dataT, jsT] = cases[i];
    const op = fromTyped(opT, lib);
    const target = fromTyped(targetT, lib);
    const handle = target === null ? 0 : handles[target];
    const args = fromTyped(argsT, lib, true);
    const [want, extra] = results[i];
    const where = `${name} #${i} ${op} ${JSON.stringify(fromTyped(argsT, lib), (k, v) => (typeof v === "bigint" ? `${v}n` : v))}`;
    let got;
    try {
      if (op === "doc_open") {
        const data = typeof dataT === "string" ? await readFile(dataT) : fromTyped(dataT, lib);
        got = await t.call("doc_open", args[0], 0, { data });
      } else if (op === "search") {
        const [, options] = args;
        const kw = {};
        const search = {};
        for (const [k, v] of Object.entries(options)) (SEARCH_ARGS.includes(k) ? search : kw)[k] = v;
        const text = await t.call("page_search_text", [kw], handle);
        assertSame(text, extra.text, `${where}: the text`);
        const re = lib.compilePattern(fromTyped(jsT, lib), { regex: search.regex, case: search.case });
        const { spans, named } = lib.matchSpans(text, re);
        assertSame(spans, extra.spans, `${where}: JavaScript's spans`);
        // (without a match, JavaScript cannot tell whether the pattern has named
        // groups: no list, which the core takes as Python's empty one)
        if (spans.length) assertSame(named, extra.named, `${where}: JavaScript's named groups`);
        got = await t.call(
          "page_search",
          [kw, spans, search.return_groups ?? true, search.return_chars ?? true, search.main_group ?? 0, named],
          handle,
        );
      } else if (op === "page_filter") {
        got = await t.call(op, [extra], handle);
      } else {
        got = await t.call(op, args, handle);
      }
      if (makers.has(op)) got = relabel(got);
      got = relabelPages(got);
    } catch (e) {
      if (!(e instanceof lib.PDFPlumberError)) throw e;
      got = { exception: e.name, message: e.message };
    }
    assertSame(got, want, where);
    checked++;
  }
  lib.dropCore();
  return checked;
}

function assert(cond, message) {
  if (!cond) throw new Error(`assertion failed: ${message}`);
}

function assertSame(actual, expected, what) {
  const d = difference(actual, expected);
  if (d) throw new Error(`${what}: ${d}`);
}

async function rejects(promise, check, what) {
  try {
    await promise;
  } catch (e) {
    check(e);
    return e;
  }
  throw new Error(`${what}: did not reject`);
}

/** The values crates/wasm/src/ops.rs write_test_values writes by hand, as
 * JavaScript must receive them (transport only: no Python semantics). */
function expectedTestValues(lib) {
  const deep = [];
  let cur = deep;
  for (let i = 1; i < 100_000; i++) {
    const next = [];
    cur.push(next);
    cur = next;
  }
  cur.push(null);
  const row = (i) => {
    const o = {
      n: i === 1 ? 7 : i + 0.5,
      s: i % 2 === 0 ? "even" : "odd",
      os: i === 1 ? null : "x",
      b: i % 2 === 0,
      none: null,
      seq: i === 2 ? null : [i, 1],
      on: i === 0 ? null : -0,
      g: i === 0 ? 2n ** 60n : ["a"],
    };
    Object.defineProperty(o, "__proto__", { value: i, enumerable: true, writable: true, configurable: true });
    return o;
  };
  const dictKeys = {};
  for (const [k, v] of [
    ["__proto__", 1],
    ["\u0000", 2],
    ["\ud800", 3],
    ["constructor", null],
  ])
    Object.defineProperty(dictKeys, k, { value: v, enumerable: true, writable: true, configurable: true });
  return {
    nan: NaN,
    inf: Infinity,
    "-inf": -Infinity,
    "-0.0": -0,
    "0.1": 0.1,
    "5e-324": 5e-324,
    "1.7976931348623157e308": Number.MAX_VALUE,
    "1e21": 1e21,
    "float 3.0": 3,
    "int 3": 3,
    "int -2147483649": -2147483649,
    "int 2**53": 2 ** 53,
    "int 2**53+1": 2n ** 53n + 1n,
    "int big": -123456789012345678901234567890n,
    "str wtf8 lone high": "a\ud800b",
    "str wtf8 lone low": "\udc00",
    "str wtf8 split pair": "😀",
    "str utf16 lone": "\udfffA\ud800",
    "str code points": "\u{1f600}\udc00é\u0000",
    "str nul": "\u0000x",
    "str escapes": '"\\\n\t\u0001\u007f é😀',
    bytes: new Uint8Array([0, 255, 128]),
    "dict keys": dictKeys,
    "pdf objects": [
      new lib.PSLiteral("DeviceRGB"),
      new lib.PSLiteral(new Uint8Array([0xff])),
      new lib.PSKeyword(new Uint8Array([0x42, 0x54])),
      new lib.PDFObjRef(12),
    ],
    stream: new lib.PDFStream(5, 0, { Length: 3 }, new Uint8Array([97, 98, 99]), null),
    deep,
    table: [row(0), row(1), row(2)],
  };
}

/** A list nested `n` levels deep around `null`. */
function nested(n) {
  let v = null;
  for (let i = 0; i < n; i++) v = [v];
  return v;
}

function depthOf(v) {
  let d = 0;
  while (Array.isArray(v)) {
    d++;
    v = v[0];
  }
  return d;
}

/** A WebAssembly module with the exports of crates/wasm/src/abi.rs whose
 * `pp_call` recurses without end: the engine's stack overflow (a
 * `RangeError` in V8 and JavaScriptCore, an `InternalError` in
 * SpiderMonkey, not a `WebAssembly.RuntimeError`) leaves the instance's
 * state undefined. */
function overflowingModule() {
  const uleb = (n) => {
    const out = [];
    do {
      let b = n & 0x7f;
      n >>>= 7;
      if (n) b |= 0x80;
      out.push(b);
    } while (n);
    return out;
  };
  const name = (s) => [...uleb(s.length), ...[...s].map((c) => c.charCodeAt(0))];
  const section = (id, bytes) => [id, ...uleb(bytes.length), ...bytes];
  const vec = (items) => [...uleb(items.length), ...items.flat()];
  const I32 = 0x7f;
  const types = [
    [0x60, 1, I32, 1, I32], // 0: (i32) -> i32
    [0x60, 1, I32, 0], // 1: (i32) -> ()
    [0x60, 2, I32, I32, 1, I32], // 2: (i32, i32) -> i32
    [0x60, 0, 1, I32], // 3: () -> i32
    [0x60, 0, 0], // 4: () -> ()
  ];
  const funcs = [0, 1, 2, 3, 3, 4, 3];
  const exportsList = [
    [...name("memory"), 2, 0],
    ...["pp_alloc", "pp_free", "pp_call", "pp_result_ptr", "pp_result_len", "pp_result_free", "pp_buffers"].map(
      (n, i) => [...name(n), 0, i],
    ),
  ];
  const body = (code) => [...uleb(code.length + 1), 0, ...code];
  const bodies = [
    body([0x41, 8, 0x0b]), // pp_alloc: 8
    body([0x0b]),
    body([0x20, 0, 0x20, 1, 0x10, 2, 0x0b]), // pp_call: pp_call(request, data)
    body([0x41, 0, 0x0b]),
    body([0x41, 0, 0x0b]),
    body([0x0b]),
    body([0x41, 0, 0x0b]),
  ];
  return new Uint8Array([
    0x00, 0x61, 0x73, 0x6d, 1, 0, 0, 0,
    ...section(1, vec(types)),
    ...section(3, vec(funcs.map((f) => [f]))),
    ...section(5, [1, 0, 1]),
    ...section(7, vec(exportsList)),
    ...section(10, vec(bodies)),
  ]);
}

/** Values that must cross JavaScript → Rust → JavaScript unchanged. */
function echoValues(lib) {
  const values = [
    null,
    true,
    false,
    0,
    -0,
    1,
    -1,
    2 ** 31 - 1,
    -(2 ** 31),
    2 ** 31,
    Number.MAX_SAFE_INTEGER,
    -Number.MAX_SAFE_INTEGER,
    2 ** 53,
    2 ** 60,
    0.1,
    -1.5e-300,
    5e-324,
    Number.MAX_VALUE,
    NaN,
    Infinity,
    -Infinity,
    10n ** 40n,
    -(10n ** 40n),
    "",
    "plain",
    "é😀\u0000 ",
    new Uint8Array([0, 1, 255]),
    [],
    {},
    [1, [2, [3, [4]]]],
    { b: 1, a: [2, { c: "x" }], "": null },
  ];
  // the core's ints and floats are both numbers here; BigInts within ±2^53
  // come back as numbers
  return values.map((v) => [v, typeof v === "bigint" && v >= -(2n ** 53n) && v <= 2n ** 53n ? Number(v) : v]);
}

export const tests = [
  {
    name: "the WebAssembly core loads and reports its version",
    async run({ lib }) {
      const t = await lib.internals();
      const info = await t.info();
      assert(info.pdfminer === "20260107" && info.pdfplumber === "0.11.9", JSON.stringify(info));
    },
  },
  {
    name: "adversarial values cross unchanged (binary and JSON results)",
    async run({ lib }) {
      const t = await lib.internals();
      const expected = expectedTestValues(lib);
      try {
        for (const format of ["binary", "json"]) {
          await t.setFormat(format);
          assertSame(await t.testValues(), expected, `test_values (${format})`);
          for (const [v, back] of echoValues(lib)) assertSame(await t.echo(v), back, `echo ${String(v)} (${format})`);
          // tuples and floats given explicitly
          assertSame(await t.echo(new lib.PyTuple([new lib.PyFloat(3), 1])), [3, 1], `tuple (${format})`);
        }
      } finally {
        await t.setFormat("binary");
      }
    },
  },
  {
    name: "arguments nested 10^5 levels deep cross both ways; cyclic arguments are refused",
    async run({ lib, readFile }) {
      const t = await lib.internals();
      const pdf = await lib.open(await readFile("corpus/micro/text-operators.pdf"));
      try {
        assert(lib.encodeValue(nested(100_000)).length === 16 + 2 * 100_000 + 1, "encoded size");
        assert(depthOf(await t.echo(nested(100_000))) === 100_000, "depth after echo");
        assert(depthOf(await t.echo({ a: [nested(50_000)] }).then((v) => v.a[0])) === 50_000, "in a dict");
        const cyclic = [1];
        cyclic.push({ k: cyclic });
        await rejects(t.echo(cyclic), (e) => assert(e instanceof TypeError, e.name), "cycle");
        // the same object twice is not a cycle
        const shared = { x: 1 };
        assertSame(await t.echo([shared, [shared]]), [{ x: 1 }, [{ x: 1 }]], "shared");
        // the core is alive, with the document opened before
        const info = await t.info();
        assert(info.handles === 1 && !t.core.crashed, JSON.stringify(info));
      } finally {
        await pdf.close();
      }
    },
  },
  {
    name: "an exception out of the module (stack overflow) marks the core as crashed",
    only: "direct",
    async run({ lib }) {
      const core = await lib.WasmCore.create(new WebAssembly.Module(overflowingModule()), null);
      assert(!core.crashed, "fresh");
      await rejects(core.call("info", 0, null), (e) => assert(e instanceof lib.CoreCrash, `${e.name} ${e.message}`), "overflow");
      assert(core.crashed, "crashed after the overflow");
      await rejects(core.call("info", 0, null), (e) => assert(e instanceof lib.CoreCrash, e.name), "dead");
    },
  },
  {
    name: "table rows keep their keys when two shapes differ only in where U+0000 falls",
    async run({ lib }) {
      const t = await lib.internals();
      const a = { "a\u0000b": 1, c: 2 };
      const b = { a: 1, "b\u0000c": 2 };
      for (const format of ["binary", "json"]) {
        await t.setFormat(format);
        assertSame(await t.echo([a, a]), [a, a], `first shape (${format})`);
        assertSame(await t.echo([b, b]), [b, b], `second shape (${format})`);
        assertSame(await t.echo([a, b, a, b]), [a, b, a, b], `both (${format})`);
      }
      await t.setFormat("binary");
    },
  },
  {
    name: "lone surrogates cross from Rust to JavaScript (hand-made generalized UTF-8)",
    async run({ lib }) {
      const t = await lib.internals();
      for (const s of ["\ud800", "a\udc00b", "\udbff\udbff", "😀", "x\u{10ffff}\ud800\u0000"]) {
        const bytes = lib.encodeGeneralizedUtf8(s);
        assertSame(await t.call("echo_wtf8", bytes), s, `echo_wtf8 ${JSON.stringify(s)}`);
        assertSame(lib.decodeGeneralizedUtf8(bytes), s, "decodeGeneralizedUtf8");
      }
      // hand-made: U+D800 as ED A0 80 between two ASCII letters
      assertSame(await t.call("echo_wtf8", new Uint8Array([0x61, 0xed, 0xa0, 0x80, 0x62])), "a\ud800b", "ED A0 80");
      await rejects(
        t.call("echo_wtf8", new Uint8Array([0xc0, 0x80])),
        (e) => assert(e.name === "ValueError", e.name),
        "overlong UTF-8",
      );
      // a str argument with a lone surrogate is a Python str (pycompat::PyStr),
      // as a value and as a dict key, and comes back unchanged
      for (const v of ["\ud800", "a\udc00b", ["\udbff", { "k\ud800": "\udfff" }]]) {
        assertSame(await t.echo(v), v, `lone surrogate argument ${JSON.stringify(v)}`);
      }
    },
  },
  {
    name: "a hand-made PJB1 block decodes (strings with surrogates, tables)",
    async run({ lib }) {
      // header, then: dict(2) {"k": "a\ud800", "t": table(2 rows, 1 shape, 1 col "k": F64 [1.5, -0])}
      const b = [];
      const u32 = (v) => b.push(v & 255, (v >> 8) & 255, (v >> 16) & 255, v >>> 24);
      b.push(0x50, 0x4a, 0x42, 0x31);
      u32(0);
      u32(0); // string table offset, patched below
      u32(0);
      b.push(0x0b, 2, 0x07, 0, 0x07, 1, 0x07, 2); // dict(2), "k": str#1
      b.push(0x10, 2, 1, 2, 1, 0); // table: 2 rows, 1 shape: 2 rows, 1 col, key #0
      b.push(1); // F64 column
      while (b.length % 8) b.push(0);
      const f = new DataView(new ArrayBuffer(16));
      f.setFloat64(0, 1.5, true);
      f.setFloat64(8, -0, true);
      b.push(...new Uint8Array(f.buffer));
      while (b.length % 4) b.push(0);
      const table = b.length;
      b[8] = table & 255;
      b[9] = table >> 8;
      // 3 strings, 1 special: plain "k", "t" (UTF-8 blob "kt"), special #1 = "a\ud800"
      u32(3);
      u32(1);
      u32(2);
      b.push(0x6b, 0x74, 0, 0);
      u32(1);
      u32(2); // UTF-16 end offsets of the plain strings #0 and #2
      u32(1);
      u32(2);
      b.push(0x61, 0, 0x00, 0xd8);
      // keys: #0 "k" #1 "a\ud800" #2 "t"
      const value = lib.decodeBinary(new Uint8Array(b));
      assertSame(value, { k: "a\ud800", t: [{ k: 1.5 }, { k: -0 }] }, "hand-made block");
    },
  },
  {
    name: "Python exceptions arrive with their class, MRO and message",
    async run({ lib, oracle }) {
      const t = await lib.internals();
      const mro = new Map((await oracle("exceptions")).map((l) => [l.in[0], l.out]));
      const kinds = await t.exceptionKinds();
      let n = 0;
      for (const q of kinds) {
        const e = await rejects(t.raise(q, `msg ${q}`), () => {}, q);
        if (q === "pycompat.NotPorted") {
          assert(e instanceof lib.NotImplemented && e.name === "NotImplemented", "NotPorted → NotImplemented");
          continue;
        }
        assert(e instanceof lib.PDFPlumberError && e instanceof Error, `${q} class`);
        const expected = mro.get(q);
        assert(expected, `${q}: no oracle MRO`);
        assertSame([...e.pythonMro], expected, `${q} MRO`);
        assert(e.name === q.slice(q.lastIndexOf(".") + 1), `${q} name ${e.name}`);
        assert(e.message === `msg ${q}`, `${q} message`);
        assert(e.isInstance("BaseException") && e.isInstance(q), `${q} isInstance`);
        n++;
      }
      assert(n === kinds.length - 1 && n > 60, `${n} classes`);
      const e = await rejects(t.raise("PDFSyntaxError", ""), () => {}, "PDFSyntaxError");
      assert(e.isInstance("PSException") && !e.isInstance("ValueError"), "isinstance");
      // str(e) holding a lone surrogate arrives as Python has it
      const s = await rejects(t.raise("KeyError", "'\udc80'"), () => {}, "KeyError");
      assertSame(s.message, "'\udc80'", "a message with a lone surrogate");
    },
  },
  {
    name: "diagnostics reach the callback, filtered by level",
    async run({ lib }) {
      const t = await lib.internals();
      const entries = [
        ["pdfminer.pdfdocument", 10, "debug"],
        ["pdfminer.pdfinterp", 30, "warning"],
        ["UserWarning", 30, "w é😀"],
        ["pdfminer.psparser", 40, "error"],
      ];
      const got = [];
      await t.warn(entries, { minLevel: 30, onDiagnostics: (d, dropped) => got.push([d, dropped]) });
      assertSame(
        got,
        [[entries.slice(1).map(([source, level, message]) => ({ source, level, message })), 0]],
        "level 30",
      );
      const none = [];
      await t.warn(entries, { minLevel: 0, onDiagnostics: (d) => none.push(d) });
      assert(none.length === 0, "nothing collected at level 0");
      const many = [];
      await t.warn(
        Array.from({ length: 1005 }, (_, i) => ["x", 30, `${i}`]),
        { minLevel: 10, onDiagnostics: (d, dropped) => many.push([d.length, dropped]) },
      );
      assertSame(many, [[1000, 5]], "bounded");
      // the public policy: onWarning gets warnings and above, with the level name
      const warnings = [];
      const policy = lib.diagnosticsPolicy({ onWarning: (w) => warnings.push(w) }, 3);
      assert(policy.minLevel === 30, "onWarning: level 30");
      await t.warn(entries, policy);
      assertSame(
        warnings,
        [
          { source: "pdfminer.pdfinterp", level: "warning", message: "warning", page: 3 },
          { source: "UserWarning", level: "warning", message: "w é😀", page: 3 },
          { source: "pdfminer.psparser", level: "error", message: "error", page: 3 },
        ],
        "onWarning",
      );
      assert(lib.diagnosticsPolicy({}).minLevel === 0, "silent by default");
      assert(lib.diagnosticsPolicy({ logLevel: "debug", onWarning() {} }).minLevel === 10, "debug");
    },
  },
  {
    name: "predefined CMaps load lazily, once, and match pdfminer",
    only: "direct",
    async run({ lib, oracle, readCMap }) {
      const loads = [];
      const read = async (name) => {
        loads.push(name);
        return readCMap(name);
      };
      lib.configure({ cmaps: { load: read } });
      try {
        const t = await lib.internals();
        for (const { in: input, out } of await oracle("cmaps")) {
          const [what, ...args] = input.map((v) => fromTyped(v, lib));
          if (what === "decode") assertSame(await t.cmapDecode(args[0], args[1]), fromTyped(out, lib), `decode ${args[0]}`);
          else
            assertSame(await t.cmapUnichr(args[0], args[1], args[2]), fromTyped(out, lib), `unichr ${args[0]} ${args[1]}`);
        }
        assert(new Set(loads).size === loads.length, `each CMap loaded once: ${loads.join(",")}`);
        assert(loads.includes("UniJIS-UCS2-H") && loads.includes("to-unicode-Adobe-Japan1"), loads.join(","));
        // a failing loader rejects the call, and a later call can succeed
        lib.configure({
          cmaps: {
            load: async () => {
              throw new Error("no network");
            },
          },
        });
        const t2 = await lib.internals();
        await rejects(t2.cmapDecode("UniGB-UCS2-H", new Uint8Array([0, 0x41])), (e) => assert(/no network/.test(e.message), e.message), "loader failure");
        // without CMaps: pdfminer's CMapNotFound
        lib.configure({ cmaps: false });
        const t3 = await lib.internals();
        await rejects(
          t3.cmapDecode("UniGB-UCS2-H", new Uint8Array([0, 0x41])),
          (e) => assert(e.name === "CMapNotFound" && e.isInstance("pdfminer.cmapdb.CMapDB.CMapNotFound"), e.name),
          "cmaps: false",
        );
        // unknown names never reach the loader
        lib.configure({ cmaps: { load: read } });
        const t4 = await lib.internals();
        loads.length = 0;
        await rejects(t4.cmapDecode("No-Such-CMap", new Uint8Array([1])), (e) => assert(e.name === "CMapNotFound", e.name), "unknown");
        assert(loads.length === 0, "no load for an unknown name");
      } finally {
        lib.configure({});
      }
    },
  },
  {
    name: "a call that misses a CMap is re-run from scratch: Python's result, its diagnostics once",
    async run({ lib, oracle, readFile }) {
      const t = await lib.internals();
      const doc = await t.call("doc_open", null, 0, { data: await readFile("corpus/micro/text-operators.pdf") });
      const handles = (await t.info()).handles;
      let n = 0;
      for (const { in: input, out } of await oracle("cmaps")) {
        const [what, name, code] = input.map((v) => fromTyped(v, lib));
        if (what !== "decode" || n >= 60) continue;
        // the document is an argument: it caches the CMap, as a font does
        const [cids, derived] = await t.call("doc_cmap_decode", [doc, name, code]);
        assertSame(cids, fromTyped(out, lib), `decode ${name}`);
        assertSame(await t.call("echo", null, 0), null, "alive");
        await t.free(derived);
        n++;
      }
      assert((await t.info()).handles === handles, "no handle left by the void runs");
      // a request that warns, then misses a CMap: the warning once
      const got = [];
      const result = await t.call(
        "sequence",
        [
          ["warn", 0, [["pdfminer.pdfinterp", 30, "w"]]],
          ["doc_cmap_decode", 0, [doc, "UniKS-UCS2-H", new Uint8Array([0xac, 0x00])]],
        ],
        0,
        { minLevel: 30, onDiagnostics: (d, dropped) => got.push([d, dropped]) },
      );
      assertSame(result[1][0], [1086], "UniKS-UCS2-H");
      assertSame(got, [[[{ source: "pdfminer.pdfinterp", level: 30, message: "w" }], 0]], "diagnostics");
      await t.free(result[1][1]);
      await t.free(doc);
    },
  },
  {
    name: "the default CMap location serves every CMap",
    async run({ lib }) {
      const t = await lib.internals();
      assertSame(await t.cmapDecode("UniKS-UCS2-H", new Uint8Array([0xac, 0x00])), [1086], "UniKS-UCS2-H");
      assert(lib.CMAP_NAMES.length > 100, "names");
    },
  },
  {
    name: "text, words, search and tables over pdfplumber's objects match pdfplumber",
    async run({ lib, oracle }) {
      const t = await lib.internals();
      let checked = 0;
      for (const { in: input, out } of await oracle("text")) {
        const [path, , charsT, edgesT, bboxT] = input;
        const chars = fromTyped(charsT, lib, true);
        const plainChars = fromTyped(charsT, lib);
        const edges = fromTyped(edgesT, lib, true);
        const bbox = fromTyped(bboxT, lib, true);
        let tm;
        for (const [op, argsT, resultT] of out.d[0][1]) {
          const args = fromTyped(argsT, lib, true);
          const expected = fromTyped(resultT, lib);
          const where = `${path} ${op} ${JSON.stringify(fromTyped(argsT, lib))}`;
          const run = async () => {
            switch (op) {
              case "extract_text":
                return t.extractText(chars, args);
              case "extract_words":
                return t.extractWords(chars, args);
              case "dedupe_chars": {
                const kept = await t.dedupeChars(chars, args[0], args[1]);
                return kept;
              }
              case "textmap":
                if (tm) await tm.free();
                tm = await t.textmap(chars, args);
                return tm.text;
              case "search": {
                const [, pattern, regex, kase, groups, rchars, main] = fromTyped(argsT, lib);
                return tm.search(pattern, { regex, case: kase, return_groups: groups, return_chars: rchars, main_group: main });
              }
              case "lines":
                return tm.lines(args[1], args[2]);
              case "find_tables":
                return t.findTables(chars, edges, bbox, args, { x_tolerance: 3, y_tolerance: 3 });
            }
            throw new Error(`unknown op ${op}`);
          };
          let actual;
          try {
            actual = await run();
          } catch (e) {
            actual = { exception: e.name, message: e.message };
          }
          if (op === "dedupe_chars" && Array.isArray(expected)) {
            assertSame(actual, expected.map((i) => plainChars[i]), where);
          } else {
            assertSame(actual, expected, where);
          }
          checked++;
        }
        if (tm) await tm.free();
        tm = undefined;
      }
      assert(checked > 200, `${checked} operations`);
    },
  },
  {
    name: "search reads main_group for each match, as m.group() does",
    async run({ lib, oracle }) {
      const t = await lib.internals();
      let checked = 0;
      for (const { in: input, out } of await oracle("search_groups")) {
        const [charsT, pattern, mainT] = input;
        const tm = await t.textmap(fromTyped(charsT, lib, true), {});
        const main = fromTyped(mainT, lib, true);
        let actual;
        try {
          actual = await tm.search(pattern, { return_chars: false, main_group: main });
        } catch (e) {
          actual = { exception: e.name, message: e.message };
        }
        assertSame(actual, fromTyped(out, lib), `${pattern} main_group=${String(fromTyped(mainT, lib))}`);
        await tm.free();
        checked++;
      }
      assert(checked >= 80, `${checked} cases`);
    },
  },
  {
    name: "search patterns follow JavaScript RegExp semantics, spans converted to code points",
    async run({ lib }) {
      const t = await lib.internals();
      const chars = [..."a😀b ab"].map((text, i) => ({
        text,
        x0: new lib.PyFloat(i * 10),
        x1: new lib.PyFloat(i * 10 + 8),
        top: new lib.PyFloat(0),
        bottom: new lib.PyFloat(10),
        doctop: new lib.PyFloat(0),
        upright: true,
        height: new lib.PyFloat(10),
        width: new lib.PyFloat(8),
        size: new lib.PyFloat(10),
      }));
      const tm = await t.textmap(chars, {});
      assert(tm.text === "a😀b ab", tm.text);
      const r = await tm.search(/b/, { return_chars: false, return_groups: false });
      assertSame(r.map((m) => [m.text, m.x0]), [["b", 20], ["b", 50]], "code point offsets after an astral char");
      await rejects(tm.search(/\ud83d/), (e) => assert(e.name === "ValueError", e.name), "half of a pair");
      await rejects(tm.search(/a/, { regex: false }), (e) => assert(e.name === "ValueError", e.name), "RegExp with regex: false");
      assertSame((await tm.search("A", { case: false, return_chars: false })).length, 2, "case-insensitive string");
      assertSame((await tm.search(".", { regex: false })).length, 0, "escaped string");
      await tm.free();
    },
  },
  {
    name: "open() reads every input kind; page features are not implemented yet",
    async run({ lib, readFile, fileInput, missingInput }) {
      const rel = "corpus/micro/text-operators.pdf";
      const bytes = await readFile(rel);
      const inputs = [bytes, bytes.slice().buffer, new Blob([bytes]), fileInput(rel)];
      if (typeof fileInput(rel) === "string") inputs.push(new URL(fileInput(rel), globalThis.location?.href ?? "file:///"));
      for (const input of inputs) {
        const pdf = await lib.open(input);
        try {
          let thrown;
          try {
            void pdf.pages;
          } catch (e) {
            thrown = e;
          }
          assert(thrown instanceof lib.NotImplemented && thrown.name === "NotImplemented", "pages");
          await rejects(pdf.annots, (e) => assert(e instanceof lib.NotImplemented, e.name), "annots");
        } finally {
          await pdf.close();
          await pdf.close();
        }
      }
      await rejects(lib.open(missingInput()), (e) => assert(e instanceof Error && !(e instanceof lib.PDFPlumberError), e.name), "missing file");
      await rejects(lib.open(42), (e) => assert(e instanceof TypeError, e.name), "bad input");
    },
  },
  {
    name: "open() takes the options pdfplumber.open() takes, whatever their values",
    async run({ lib, oracle, readFile }) {
      let checked = 0;
      for (const { in: input, out } of await oracle("open")) {
        const [path, optionsT] = input;
        const options = fromTyped(optionsT, lib, true);
        const where = `open(${path}, ${JSON.stringify(fromTyped(optionsT, lib), (k, v) => (v instanceof Uint8Array ? [...v] : v))})`;
        assert(out === null, `${where}: Python raised; replayed once the page layer lands`);
        const pdf = await lib.open(await readFile(path), options);
        await pdf.close();
        checked++;
      }
      assert(checked >= 9, `${checked} cases`);
    },
  },
  {
    name: "a PDF is in the module's memory once",
    only: "direct",
    async run({ lib }) {
      const t = await lib.internals();
      const size = 32 * 2 ** 20;
      const before = t.core.memoryBytes();
      const pdf = await lib.open(pdfWithStream(size));
      const grown = t.core.memoryBytes() - before;
      await pdf.close();
      assert(grown < 1.5 * size, `memory grew by ${grown} bytes for ${size}`);
    },
  },
  {
    name: "a CJK document extracts like pdfplumber, its CMaps fetched once",
    only: "direct",
    async run({ lib, oracle, readFile, readCMap }) {
      let checked = 0;
      for (const { in: input, out } of await oracle("documents")) {
        const [rel, numberT] = input;
        const number = fromTyped(numberT, lib);
        const expected = fromTyped(out, lib);
        const loads = [];
        lib.configure({
          cmaps: {
            load: async (name) => {
              loads.push(name);
              return readCMap(name);
            },
          },
        });
        const t = await lib.internals();
        // pdfplumber.open, pdf.pages, page.extract_text(): the first call
        // that needs a CMap is void, the core rebuilds the document and the
        // call runs again
        const doc = await t.call("doc_open", {}, 0, { data: await readFile(rel) });
        const pages = await t.call("doc_pages", null, doc);
        const text = await t.call("page_extract_text", [{}], pages[number - 1].handle);
        assertSame(text, expected.text, rel);
        const want = [...expected.cmaps, ...expected.umaps.map((u) => `to-unicode-${u}`)].sort();
        assertSame([...loads].sort(), want, `${rel}: the CMaps fetched`);
        await t.free(doc);
        checked++;
      }
      assert(checked >= 2, `${checked} documents`);
    },
  },
  {
    name: "page operations through the boundary match pdfplumber (sessions of a Python script)",
    async run({ lib, oracle, readFile }) {
      let checked = 0;
      for (const { in: input, out } of await oracle("pages")) {
        checked += await replaySession(lib, input, out, readFile);
      }
      assert(checked > 250, `${checked} operations`);
    },
  },
  {
    name: "no buffer of the boundary leaks",
    only: "direct",
    async run({ lib }) {
      const t = await lib.internals();
      await t.testValues();
      await rejects(t.raise("ValueError", "x"), () => {}, "raise");
      assert(t.core.buffers() === 0, `${t.core.buffers()} live buffers`);
    },
  },
];
