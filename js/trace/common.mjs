// What the plumber_* stages share, through the public API: the helpers of
// oracle/stages/plumber_objects.py (open_pdf, exc, select, digest, Names,
// Objects, boxes, annot_value) and plumber_text.py (out_line, derive_bbox,
// derive, PREDICATES), on typed values (js/src/core/typed.ts) read from
// the core after each call of the API (`core.lastTyped`).
import { readFileSync } from "node:fs";

import { Named, canon, canonException, digest, sha256Hex } from "./canon.mjs";
import { ExcValue, fromTypedJson } from "./typed-json.mjs";

/** The package and its internal API (js/trace/pdftrace-js.mjs fills it). */
export const lib = {};

const OPS_FILE = new URL("./plumber-ops.json", import.meta.url);
let ops;

/** The option tables of the stages (oracle/codegen/plumber_ops.py), as
 * typed values. */
export function tables() {
  if (!ops) {
    const raw = JSON.parse(readFileSync(OPS_FILE, "utf8"));
    ops = { ...raw.constants };
    for (const [name, value] of Object.entries(raw.tables)) ops[name] = fromTypedJson(value, lib);
  }
  return ops;
}

/** A harness error, never written in a trace (canon.py `CanonError`). */
export class HarnessError extends Error {}

/** Whether an error is a Python exception of the core (or of the API's
 * Python semantics: search patterns), which a stage writes. */
export function isPython(e) {
  return e instanceof lib.PDFPlumberError || e instanceof ExcValue;
}

/** Rethrows what is not a Python exception (a harness error). */
export function python(e) {
  if (!isPython(e)) throw e;
  return e;
}

/** `exc(e)`: `canon(e)`, then ` cause <canon>` for each exception a
 * `PdfminerException` or `MalformedPDFException` wraps (`error.cause`). */
export function exc(e) {
  python(e);
  const parts = [canonException(e)];
  while ((e.name === "PdfminerException" || e.name === "MalformedPDFException") && e.cause instanceof Error) {
    e = e.cause;
    parts.push("cause " + canonException(e));
  }
  return parts.join(" ");
}

/** `select(sha256, index, variants, k)` */
export function select(sha, index, variants, k) {
  if (!variants.length) return [];
  const start = (Number.parseInt(sha.slice(0, 8), 16) + index * k) % variants.length;
  const out = [];
  for (let j = 0; j < Math.min(k, variants.length); j++) out.push(variants[(start + j) % variants.length]);
  return out;
}

/** `out_line(i, value)` / `bounded(label, i, value)`: `label i <canon>`, or
 * `labeld i <length> <digest>` beyond OUT_LIMIT characters. */
export function bounded(label, i, value) {
  const text = canon(value);
  if (text.length > tables().OUT_LIMIT) return `${label}d ${i} ${text.length} ${sha256Hex(text).slice(0, 16)}`;
  return `${label} ${i} ${text}`;
}

/** The typed value of the last result of the document's core. */
export function typed(pdf) {
  return pdf.core.lastTyped;
}

/** A typed dict's items as the options object of an API call (its values
 * stay typed: the core receives Python's types). */
export function kwargs(d) {
  const o = {};
  if (d === null || d === undefined) return o;
  for (const [k, v] of d.entries) Object.defineProperty(o, k, { value: v, enumerable: true, writable: true, configurable: true });
  return o;
}

/** `open_pdf(path, password, max_pages, **options)`: `pdfplumber.open(path,
 * pages=PAGES, password=password, **options)`, by path (Node's `open(path)`
 * opens as pdfplumber.open(path)). `extra`: options of the API (hooks). */
export async function openPdf(path, password, maxPages, options = {}, extra = {}) {
  const pages = maxPages ? Array.from({ length: maxPages }, (_, i) => i + 1) : null;
  return lib.open(path, { pages, password, ...options, ...extra });
}

/** Python's type name of a typed value (messages the stages raise). */
export function typeName(v) {
  if (v === null || v === undefined) return "NoneType";
  if (typeof v === "boolean") return "bool";
  if (typeof v === "number" || typeof v === "bigint") return "int";
  if (typeof v === "string") return "str";
  if (v instanceof Uint8Array) return "bytes";
  if (Array.isArray(v)) return "list";
  const name = v.constructor?.name;
  return { PyFloat: "float", PyTuple: "tuple", PyDict: "dict" }[name] ?? name;
}

/** The port's name of an inline image (`inline-<k>`). */
const INLINE = /^inline-(0|[1-9][0-9]*)$/;

/** `Names`: an inline image name written `inline{"n": k}`, a `Page` (a
 * `PageRef`) `Page{"page_number": n}` (canon.mjs). */
export class Names {
  constructor() {
    this.inline = [];
  }

  /** `learn(objects)`: the inline names of the images, in order (an
   * inline image's stream has no object id). */
  learn(objects) {
    for (const [, objs] of objects.entries) {
      for (const o of objs) {
        if (o.get("object_type") !== "image" || !o.has("stream")) continue;
        const stream = o.get("stream");
        const name = o.get("name");
        const inlineStream = stream instanceof lib.PDFStream && (stream.objid === null || stream.objid === undefined);
        if (inlineStream && typeof name === "string" && INLINE.test(name) && !this.inline.includes(name)) this.inline.push(name);
      }
    }
  }

  value(v) {
    if (typeof v === "string") {
      const n = this.inline.indexOf(v);
      if (n >= 0) return new Named("inline", [["n", n]]);
    }
    return v;
  }

  /** `obj(o)`: the dict with its `name` and `P` written as above. */
  obj(o) {
    return new lib.PyDict(o.entries.map(([k, v]) => [k, k === "name" || k === "P" ? this.value(v) : v]));
  }

  /** `text(s)`: each inline name replaced by `<inline k>` (the longer
   * names first: the port's are prefixes of one another). */
  text(s) {
    const order = this.inline.map((_, k) => k).sort((a, b) => this.inline[b].length - this.inline[a].length);
    for (const k of order) s = s.split(this.inline[k]).join(`<inline ${k}>`);
    return s;
  }
}

/** `annot_value(a, names)`: an annotation as written, its `data` dict
 * holding the `Page` in place of `/P`. */
export function annotValue(a, names) {
  const data = a.get("data");
  if (!(data instanceof lib.PyDict)) throw new ExcValue("AttributeError", `'${typeName(data)}' object has no attribute 'items'`);
  return names.obj(new lib.PyDict(a.entries.map(([k, v]) => [k, k === "data" ? names.obj(data) : v])));
}

/** `Objects` of plumber_objects.py: the `kinds`/`keys`/`obj` (and
 * `lchars`) lines of a page's objects. */
export class ObjectsWriter {
  constructor(names) {
    this.names = names;
    this.keys = new Map();
    /** canonical text of a char (keys and values) -> its first index */
    this.chars = new Map();
  }

  write(lines, objects, refer) {
    this.names.learn(objects);
    lines.push("kinds " + canon(objects.keys()));
    for (const [kind, objs] of objects.entries) {
      const refs = [];
      for (let i = 0; i < objs.length; i++) {
        const o = this.names.obj(objs[i]);
        const keys = o.keys();
        const values = canon(new lib.PyTuple(o.entries.map(([, v]) => v)));
        if (kind === "char") {
          const text = canon(keys) + " " + values;
          if (refer) {
            const j = refer.get(text);
            refs.push(j === undefined ? null : j);
            if (j !== undefined) continue;
          } else if (!this.chars.has(text)) this.chars.set(text, i);
        }
        const known = this.keys.get(kind);
        if (!known || known.length !== keys.length || known.some((k, n) => k !== keys[n])) {
          this.keys.set(kind, keys);
          lines.push("keys " + canon(kind) + " " + canon(keys));
        }
        lines.push("obj " + canon(kind) + " " + i + " " + values);
      }
      if (refer && kind === "char") lines.push("lchars " + canon(refs));
    }
  }
}

export const BOXES = ["rotation", "mediabox", "cropbox", "trimbox", "bleedbox", "artbox", "bbox", "initial_doctop", "width", "height"];

/** The typed geometry of a page (its `doc_pages` or `page_info` value). */
export function geometry(page) {
  const g = page[lib.TYPED];
  if (!(g instanceof lib.PyDict)) throw new HarnessError("a page without its typed geometry (is the core in typed mode?)");
  return g;
}

/** `boxes(page)` */
export function boxLines(lines, page) {
  const g = geometry(page);
  for (const name of BOXES) if (g.has(name)) lines.push("box " + name + " " + canon(g.get(name)));
}

/** Python's `+`, `-`, `*` on typed numbers: two ints stay an int, else a
 * float (IEEE, as CPython's). */
export function pyArith(op, a, b) {
  const isFloat = (x) => x instanceof lib.PyFloat;
  const num = (x) => (isFloat(x) ? x.value : x);
  if (!isFloat(a) && !isFloat(b)) {
    if (typeof a !== typeof b || typeof a === "bigint") {
      const x = BigInt(a);
      const y = BigInt(b);
      const r = op === "+" ? x + y : op === "-" ? x - y : x * y;
      return r >= -(2n ** 53n) && r <= 2n ** 53n ? Number(r) : r;
    }
    const r = op === "+" ? a + b : op === "-" ? a - b : a * b;
    if (Number.isSafeInteger(r)) return r;
    return pyArith(op, BigInt(a), BigInt(b));
  }
  const x = Number(num(a));
  const y = Number(num(b));
  return new lib.PyFloat(op === "+" ? x + y : op === "-" ? x - y : x * y);
}

/** `derive_bbox(page, fractions, relative)` of plumber_text.py. */
export function deriveBbox(page, fractions, relative) {
  const [x0, top, x1, bottom] = items(geometry(page).get("bbox"));
  const w = pyArith("-", x1, x0);
  const h = pyArith("-", bottom, top);
  const [f0, f1, f2, f3] = items(fractions);
  if (truthy(relative)) return new lib.PyTuple([pyArith("*", w, f0), pyArith("*", h, f1), pyArith("*", w, f2), pyArith("*", h, f3)]);
  return new lib.PyTuple([
    pyArith("+", x0, pyArith("*", w, f0)),
    pyArith("+", top, pyArith("*", h, f1)),
    pyArith("+", x0, pyArith("*", w, f2)),
    pyArith("+", top, pyArith("*", h, f3)),
  ]);
}

/** The items of a typed sequence. */
export function items(v) {
  if (Array.isArray(v)) return v;
  if (v instanceof lib.PyTuple) return v.items;
  throw new HarnessError(`not a sequence: ${typeName(v)}`);
}

/** Python's truth of a typed value. */
export function truthy(v) {
  if (v === null || v === undefined || v === false) return false;
  if (typeof v === "number") return v !== 0;
  if (typeof v === "bigint") return v !== 0n;
  if (typeof v === "string") return v.length > 0;
  if (v instanceof lib.PyFloat) return v.value !== 0;
  if (Array.isArray(v)) return v.length > 0;
  if (v instanceof lib.PyTuple) return v.items.length > 0;
  if (v instanceof lib.PyDict) return v.entries.length > 0;
  return true;
}

/** The predicates of `filter` steps (plumber_text.py PREDICATES), on the
 * public objects the API gives the test function. */
export const PREDICATES = {
  // obj.get("upright", True)
  upright: (o) => ("upright" in o ? o.upright : true),
  // obj["object_type"] != "rect"
  "not-rect": (o) => o.object_type !== "rect",
  // obj.get("size", 0) < 10
  small: (o) => ("size" in o ? o.size : 0) < 10,
};

/** `derive(page, steps)` of plumber_text.py, through the API. */
export async function derive(page, steps) {
  for (const step of steps) {
    const name = step[0];
    if (name === "crop" || name === "within_bbox" || name === "outside_bbox") {
      const kw = kwargs(step[2]);
      const bbox = deriveBbox(page, step[1], step[2].get("relative"));
      const method = { crop: "crop", within_bbox: "withinBbox", outside_bbox: "outsideBbox" }[name];
      page = await page[method](bbox, kw);
    } else if (name === "filter") {
      page = await page.filter(PREDICATES[step[1]]);
    } else if (name === "dedupe_chars") {
      page = await page.dedupeChars(kwargs(step[1]));
    } else throw new HarnessError(`unknown step ${name}`);
  }
  return page;
}

/** The page's index of each object of a derived page, by identity
 * (`oracle_object_indices`, crates/wasm/src/oracle.rs). */
export async function objectIndices(derived, parent, kind) {
  await derived.pdf.job((call) => call("oracle_object_indices", derived.handle, [parent.handle, kind]));
  return typed(derived.pdf);
}

/** The ids line value of a page (`oracle_page_ids`). */
export async function pageIds(page) {
  await page.pdf.job((call) => call("oracle_page_ids", page.handle, null));
  return typed(page.pdf);
}

/** `file_sha256(path)` */
export function fileSha256(path) {
  return sha256Hex(readFileSync(path));
}

export { canon, digest, sha256Hex, Named, ExcValue };
