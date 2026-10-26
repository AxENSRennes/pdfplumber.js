// The replayer of the sessions of the public API (oracle/unit/js_api.py,
// sessions.jsonl): each session's calls made through the PUBLIC API only
// (open(), the documents, pages, derived pages and tables it gives), in a
// new core (a new Python process), the calls of a step made without
// waiting and awaited in the session's order, each result compared with
// what Python gives for the statements the API runs. Runtime-agnostic:
// Node (vitest), the browsers (Playwright, in the page and in worker mode)
// and the dist build run it.
//
// env = {
//   readFile(rel),   the bytes of a file of PDFPLUMBER_ROOT
//   fileInput(rel),  the file as a path (Node; the node_only sessions)
// }
import { difference, fromTyped } from "./typed.mjs";

/** The predicates of `filter` (oracle/unit/js_pages_run.py PREDICATES),
 * on the objects the API gives. */
export const PREDICATES = {
  // obj.get("upright", True)
  upright: (o) => ("upright" in o ? o.upright : true),
  // obj["object_type"] != "rect"
  "not-rect": (o) => o.object_type !== "rect",
  // obj.get("size", 0) < 10
  small: (o) => ("size" in o ? o.size : 0) < 10,
  // obj.get("text", ""): a value whose truth is a string's, alike in both
  text: (o) => ("text" in o ? o.text : ""),
  // raise ValueError("boom"): the error rejects filter()
  raises: () => {
    const e = new Error("boom");
    e.name = "ValueError";
    throw e;
  },
};

/** The functions given to a structure tree's `findAll`/`find`
 * (oracle/unit/js_pages_run.py ELEMENT_PREDICATES), on the API's
 * elements. */
export const ELEMENT_PREDICATES = {
  // el.type.startswith("H")
  "starts-with-H": (el) => el.type.startsWith("H"),
  // bool(el.mcids)
  "has-mcids": (el) => el.mcids.length > 0,
};

/** The fields of a structure element, in the dataclass's order. */
const ELEMENT_FIELDS = ["type", "revision", "id", "lang", "alt_text", "actual_text", "title", "page_number", "attributes", "mcids"];

const UNRESOLVED = { $named: "Unresolved" };

function setOwn(obj, key, value) {
  Object.defineProperty(obj, key, { value, writable: true, enumerable: true, configurable: true });
}

/** A value made by `expand` without recursion (the values of a depth
 * window are nested deeper than the JavaScript stack): `expand(x)` gives
 * `[value]` for a leaf, or `[container, [[key, child], ...]]`, the
 * children expanded in turn (`Leaf` wraps a child that is already a
 * value). Keys keep their order. */
class Leaf {
  constructor(value) {
    this.value = value;
  }
}

function build(root, expand) {
  const out = [undefined];
  const put = (holder, key, value) => (Array.isArray(holder) ? (holder[key] = value) : setOwn(holder, key, value));
  const stack = [[root, out, 0]];
  while (stack.length) {
    const [x, holder, key] = stack.pop();
    const r = x instanceof Leaf ? [x.value] : expand(x);
    put(holder, key, r[0]);
    if (r.length === 1) continue;
    const [container, entries] = r;
    for (const [k] of entries) put(container, k, null);
    for (let i = entries.length - 1; i >= 0; i--) stack.push([entries[i][1], container, entries[i][0]]);
  }
  return out[0];
}

/** An expected value (typed JSON, output mapping): a Named value is
 * `{ $named, ...fields }`. */
function expected(root, lib) {
  return build(root, (j) => {
    if (j !== null && typeof j === "object" && !Array.isArray(j) && "named" in j && "d" in j && Object.keys(j).length === 2)
      return [{ $named: j.named }, j.d.map(([k, v]) => [fromTyped(k, lib), v])];
    if (Array.isArray(j)) return [new Array(j.length), j.map((x, i) => [i, x])];
    if (j !== null && typeof j === "object" && Object.keys(j).length === 1) {
      const [k] = Object.keys(j);
      if (k === "t") return [new Array(j.t.length), j.t.map((x, i) => [i, x])];
      if (k === "d") return [{}, j.d.map(([kk, vv]) => [fromTyped(kk, lib), vv])];
    }
    return [fromTyped(j, lib)];
  });
}

const GEOMETRY = ["page_number", "initial_doctop", "rotation", "mediabox", "cropbox", "trimbox", "bleedbox", "artbox", "bbox"];

/** An error as a value: `Error{name, message, cause}`. */
function errorValue(e) {
  return {
    $named: "Error",
    name: e?.name ?? "Error",
    message: e?.message ?? String(e),
    cause: e?.cause instanceof Error ? errorValue(e.cause) : null,
  };
}

/** `v` with the name of each inline image written `"<inline k>"`, `k`
 * numbering the distinct names of a page in the order they appear
 * (oracle/unit/js_api.py `inline_names`: the port names an inline image
 * `inline-<k>`, pdfminer `str(id(stream))`). */
function inlineNames(root, lib) {
  const seen = new Map();
  const isInline = (o) =>
    o.object_type === "image" &&
    typeof o.name === "string" &&
    (o.stream instanceof lib.PDFStream || o.stream?.$named === "PDFStream") &&
    o.stream.objid === null &&
    /^inline-(0|[1-9][0-9]*)$/.test(o.name);
  return build(root, (x) => {
    if (Array.isArray(x)) return [new Array(x.length), x.map((v, i) => [i, v])];
    if (x === null || typeof x !== "object" || Object.getPrototypeOf(x) !== Object.prototype) return [x];
    const inline = isInline(x);
    let names;
    if (inline) {
      if (!seen.has(x.page_number)) seen.set(x.page_number, new Map());
      names = seen.get(x.page_number);
      if (!names.has(x.name)) names.set(x.name, names.size);
    }
    return [{}, Object.keys(x).map((k) => [k, inline && k === "name" ? new Leaf(`<inline ${names.get(x.name)}>`) : x[k]])];
  });
}

/** A result of the API as a session writes it. */
export function neutral(v, lib) {
  return inlineNames(neutralValue(v, lib), lib);
}

function neutralValue(root, lib) {
  return build(root, (v) => {
    if (v === undefined) return [null];
    if (v === null || typeof v !== "object") return [v];
    if (v instanceof Error) return [errorValue(v)];
    if (Array.isArray(v)) return [new Array(v.length), v.map((x, i) => [i, x])];
    if (v instanceof lib.PDFPlumberPage) {
      const entries = GEOMETRY.filter((k) => k in v && v[k] !== undefined).map((k) => [k, v[k]]);
      for (const k of ["width", "height"]) {
        try {
          entries.push([k, new Leaf(v[k])]);
        } catch (e) {
          entries.push([k, new Leaf(errorValue(e))]);
        }
      }
      return [{ $named: "PDFPlumberPage" }, entries];
    }
    if (v instanceof lib.Table) {
      return [{ $named: "Table", bbox: v.bbox, cells: v.cells, rows: v.rows, columns: v.columns }];
    }
    if (v instanceof lib.PDFPlumberDocument) return [{ $named: "PDFPlumberDocument" }];
    if (v instanceof lib.PDFStructTree) {
      return [{ $named: "PDFStructTree" }, [["page", v.page], ["children", v.children]]];
    }
    if (v instanceof lib.PDFStructElement) {
      return [{ $named: "PDFStructElement" }, [...ELEMENT_FIELDS.map((k) => [k, v[k]]), ["children", v.children]]];
    }
    if (v instanceof lib.PDFStream) {
      // a stream by reference: its data is read on demand
      return [{ $named: "PDFStream", objid: v.objid, genno: v.genno }, [["attrs", v.attrs]]];
    }
    if (v instanceof lib.PageRef) return [{ $named: "Page", page_number: v.page_number }];
    if (Object.getPrototypeOf(v) === Object.prototype) return [{}, Object.keys(v).map((k) => [k, v[k]])];
    return [v];
  });
}

/** Drops the messages the session leaves open (`message: null`). */
function anyMessage(actual, want) {
  if (!actual || !want || actual.$named !== "Error" || want.$named !== "Error") return actual;
  return { ...actual, message: want.message === null ? null : actual.message, cause: anyMessage(actual.cause, want.cause) };
}

/** A value an argument names that does not exist: the call is not made. */
class Missing extends Error {}

/** An argument (typed JSON, input mapping: floats cross as floats,
 * tuples as tuples), with the JavaScript forms of oracle/unit/js_api.py
 * (`JSForm`): `$num` a plain number, `$bigint`, `$map` a `Map`,
 * `$undefined`, `$ref` a value of an earlier result (`resolve`), `$cyclic`
 * an array holding itself, `$function`, `$set`, `$date`, `$float64array`,
 * `$instance` (objects the encoder refuses), `$warnings` an `onWarning`
 * collector (`warnings`); a search pattern `{regexp, flags}` is a
 * `RegExp`. */
function argument(j, lib, call, resolve, warnings) {
  const go = (x) => {
    if (Array.isArray(x)) return x.map(go);
    if (x !== null && typeof x === "object") {
      const keys = Object.keys(x);
      if (keys.length === 1) {
        const [k] = keys;
        switch (k) {
          case "$num":
            return fromTyped(x.$num, lib, false);
          case "$bigint":
            return BigInt(x.$bigint);
          case "$map":
            return new Map(x.$map.map(([kk, vv]) => [kk, go(vv)]));
          case "$undefined":
            return undefined;
          case "$ref": {
            const v = resolve(x.$ref);
            if (v === UNRESOLVED) throw new Missing();
            return v;
          }
          case "$cyclic": {
            const a = [];
            a.push(a);
            return a;
          }
          case "$function":
            return () => 1;
          // objects that are neither plain objects nor Maps (TypeError)
          case "$set":
            return new Set(["fontname"]);
          case "$date":
            return new Date(0);
          case "$float64array":
            return new Float64Array([0, 0, 100, 100]);
          case "$instance":
            return new (class Options {
              x_tolerance = 1;
            })();
          case "$warnings":
            return (w) => warnings.push(w);
          case "t":
            return new lib.PyTuple(x.t.map(go));
          case "d": {
            const o = {};
            for (const [kk, vv] of x.d) setOwn(o, fromTyped(kk, lib, true), go(vv));
            return o;
          }
        }
      }
    }
    return fromTyped(x, lib, true);
  };
  const v = go(j);
  const patterns = ["search", "findAll", "find_all", "find"];
  if (patterns.includes(call.call) && v && typeof v === "object" && "regexp" in v && Object.keys(v).length === 2)
    return new RegExp(v.regexp, v.flags);
  return v;
}

/** The input of `open()` in the form the session names (`as`). */
function inputOf(bytes, as) {
  switch (as) {
    case undefined:
      return bytes;
    case "blob":
      return new Blob([bytes]);
    case "arraybuffer":
      return bytes.slice().buffer;
    case "dataview": {
      // a view at an offset of a larger buffer
      const buffer = new Uint8Array(bytes.length + 11);
      buffer.set(bytes, 7);
      return new DataView(buffer.buffer, 7, bytes.length);
    }
  }
  throw new Error(`unknown input form ${as}`);
}

/** Replays one session (a line of sessions.jsonl); returns the number of
 * calls checked. `only`: the runtime's sessions (node_only ones are
 * skipped where open(path) does not exist). */
export async function replayApiSession(lib, line, env) {
  const [session] = line.in;
  const want = new Map(line.out.map((e) => [e.id, e.value]));
  lib.dropCore();
  const results = new Map();
  const resolve = (ref) => {
    const r = results.get(ref[0]);
    if (!r || !r.ok) return UNRESOLVED;
    let v = r.value;
    for (const i of ref.slice(1)) {
      if (typeof i === "number") {
        if (!Array.isArray(v) || i >= v.length) return UNRESOLVED;
      } else if (v === null || typeof v !== "object" || Array.isArray(v) || !Object.hasOwn(v, i)) {
        return UNRESOLVED;
      }
      v = v[i];
    }
    return v;
  };
  const warnings = [];
  let checked = 0;
  for (const step of session.steps) {
    // the inputs of open() are read first: the calls of a step are made
    // without waiting, in order
    const inputs = new Map();
    for (const call of step.calls) {
      if (call.call !== "open") continue;
      const inp = call.input;
      if ("bytes" in inp) inputs.set(call.id, Uint8Array.from(inp.bytes.match(/../g) ?? [], (h) => parseInt(h, 16)));
      else if ("bytes_of" in inp) inputs.set(call.id, inputOf(await env.readFile(inp.bytes_of), inp.as));
      else if (inp.as === "file-url") inputs.set(call.id, env.fileUrl(inp.path));
      else inputs.set(call.id, env.fileInput(inp.path));
    }
    const pending = step.calls.map((call) => {
      const target = call.on === null ? lib : resolve(call.on);
      if (target === UNRESOLVED) return Promise.resolve({ unresolved: true });
      let args;
      try {
        args = call.args.map((a) => argument(a, lib, call, resolve, warnings));
      } catch (e) {
        if (e instanceof Missing) return Promise.resolve({ unresolved: true });
        throw e;
      }
      let p;
      try {
        if ("get" in call) p = target[call.get];
        else if (call.call === "open") p = lib.open(inputs.get(call.id), ...args);
        else if (call.call === "filter" && call.predicate !== undefined) p = target.filter(PREDICATES[call.predicate]);
        else if (call.call === "asyncDispose") p = target[Symbol.asyncDispose]();
        else if (call.element_predicate) p = target[call.call](ELEMENT_PREDICATES[call.element_predicate]);
        else p = target[call.call](...args);
      } catch (e) {
        p = Promise.reject(e);
      }
      return Promise.resolve(p).then(
        (value) => ({ ok: true, value }),
        (error) => ({ ok: false, error }),
      );
    });
    for (const k of step.await) {
      const call = step.calls[k];
      const outcome = await pending[k];
      const name = call.get ?? call.call;
      const where = `${session.name} #${call.id} ${name}`;
      let actual;
      if (outcome.unresolved) {
        actual = UNRESOLVED;
        results.set(call.id, { ok: false });
      } else {
        results.set(call.id, outcome);
        actual = outcome.ok ? neutral(outcome.value, lib) : errorValue(outcome.error);
      }
      const expectedValue = expected(want.get(call.id), lib);
      const d = difference(anyMessage(actual, expectedValue), expectedValue);
      if (d) throw new Error(`${where}: ${d}`);
      checked++;
    }
  }
  lib.dropCore();
  return checked;
}
