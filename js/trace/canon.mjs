// The canonical text of a typed value (oracle/canon.py; docs/verification.md,
// "Canonical Trace Format"): the JavaScript half of the trace format, for the
// harness that writes stage traces through the public API (js/trace/).
//
// Typed values (js/src/core/typed.ts): an `int` is a number (a bigint
// beyond ±2^53), a `float` a `PyFloat`, a `tuple` a `PyTuple`, a `dict` a
// `PyDict`, a `list` an array, `str` a string (UTF-16: a Python `str`
// holding a high then a low surrogate is the astral character they code,
// which `json.dumps(ensure_ascii=True)` writes alike), `bytes` a
// `Uint8Array`; pdfminer objects and `PageRef` as the public API gives
// them; an exception is a `PDFPlumberError` (or any `Error`); `Named` as
// in canon.py. Iterative: any depth.
import { createHash } from "node:crypto";

/** canon.py `Named`: written `name{fields}`. */
export class Named {
  /** @param {string} name @param {[unknown, unknown][]} fields */
  constructor(name, fields) {
    this.name = name;
    this.fields = fields;
  }
}

export const BYTES_INLINE_MAX = 256;

export function sha256Hex(data) {
  return createHash("sha256").update(data).digest("hex");
}

/** `json.dumps(s, ensure_ascii=True)` */
export function jsonString(s) {
  let out = '"';
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i);
    if (c === 0x22) out += '\\"';
    else if (c === 0x5c) out += "\\\\";
    else if (c === 0x0a) out += "\\n";
    else if (c === 0x0d) out += "\\r";
    else if (c === 0x09) out += "\\t";
    else if (c === 0x08) out += "\\b";
    else if (c === 0x0c) out += "\\f";
    else if (c < 0x20 || c >= 0x7f) out += "\\u" + c.toString(16).padStart(4, "0");
    else out += s[i];
  }
  return out + '"';
}

/** `repr(float)` (CPython's short repr: the shortest digits, fixed
 * notation for a decimal point position in (-4, 16], else exponent). */
export function floatRepr(x) {
  if (Number.isNaN(x)) return "nan";
  if (x === Infinity) return "inf";
  if (x === -Infinity) return "-inf";
  if (x === 0) return Object.is(x, -0) ? "-0.0" : "0.0";
  const sign = x < 0 ? "-" : "";
  const [mant, exp] = Math.abs(x).toExponential().split("e");
  const digits = mant.replace(".", "");
  const decpt = Number(exp) + 1;
  if (decpt > -4 && decpt <= 16) {
    if (decpt <= 0) return `${sign}0.${"0".repeat(-decpt)}${digits}`;
    if (decpt >= digits.length) return `${sign}${digits}${"0".repeat(decpt - digits.length)}.0`;
    return `${sign}${digits.slice(0, decpt)}.${digits.slice(decpt)}`;
  }
  const e = decpt - 1;
  const m = digits.length > 1 ? `${digits[0]}.${digits.slice(1)}` : digits;
  return `${sign}${m}e${e < 0 ? "-" : "+"}${String(Math.abs(e)).padStart(2, "0")}`;
}

function bytesText(b) {
  if (b.length <= BYTES_INLINE_MAX) return `x"${Buffer.from(b.buffer, b.byteOffset, b.length).toString("hex")}"`;
  return `x#${b.length}:${sha256Hex(b).slice(0, 16)}`;
}

const SHORT = new Set(["builtins", "pdfminer", "pdfplumber", "pycompat", "oracle"]);

/** `exception_name(e)` of canon.py for an error of the API: the class name
 * for builtins, pdfminer, pdfplumber (and the port's `NotPorted`), else
 * `module.name`. */
export function exceptionName(e) {
  // an exception known by its trace name only (typed JSON's `ExcValue`)
  if (!e.pythonModule) return e.name;
  const module = e.pythonModule;
  const qual = e.pythonQualname ?? e.name;
  const name = qual.slice(qual.lastIndexOf(".") + 1);
  if (SHORT.has(module.split(".")[0])) return name;
  return `${module}.${name}`;
}

/** `canon(e)`: `!Name("message")`, addresses normalized. */
export function canonException(e) {
  const message = String(e.message).replace(/ at 0x[0-9a-fA-F]+>/g, " at 0x?>");
  return `!${exceptionName(e)}(${jsonString(message)})`;
}

const isClass = (v, name) => v !== null && typeof v === "object" && v.constructor && v.constructor.name === name;

/** The text of a leaf, or undefined for a container. */
function scalar(v) {
  if (v === null || v === undefined) return "None";
  switch (typeof v) {
    case "boolean":
      return v ? "True" : "False";
    case "number":
      if (!Number.isInteger(v)) throw new Error(`canon: a number that is not an int: ${v} (a float must be a PyFloat)`);
      return Object.is(v, -0) ? "0" : String(v);
    case "bigint":
      return v.toString();
    case "string":
      return jsonString(v);
  }
  if (v instanceof Uint8Array) return bytesText(v);
  if (isClass(v, "PyFloat")) return floatRepr(v.value);
  if (isClass(v, "PSLiteral")) return "/" + (typeof v.name === "string" ? jsonString(v.name) : bytesText(v.name));
  if (isClass(v, "PSKeyword")) return "K" + bytesText(v.name);
  if (v instanceof Error) return canonException(v);
  return undefined;
}

const SEP = ", ";

/** `canon(value)` */
export function canon(value) {
  const leaf = scalar(value);
  if (leaf !== undefined) return leaf;
  const out = [];
  // work items: { v } to write, { s } to emit, { x } to leave a container
  const stack = [{ v: value }];
  const active = new Set();
  const pushSeq = (open, items, close) => {
    stack.push({ s: close });
    for (let i = items.length - 1; i >= 0; i--) {
      stack.push({ v: items[i] });
      if (i) stack.push({ s: SEP });
    }
    stack.push({ s: open });
  };
  const pushMap = (pairs) => {
    stack.push({ s: "}" });
    for (let i = pairs.length - 1; i >= 0; i--) {
      const [k, x] = pairs[i];
      stack.push({ v: x });
      stack.push({ s: ": " });
      stack.push({ v: k });
      if (i) stack.push({ s: SEP });
    }
    stack.push({ s: "{" });
  };
  while (stack.length) {
    const item = stack.pop();
    if ("s" in item) {
      out.push(item.s);
      continue;
    }
    if ("x" in item) {
      active.delete(item.x);
      continue;
    }
    const v = item.v;
    const l = scalar(v);
    if (l !== undefined) {
      out.push(l);
      continue;
    }
    if (active.has(v)) throw new Error("canon: a cyclic value");
    active.add(v);
    stack.push({ x: v });
    if (Array.isArray(v)) pushSeq("[", v, "]");
    else if (isClass(v, "PyTuple")) pushSeq("(", v.items, ")");
    else if (isClass(v, "PyDict")) pushMap(v.entries);
    else if (v instanceof Named) {
      out.push(v.name);
      pushMap(v.fields);
    } else if (isClass(v, "PageRef")) {
      // `Page{"page_number": n}` (plumber_objects.py Names)
      out.push("Page");
      pushMap([["page_number", v.page_number]]);
    } else if (isClass(v, "PDFObjRef")) {
      out.push("R");
      stack.push({ v: v.objid });
    } else if (isClass(v, "PDFStream")) pushSeq("Stream(", [v.objid, v.genno, v.attrs, v.rawdata], ")");
    else throw new Error(`canon: unsupported value ${Object.prototype.toString.call(v)} ${v?.constructor?.name}`);
  }
  return out.join("");
}

/** `digest(value)`: the first 16 hex digits of the SHA-256 of the canon. */
export function digest(value) {
  return sha256Hex(canon(value)).slice(0, 16);
}
