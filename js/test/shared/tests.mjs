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
      // a str argument with a lone surrogate: refused by the core (its str
      // cannot hold one yet)
      await rejects(t.echo("\ud800"), (e) => assert(e.name === "UnicodeEncodeError", e.name), "lone surrogate argument");
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
      await rejects(lib.open(bytes, { pages: "1" }), (e) => assert(e instanceof TypeError, e.name), "bad option");
    },
  },
  {
    name: "a CJK document extracts like pdfplumber",
    todo: "needs the page layer (pdf.pages)",
    async run() {},
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
