/**
 * The value encodings of the boundary (docs/js-runtime.md, "Formats"):
 * decoding of PJB1 blocks and JSON results written by crates/wasm, and
 * encoding of PJB1 arguments. Mirrors crates/wasm/src/wire/{binary,json}.rs.
 */

import { PageRef, PDFObjRef, PDFStream, PSKeyword, PSLiteral, PyFloat, PyTuple } from "../pdfobjects.js";
import { PyDict } from "./typed.js";

export const TAG = {
  NONE: 0x00,
  FALSE: 0x01,
  TRUE: 0x02,
  I32: 0x03,
  INT_F64: 0x04,
  BIGINT: 0x05,
  F64: 0x06,
  STR: 0x07,
  BYTES: 0x08,
  LIST: 0x09,
  TUPLE: 0x0a,
  DICT: 0x0b,
  LITERAL: 0x0c,
  KEYWORD: 0x0d,
  REF: 0x0e,
  STREAM: 0x0f,
  TABLE: 0x10,
  INLINE_STR: 0x11,
  PAGE: 0x12,
  /** A `PDFStream` by reference: handle, objid, genno, attrs (`streamRefs`). */
  STREAM_REF: 0x13,
} as const;

export const COL = {
  GENERIC: 0,
  F64: 1,
  STR: 2,
  OPT_STR: 3,
  BOOL: 4,
  NONE: 5,
  NUM_SEQ: 6,
  OPT_F64: 7,
} as const;

const MAGIC = 0x31424a50; // "PJB1" read as a little-endian u32
const HEADER_LEN = 16;
const NO_STR = 0xffffffff;
const NO_SEQ = 0xff;
const LITTLE_ENDIAN = new Uint8Array(new Uint16Array([1]).buffer)[0] === 1;

const utf8 = new TextDecoder("utf-8", { ignoreBOM: true });
const latin1 = new TextDecoder("latin1");

export class WireError extends Error {
  constructor(message: string) {
    super(`malformed result: ${message}`);
    this.name = "WireError";
  }
}

/** Sets `obj[key] = value` as an own data property, `__proto__` included. */
function setOwn(obj: Record<string, unknown>, key: string, value: unknown): void {
  if (key === "__proto__") {
    Object.defineProperty(obj, key, { value, writable: true, enumerable: true, configurable: true });
  } else {
    obj[key] = value;
  }
}

/** A string from UTF-16 code units (lone surrogates kept). */
function fromUnits(units: Uint16Array): string {
  let s = "";
  for (let i = 0; i < units.length; i += 8192) {
    s += String.fromCharCode.apply(null, units.subarray(i, i + 8192) as unknown as number[]);
  }
  return s;
}

const enum K {
  LIST,
  DICT,
  LITERAL,
  REF,
  STREAM,
  PAGE,
  STREAM_REF,
}

interface Frame {
  k: K;
  arr?: unknown[];
  obj?: Record<string, unknown>;
  n: number;
  i: number;
  key?: string;
  /** Typed mode: the frame is a tuple (`LIST`), the items of a dict. */
  tuple?: boolean;
  entries?: [string, unknown][];
}

class Reader {
  readonly view: DataView;
  pos: number;
  strings: string[] = [];

  constructor(
    readonly u8: Uint8Array,
    readonly base: number,
    /** Typed values (core/typed.ts): `float` → `PyFloat`, `tuple` →
     * `PyTuple`, `dict` → `PyDict`; a block of the typed format has no
     * table. */
    readonly typed = false,
  ) {
    this.view = new DataView(u8.buffer, u8.byteOffset, u8.byteLength);
    this.pos = base;
  }

  check(n: number): void {
    if (this.pos + n > this.u8.length) throw new WireError("truncated");
  }

  varint(): number {
    let v = 0;
    let mul = 1;
    for (;;) {
      if (this.pos >= this.u8.length) throw new WireError("truncated varint");
      const b = this.u8[this.pos++];
      v += (b & 0x7f) * mul;
      if (b < 0x80) return v;
      mul *= 128;
      if (mul > 2 ** 49) throw new WireError("varint too long");
    }
  }

  u32(): number {
    this.check(4);
    const v = this.view.getUint32(this.pos, true);
    this.pos += 4;
    return v;
  }

  f64(): number {
    this.check(8);
    const v = this.view.getFloat64(this.pos, true);
    this.pos += 8;
    return v;
  }

  bytes(n: number): Uint8Array {
    this.check(n);
    const b = this.u8.slice(this.pos, this.pos + n);
    this.pos += n;
    return b;
  }

  align(n: number): void {
    const rel = this.pos - this.base;
    this.pos += (n - (rel % n)) % n;
  }

  /** The string table (see crates/wasm/src/wire/binary.rs). */
  readStrings(tableOffset: number): void {
    const save = this.pos;
    this.pos = this.base + tableOffset;
    const count = this.u32();
    const nSpecial = this.u32();
    const blobLen = this.u32();
    this.check(blobLen);
    const whole = blobLen ? utf8.decode(this.u8.subarray(this.pos, this.pos + blobLen)) : "";
    this.pos += blobLen;
    this.align(4);
    const nPlain = count - nSpecial;
    // every string has a 4-byte offset or entry: the counts cannot exceed the block
    if (nPlain < 0 || count * 4 > this.u8.length - this.pos) throw new WireError("string counts");
    const endsAt = this.pos;
    this.pos += nPlain * 4;
    const strings: string[] = new Array(count);
    const special = new Uint8Array(count);
    for (let k = 0; k < nSpecial; k++) {
      const idx = this.u32();
      const n = this.u32();
      this.check(n * 2);
      const units = new Uint16Array(n);
      for (let j = 0; j < n; j++) units[j] = this.view.getUint16(this.pos + 2 * j, true);
      this.pos += 2 * n;
      this.align(4);
      if (idx >= count) throw new WireError("string index");
      strings[idx] = fromUnits(units);
      special[idx] = 1;
    }
    let start = 0;
    let p = endsAt;
    for (let i = 0; i < count; i++) {
      if (special[i]) continue;
      const end = this.view.getUint32(p, true);
      p += 4;
      strings[i] = whole.substring(start, end);
      start = end;
    }
    this.strings = strings;
    this.pos = save;
  }

  str(): string {
    const i = this.varint();
    const s = this.strings[i];
    if (s === undefined) throw new WireError("string index");
    return s;
  }

  f64Array(n: number): Float64Array {
    this.check(n * 8);
    const abs = this.u8.byteOffset + this.pos;
    let out: Float64Array;
    if (LITTLE_ENDIAN && abs % 8 === 0) {
      out = new Float64Array(this.u8.buffer, abs, n);
    } else {
      out = new Float64Array(n);
      for (let i = 0; i < n; i++) out[i] = this.view.getFloat64(this.pos + 8 * i, true);
    }
    this.pos += n * 8;
    return out;
  }

  u32Array(n: number): Uint32Array {
    this.check(n * 4);
    const abs = this.u8.byteOffset + this.pos;
    let out: Uint32Array;
    if (LITTLE_ENDIAN && abs % 4 === 0) {
      out = new Uint32Array(this.u8.buffer, abs, n);
    } else {
      out = new Uint32Array(n);
      for (let i = 0; i < n; i++) out[i] = this.view.getUint32(this.pos + 4 * i, true);
    }
    this.pos += n * 4;
    return out;
  }

  /** One value, as `value()` reads it, recursing into the lists and tuples
   * of the first `depth` levels (performance: the generic cells of a
   * table, points and paths, are small nested sequences, which the frames
   * of `value()` make slow); `value()` reads anything else, and what is
   * deeper. Only tables call it, which a typed block has none of: values
   * are the plain ones. */
  item(depth: number): unknown {
    if (this.pos < this.u8.length) {
      const tag = this.u8[this.pos];
      switch (tag) {
        // the scalars of points and paths, read as `value()` reads them
        case TAG.F64:
          this.pos++;
          return this.f64();
        case TAG.I32: {
          this.pos++;
          this.check(4);
          const v = this.view.getInt32(this.pos, true);
          this.pos += 4;
          return v;
        }
        case TAG.STR:
          this.pos++;
          return this.str();
        case TAG.LIST:
        case TAG.TUPLE: {
          if (depth <= 0) break;
          this.pos++;
          const n = this.varint();
          if (n > this.u8.length - this.pos) throw new WireError("list longer than its block");
          const arr = new Array<unknown>(n);
          for (let i = 0; i < n; i++) arr[i] = this.item(depth - 1);
          return arr;
        }
      }
    }
    return this.value();
  }

  /** One value, without recursion (a table's generic cells recurse once
   * per nested table). */
  value(): unknown {
    const u8 = this.u8;
    const stack: Frame[] = [];
    let value: unknown;
    for (;;) {
      if (this.pos >= u8.length) throw new WireError("truncated value");
      const tag = u8[this.pos++];
      switch (tag) {
        case TAG.NONE:
          value = null;
          break;
        case TAG.FALSE:
          value = false;
          break;
        case TAG.TRUE:
          value = true;
          break;
        case TAG.I32:
          this.check(4);
          value = this.view.getInt32(this.pos, true);
          this.pos += 4;
          break;
        case TAG.INT_F64:
          value = this.f64();
          break;
        case TAG.F64:
          value = this.typed ? new PyFloat(this.f64()) : this.f64();
          break;
        case TAG.BIGINT: {
          const n = this.varint();
          this.check(n);
          value = BigInt(latin1.decode(u8.subarray(this.pos, this.pos + n)));
          this.pos += n;
          break;
        }
        case TAG.STR:
          value = this.str();
          break;
        case TAG.INLINE_STR: {
          const n = this.varint();
          value = decodeGeneralizedUtf8(this.bytes(n));
          break;
        }
        case TAG.BYTES:
          value = this.bytes(this.varint());
          break;
        case TAG.LIST:
        case TAG.TUPLE: {
          const n = this.varint();
          if (n > u8.length - this.pos) throw new WireError("list longer than its block");
          const tuple = this.typed && tag === TAG.TUPLE;
          if (n === 0) {
            value = tuple ? new PyTuple([]) : [];
            break;
          }
          stack.push({ k: K.LIST, arr: new Array(n), n, i: 0, tuple });
          continue;
        }
        case TAG.DICT: {
          const n = this.varint();
          if (n > u8.length - this.pos) throw new WireError("dict longer than its block");
          if (n === 0) {
            value = this.typed ? new PyDict([]) : {};
            break;
          }
          if (this.typed) stack.push({ k: K.DICT, entries: [], n, i: 0 });
          else stack.push({ k: K.DICT, obj: {}, n, i: 0 });
          continue;
        }
        case TAG.LITERAL:
          stack.push({ k: K.LITERAL, n: 1, i: 0 });
          continue;
        case TAG.KEYWORD:
          value = new PSKeyword(this.bytes(this.varint()));
          break;
        case TAG.REF:
          stack.push({ k: K.REF, n: 1, i: 0 });
          continue;
        case TAG.STREAM:
          stack.push({ k: K.STREAM, arr: new Array(5), n: 5, i: 0 });
          continue;
        case TAG.PAGE:
          stack.push({ k: K.PAGE, arr: new Array(2), n: 2, i: 0 });
          continue;
        case TAG.STREAM_REF:
          stack.push({ k: K.STREAM_REF, arr: new Array(4), n: 4, i: 0 });
          continue;
        case TAG.TABLE:
          if (this.typed) throw new WireError("a table in a typed block");
          value = this.table();
          break;
        default:
          throw new WireError(`unknown tag ${tag}`);
      }
      // attach to the open containers, closing the full ones
      for (;;) {
        const top = stack[stack.length - 1];
        if (top === undefined) return value;
        if (top.k === K.LIST) {
          top.arr![top.i++] = value;
          if (top.i < top.n) break;
          value = top.tuple ? new PyTuple(top.arr!) : top.arr;
        } else if (top.k === K.DICT) {
          if (top.key === undefined) {
            if (typeof value !== "string") throw new WireError("a dict key is not a str");
            top.key = value;
            break;
          }
          if (top.entries) top.entries.push([top.key, value]);
          else setOwn(top.obj!, top.key, value);
          top.key = undefined;
          if (++top.i < top.n) break;
          value = top.entries ? new PyDict(top.entries) : top.obj;
        } else if (top.k === K.LITERAL) {
          value = new PSLiteral(value as string | Uint8Array);
        } else if (top.k === K.REF) {
          value = new PDFObjRef(value as number | bigint);
        } else if (top.k === K.PAGE) {
          top.arr![top.i++] = value;
          if (top.i < top.n) break;
          value = pageRef(top.arr![0] as number, top.arr![1] as number | bigint);
        } else if (top.k === K.STREAM_REF) {
          top.arr![top.i++] = value;
          if (top.i < top.n) break;
          value = streamRef(top.arr!);
        } else {
          top.arr![top.i++] = value;
          if (top.i < top.n) break;
          const [objid, genno, attrs, rawdata, data] = top.arr as [
            number | bigint | null,
            number | bigint | null,
            Record<string, unknown>,
            Uint8Array | null,
            Uint8Array | null,
          ];
          value = new PDFStream(objid, genno, attrs, rawdata, data);
        }
        stack.pop();
      }
    }
  }

  /** A list of dicts written by columns, grouped by key sequence (shape):
   * each column is decoded into an array, the rows of each shape are built
   * (see `rowBuilder`), then put back in order. */
  table(): Record<string, unknown>[] {
    const n = this.varint();
    const nshapes = this.varint();
    if (n > this.u8.length - this.pos || nshapes > n || nshapes === 0) throw new WireError("bad table");
    const groups: Record<string, unknown>[][] = [];
    let total = 0;
    for (let s = 0; s < nshapes; s++) {
      const rows = this.varint();
      const ncols = this.varint();
      total += rows;
      if (total > n) throw new WireError("bad table");
      const keys: string[] = [];
      for (let c = 0; c < ncols; c++) keys.push(this.str());
      const kinds: number[] = [];
      const cols: ArrayLike<unknown>[] = [];
      for (let c = 0; c < ncols; c++) {
        this.check(1);
        const kind = this.u8[this.pos++];
        kinds.push(kind);
        cols.push(this.column(kind, rows));
      }
      groups.push(rowBuilder(keys, kinds)(cols, rows));
    }
    if (total !== n) throw new WireError("bad table");
    if (nshapes === 1) return groups[0];
    this.check(n);
    const order = this.u8.subarray(this.pos, this.pos + n);
    this.pos += n;
    const next = new Array<number>(nshapes).fill(0);
    const out: Record<string, unknown>[] = new Array(n);
    for (let i = 0; i < n; i++) {
      const s = order[i];
      const row = s < nshapes ? groups[s][next[s]++] : undefined;
      if (row === undefined) throw new WireError("bad table order");
      out[i] = row;
    }
    return out;
  }

  /** The cells of a column. */
  column(kind: number, n: number): ArrayLike<unknown> {
    switch (kind) {
      case COL.NONE:
        return NULLS;
      case COL.BOOL: {
        this.check(n);
        const out = new Array<boolean>(n);
        for (let i = 0; i < n; i++) out[i] = this.u8[this.pos + i] !== 0;
        this.pos += n;
        return out;
      }
      case COL.F64:
        this.align(8);
        return this.f64Array(n);
      case COL.OPT_F64: {
        this.check(n);
        const mask = this.u8.subarray(this.pos, this.pos + n);
        this.pos += n;
        this.align(8);
        const a = this.f64Array(n);
        const out = new Array<number | null>(n);
        for (let i = 0; i < n; i++) out[i] = mask[i] ? a[i] : null;
        return out;
      }
      case COL.STR:
      case COL.OPT_STR: {
        this.align(4);
        const idx = this.u32Array(n);
        const strings = this.strings;
        const out = new Array<string | null>(n);
        for (let i = 0; i < n; i++) {
          const j = idx[i];
          if (j === NO_STR && kind === COL.OPT_STR) out[i] = null;
          else {
            const s = strings[j];
            if (s === undefined) throw new WireError("string index");
            out[i] = s;
          }
        }
        return out;
      }
      case COL.NUM_SEQ: {
        // the lengths, then every number (crates/wasm/src/wire/binary.rs)
        this.check(n);
        const lens = this.u8.subarray(this.pos, this.pos + n);
        this.pos += n;
        this.align(8);
        let total = 0;
        for (let i = 0; i < n; i++) if (lens[i] !== NO_SEQ) total += lens[i];
        const values = this.f64Array(total);
        const out = new Array<number[] | null>(n);
        let p = 0;
        for (let i = 0; i < n; i++) {
          const len = lens[i];
          if (len === NO_SEQ) {
            out[i] = null;
            continue;
          }
          const seq = new Array<number>(len);
          for (let j = 0; j < len; j++) seq[j] = values[p++];
          out[i] = seq;
        }
        return out;
      }
      case COL.GENERIC: {
        const out = new Array<unknown>(n);
        for (let i = 0; i < n; i++) out[i] = this.item(SHALLOW);
        return out;
      }
      default:
        throw new WireError(`unknown column type ${kind}`);
    }
  }
}

/** The levels of nested sequences `Reader.item` reads by recursion. */
const SHALLOW = 3;

/** The cells of a column of `None`s. */
const NULLS: ArrayLike<unknown> = new Proxy([], { get: (_t, p) => (p === "length" ? Infinity : null) });

type RowBuilder = (cols: ArrayLike<unknown>[], n: number) => Record<string, unknown>[];

let evalAllowed: boolean | undefined;
const builders = new Map<string, RowBuilder>();

/** Rows by keyed stores: the builder when code generation is not allowed
 * (a Content-Security-Policy without 'unsafe-eval'). */
function genericRows(keys: string[]): RowBuilder {
  return (cols, n) => {
    const rows: Record<string, unknown>[] = new Array(n);
    for (let i = 0; i < n; i++) {
      const o: Record<string, unknown> = {};
      for (let c = 0; c < keys.length; c++) setOwn(o, keys[c], cols[c][i]);
      rows[i] = o;
    }
    return rows;
  };
}

/** The function that builds the rows of a table shape: an object literal
 * with the keys in order, so that every row is created with its final
 * shape (about six times faster than keyed stores in V8, the measurements
 * of docs/js-runtime.md). The keys are written as JSON string literals
 * (`__proto__` as a computed key, which defines an own property). */
export function rowBuilder(keys: string[], kinds: number[]): RowBuilder {
  if (evalAllowed === undefined) {
    try {
      evalAllowed = new Function("return 1")() === 1;
    } catch {
      evalAllowed = false;
    }
  }
  if (!evalAllowed) return genericRows(keys);
  // unambiguous whatever the keys hold (U+0000 included)
  const signature = JSON.stringify([kinds, keys]);
  let b = builders.get(signature);
  if (b === undefined) {
    if (builders.size >= 512) builders.clear();
    const decl = keys.map((_, c) => `const c${c} = cols[${c}];`).join("");
    const fields = keys.map(
      (k, c) =>
        `${k === "__proto__" ? '["__proto__"]' : JSON.stringify(k)}: ${kinds[c] === COL.NONE ? "null" : `c${c}[i]`}`,
    );
    b = new Function(
      "cols",
      "n",
      `${decl}const rows = new Array(n);for (let i = 0; i < n; i++) rows[i] = {${fields.join(",")}};return rows;`,
    ) as RowBuilder;
    builders.set(signature, b);
  }
  return b;
}

/** Forces the keyed-store builder (tests of the fallback). */
export function setCodegen(allowed: boolean | undefined): void {
  evalAllowed = allowed;
  builders.clear();
}

/** Decodes the PJB1 block at `offset` of `u8`. */
export function decodeBinary(u8: Uint8Array, offset = 0, typed = false): unknown {
  const r = new Reader(u8, offset, typed);
  r.check(HEADER_LEN);
  if (r.view.getUint32(offset, true) !== MAGIC) throw new WireError("bad PJB1 magic");
  const tableOffset = r.view.getUint32(offset + 8, true);
  if (tableOffset) r.readStrings(tableOffset);
  r.pos = offset + HEADER_LEN;
  return r.value();
}

// ---------------------------------------------------------------------------
// Streams and pages by reference

/** What makes the `PDFStream` of a stream reference (its handle, `objid`,
 * `genno`, `attrs`): the core whose response is decoded
 * (core/runtime.ts `StreamRefs`), which reads its data on demand and keeps
 * one object per stream. */
export type StreamRefMaker = (handle: number, objid: number | bigint | null, genno: number | bigint | null, attrs: Record<string, unknown>) => PDFStream;

/** What makes the `PageRef` of a page held as a value: the core whose
 * response is decoded, which notes the document it belongs to. */
export type PageRefMaker = (handle: number, pageNumber: number | bigint) => PageRef;

/** The makers of the objects by reference of a response (its streams and
 * pages). */
export interface RefMakers {
  stream?: StreamRefMaker;
  page?: PageRefMaker;
}

let refMakers: RefMakers | undefined;

/** Runs `f` (the decoding of a response) with the makers of its streams
 * and pages by reference. */
export function withRefMakers<T>(makers: RefMakers | undefined, f: () => T): T {
  const previous = refMakers;
  refMakers = makers;
  try {
    return f();
  } finally {
    refMakers = previous;
  }
}

/** A stream reference's `PDFStream`: from the maker, or, decoded outside
 * a core (a response copied elsewhere), a stream whose data cannot be
 * read. */
function streamRef(fields: unknown[]): PDFStream {
  const [handle, objid, genno, attrs] = fields as [number, number | bigint | null, number | bigint | null, Record<string, unknown>];
  const maker = refMakers?.stream;
  return maker ? maker(handle, objid, genno, attrs) : PDFStream.reference(objid, genno, attrs, handle, null);
}

/** A page value's `PageRef`: from the maker, or, decoded outside a core,
 * a reference no document takes (`pdf.page()` refuses it). */
function pageRef(handle: number, pageNumber: number | bigint): PageRef {
  const maker = refMakers?.page;
  return maker ? maker(handle, pageNumber) : new PageRef(handle, pageNumber);
}

// ---------------------------------------------------------------------------
// JSON results

const hexBytes = (hex: string): Uint8Array => {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(hex.substr(2 * i, 2), 16);
  return out;
};

/** Undoes the escapes of the JSON encoding (crates/wasm/src/wire/json.rs)
 * without recursion. */
function fixup(root: unknown): unknown {
  const box = { v: root };
  type Slot = [Record<string | number, unknown>, string | number];
  const stack: Slot[] = [[box as unknown as Record<string, unknown>, "v"]];
  // wrappers to build once their fields are fixed: [holder, key, make]
  const pending: [Record<string | number, unknown>, string | number, (x: never) => unknown][] = [];
  const special = (o: Record<string, unknown>): unknown[] | undefined => {
    const keys = Object.keys(o);
    return keys.length === 1 && keys[0] === "\u0000" ? (o["\u0000"] as unknown[]) : undefined;
  };
  while (stack.length) {
    const [holder, key] = stack.pop()!;
    const v = holder[key];
    if (v === null || typeof v !== "object") continue;
    if (Array.isArray(v)) {
      for (let i = 0; i < v.length; i++) stack.push([v as unknown as Record<number, unknown>, i]);
      continue;
    }
    const o = v as Record<string, unknown>;
    const sp = special(o);
    if (sp) {
      const [tag] = sp;
      switch (tag) {
        case "nan":
          holder[key] = NaN;
          break;
        case "inf":
          holder[key] = Infinity;
          break;
        case "-inf":
          holder[key] = -Infinity;
          break;
        case "int":
          holder[key] = BigInt(sp[1] as string);
          break;
        case "bytes":
          holder[key] = hexBytes(sp[1] as string);
          break;
        case "kw":
          holder[key] = new PSKeyword(hexBytes(sp[1] as string));
          break;
        case "lit": {
          const lit = { name: sp[1] };
          stack.push([lit as Record<string, unknown>, "name"]);
          // resolved after its name: a two-step slot
          holder[key] = lit;
          pending.push([holder, key, (x: { name: unknown }) => new PSLiteral(x.name as string)]);
          break;
        }
        case "ref": {
          const ref = { objid: sp[1] };
          stack.push([ref as Record<string, unknown>, "objid"]);
          holder[key] = ref;
          pending.push([holder, key, (x: { objid: unknown }) => new PDFObjRef(x.objid as number)]);
          break;
        }
        case "stream": {
          const s = { objid: sp[1], genno: sp[2], attrs: sp[3], rawdata: sp[4], data: sp[5] };
          for (const f of ["objid", "genno", "attrs", "rawdata", "data"]) stack.push([s as Record<string, unknown>, f]);
          holder[key] = s;
          pending.push([
            holder,
            key,
            (x: Record<string, unknown>) =>
              new PDFStream(
                x.objid as number,
                x.genno as number,
                x.attrs as Record<string, unknown>,
                x.rawdata as Uint8Array,
                x.data as Uint8Array,
              ),
          ]);
          break;
        }
        case "streamref": {
          const s = { handle: sp[1], objid: sp[2], genno: sp[3], attrs: sp[4] };
          for (const f of ["handle", "objid", "genno", "attrs"]) stack.push([s as Record<string, unknown>, f]);
          holder[key] = s;
          pending.push([holder, key, (x: Record<string, unknown>) => streamRef([x.handle, x.objid, x.genno, x.attrs])]);
          break;
        }
        case "page": {
          const page = { handle: sp[1], page_number: sp[2] };
          for (const f of ["handle", "page_number"]) stack.push([page as Record<string, unknown>, f]);
          holder[key] = page;
          pending.push([
            holder,
            key,
            (x: Record<string, unknown>) => pageRef(x.handle as number, x.page_number as number),
          ]);
          break;
        }
        default:
          throw new WireError(`unknown JSON escape ${String(tag)}`);
      }
      continue;
    }
    // a real dict: unescape the keys that start with U+0000
    if (Object.keys(o).some((k) => k.startsWith("\u0000"))) {
      const fixed: Record<string, unknown> = {};
      for (const k of Object.keys(o)) setOwn(fixed, k.startsWith("\u0000") ? k.slice(1) : k, o[k]);
      holder[key] = fixed;
      for (const k of Object.keys(fixed)) stack.push([fixed, k]);
    } else {
      for (const k of Object.keys(o)) stack.push([o, k]);
    }
  }
  // build the wrapper objects innermost first
  for (let i = pending.length - 1; i >= 0; i--) {
    const [holder, key, make] = pending[i];
    holder[key] = make(holder[key] as never);
  }
  return box.v;
}

/** Decodes a JSON result. */
export function decodeJson(u8: Uint8Array, needsFixup: boolean): unknown {
  const v = JSON.parse(utf8.decode(u8));
  return needsFixup ? fixup(v) : v;
}

// ---------------------------------------------------------------------------
// Encoding arguments (JavaScript → Rust)

/** Python's `s.encode("utf-8", "surrogatepass")` of a JS string: UTF-8, a
 * lone surrogate as its own three bytes. */
export function encodeGeneralizedUtf8(s: string): Uint8Array {
  const out: number[] = [];
  for (let i = 0; i < s.length; i++) {
    let c = s.charCodeAt(i);
    if (c >= 0xd800 && c < 0xdc00 && i + 1 < s.length) {
      const d = s.charCodeAt(i + 1);
      if (d >= 0xdc00 && d < 0xe000) {
        c = 0x10000 + ((c - 0xd800) << 10) + (d - 0xdc00);
        i++;
      }
    }
    if (c < 0x80) out.push(c);
    else if (c < 0x800) out.push(0xc0 | (c >> 6), 0x80 | (c & 0x3f));
    else if (c < 0x10000) out.push(0xe0 | (c >> 12), 0x80 | ((c >> 6) & 0x3f), 0x80 | (c & 0x3f));
    else out.push(0xf0 | (c >> 18), 0x80 | ((c >> 12) & 0x3f), 0x80 | ((c >> 6) & 0x3f), 0x80 | (c & 0x3f));
  }
  return Uint8Array.from(out);
}

/** The inverse of `encodeGeneralizedUtf8`. */
export function decodeGeneralizedUtf8(b: Uint8Array): string {
  const units: number[] = [];
  for (let i = 0; i < b.length; ) {
    const b0 = b[i];
    let len: number, c: number, min: number;
    if (b0 < 0x80) [len, c, min] = [1, b0, 0];
    else if (b0 >= 0xc2 && b0 < 0xe0) [len, c, min] = [2, b0 & 0x1f, 0x80];
    else if (b0 >= 0xe0 && b0 < 0xf0) [len, c, min] = [3, b0 & 0x0f, 0x800];
    else if (b0 >= 0xf0 && b0 < 0xf5) [len, c, min] = [4, b0 & 0x07, 0x10000];
    else throw new WireError("invalid generalized UTF-8");
    if (i + len > b.length) throw new WireError("invalid generalized UTF-8");
    for (let k = 1; k < len; k++) {
      if ((b[i + k] & 0xc0) !== 0x80) throw new WireError("invalid generalized UTF-8");
      c = (c << 6) | (b[i + k] & 0x3f);
    }
    if (c < min || c > 0x10ffff) throw new WireError("invalid generalized UTF-8");
    if (c >= 0x10000) units.push(0xd800 + ((c - 0x10000) >> 10), 0xdc00 + ((c - 0x10000) & 0x3ff));
    else units.push(c);
    i += len;
  }
  return fromUnits(Uint16Array.from(units));
}

const utf8Encoder = new TextEncoder();
const isWellFormed: (s: string) => boolean =
  typeof (String.prototype as { isWellFormed?: () => boolean }).isWellFormed === "function"
    ? (s) => (s as unknown as { isWellFormed(): boolean }).isWellFormed()
    : (s) => !/[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/.test(s);

class ByteWriter {
  buf = new Uint8Array(256);
  view = new DataView(this.buf.buffer);
  len = 0;

  reserve(n: number): void {
    if (this.len + n <= this.buf.length) return;
    let size = this.buf.length * 2;
    while (size < this.len + n) size *= 2;
    const next = new Uint8Array(size);
    next.set(this.buf.subarray(0, this.len));
    this.buf = next;
    this.view = new DataView(next.buffer);
  }

  u8(b: number): void {
    this.reserve(1);
    this.buf[this.len++] = b;
  }

  varint(v: number): void {
    this.reserve(10);
    while (v >= 0x80) {
      this.buf[this.len++] = (v % 128) | 0x80;
      v = Math.floor(v / 128);
    }
    this.buf[this.len++] = v;
  }

  raw(b: Uint8Array): void {
    this.reserve(b.length);
    this.buf.set(b, this.len);
    this.len += b.length;
  }
}

/** Marks, on the work stack of `encodeValue`, the end of the container
 * below it. */
const EXIT = Symbol("exit");

/** Whether `v` is a plain object (`{...}`, `Object.create(null)`, or a
 * plain object of another realm): the objects that cross as a `dict`, with
 * `Map`s. An instance of any other class (`Set`, `Date`, a typed array
 * other than `Uint8Array`, a class of the caller's...) has no Python value
 * (docs/public-api.md, "Arguments"). */
export function isPlainObject(v: unknown): v is Record<string, unknown> {
  if (v === null || typeof v !== "object") return false;
  const proto = Object.getPrototypeOf(v) as object | null;
  return proto === null || Object.getPrototypeOf(proto) === null;
}

/** The class of an object, for an error message. */
function className(v: unknown): string {
  const name = (v as { constructor?: { name?: unknown } } | null)?.constructor?.name;
  return typeof name === "string" && name ? name : "object";
}

/** Decides whether an object by reference of a result (a `PageRef`, a
 * `PDFStream`) can be an argument of a request: one of the core the request
 * goes to (core/runtime.ts `StreamRefs.owns`). */
export type RefCheck = (ref: PageRef | PDFStream) => boolean;

const OTHER_CORE = (kind: string): string =>
  `a ${kind} of a document of another core (read before configure() or a CoreCrash) cannot be an argument`;

/** Encodes a JavaScript value as a PJB1 block of plain Python values:
 * `null`/`undefined` → `None`, booleans, safe integers → `int` (and
 * `bigint`), other numbers and `PyFloat` → `float`, strings → `str` (lone
 * surrogates kept), `Uint8Array`/`ArrayBuffer` → `bytes`, arrays → `list`,
 * `PyTuple` → `tuple`, plain objects (`isPlainObject`) and `Map`s with
 * string keys → `dict` (`undefined` values left out, in both). Without
 * recursion: any depth encodes, and the core decodes and frees it without
 * recursion too. A container inside itself is a `TypeError` (Python's
 * `json.dumps` raises "Circular reference detected"); the same object twice
 * is not. Any other object is a `TypeError` (a `Set`, a `Date`, a
 * `Float64Array`... has no Python value: never an empty `dict`), and so is
 * an object by reference that `accept` refuses (one of another core). */
export function encodeValue(value: unknown, accept?: RefCheck): Uint8Array {
  const w = new ByteWriter();
  w.raw(new Uint8Array([0x50, 0x4a, 0x42, 0x31, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
  const str = (s: string): void => {
    w.u8(TAG.INLINE_STR);
    const bytes = isWellFormed(s) ? utf8Encoder.encode(s) : encodeGeneralizedUtf8(s);
    w.varint(bytes.length);
    w.raw(bytes);
  };
  // values to write, last first; a container is followed by its items,
  // then by `EXIT` and itself (it leaves `open` once they are written)
  const stack: unknown[] = [value];
  const open = new Set<object>();
  const enter = (v: object): void => {
    if (open.has(v)) throw new TypeError("cyclic argument: a container holds itself");
    open.add(v);
    stack.push(v, EXIT);
  };
  const write = (v: unknown): void => {
    if (v === null || v === undefined) return w.u8(TAG.NONE);
    switch (typeof v) {
      case "boolean":
        return w.u8(v ? TAG.TRUE : TAG.FALSE);
      case "number":
        if (Number.isSafeInteger(v) && !Object.is(v, -0)) {
          if (v >= -0x80000000 && v <= 0x7fffffff) {
            w.u8(TAG.I32);
            w.reserve(4);
            w.view.setInt32(w.len, v, true);
            w.len += 4;
          } else {
            w.u8(TAG.INT_F64);
            w.reserve(8);
            w.view.setFloat64(w.len, v, true);
            w.len += 8;
          }
          return;
        }
        w.u8(TAG.F64);
        w.reserve(8);
        w.view.setFloat64(w.len, v, true);
        w.len += 8;
        return;
      case "bigint": {
        const text = v.toString();
        w.u8(TAG.BIGINT);
        w.varint(text.length);
        w.raw(utf8Encoder.encode(text));
        return;
      }
      case "string":
        return str(v);
      case "function":
      case "symbol":
        // no Python value (docs/public-api.md, "Arguments")
        throw new TypeError(`a ${typeof v} cannot cross to Python`);
    }
    if (v instanceof PyFloat) {
      w.u8(TAG.F64);
      w.reserve(8);
      w.view.setFloat64(w.len, v.value, true);
      w.len += 8;
      return;
    }
    if (v instanceof Uint8Array || v instanceof ArrayBuffer) {
      const b = v instanceof Uint8Array ? v : new Uint8Array(v);
      w.u8(TAG.BYTES);
      w.varint(b.length);
      w.raw(b);
      return;
    }
    // the objects of a result given back: the objects they stand for
    // (crates/wasm/src/wire/binary.rs `decode_args`)
    if (v instanceof PSLiteral) {
      w.u8(TAG.LITERAL);
      stack.push(v.name);
      return;
    }
    if (v instanceof PSKeyword) {
      w.u8(TAG.KEYWORD);
      w.varint(v.name.length);
      w.raw(v.name);
      return;
    }
    if (v instanceof PDFObjRef) {
      w.u8(TAG.REF);
      stack.push(v.objid);
      return;
    }
    if (v instanceof PageRef) {
      if (accept && !accept(v)) throw new TypeError(OTHER_CORE("PageRef"));
      w.u8(TAG.PAGE);
      stack.push(v.page_number, v.handle);
      return;
    }
    if (v instanceof PDFStream) {
      if (v.handle === undefined) throw new TypeError("a PDFStream snapshot (made from its values) cannot be an argument");
      if (accept && !accept(v)) throw new TypeError(OTHER_CORE("PDFStream"));
      w.u8(TAG.STREAM_REF);
      stack.push(v.attrs, v.genno, v.objid, v.handle);
      return;
    }
    if (Array.isArray(v) || v instanceof PyTuple) {
      const items = Array.isArray(v) ? v : v.items;
      w.u8(Array.isArray(v) ? TAG.LIST : TAG.TUPLE);
      w.varint(items.length);
      enter(v as object);
      for (let i = items.length - 1; i >= 0; i--) stack.push(items[i]);
      return;
    }
    let entries: [unknown, unknown][];
    if (v instanceof PyDict) entries = v.entries;
    else if (v instanceof Map) entries = [...v.entries()].filter(([, x]) => x !== undefined);
    else if (isPlainObject(v)) entries = Object.entries(v).filter(([, x]) => x !== undefined);
    else throw new TypeError(`a ${className(v)} cannot cross to Python (docs/public-api.md, "Arguments")`);
    w.u8(TAG.DICT);
    w.varint(entries.length);
    enter(v as object);
    for (let i = entries.length - 1; i >= 0; i--) {
      const [k, x] = entries[i];
      if (typeof k !== "string") throw new TypeError("dict keys must be strings");
      // a key is written as a `str` value is
      stack.push(x, k);
    }
  };
  while (stack.length) {
    const v = stack.pop();
    if (v === EXIT) open.delete(stack.pop() as object);
    else write(v);
  }
  return w.buf.slice(0, w.len);
}
