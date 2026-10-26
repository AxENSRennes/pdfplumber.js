/**
 * Typed values: the results of the core with Python's types kept
 * (`int` and `float`, `list` and `tuple`, a `dict`'s key order), what the
 * harness that writes stage traces through the public API reads
 * (`js/trace/`, docs/architecture.md, "Vague 5"). The core writes them in
 * its typed format (`set_format("typed")`: PJB1 without tables,
 * crates/wasm/src/ops.rs `FORMAT_TYPED`); `decodeBinary(.., true)` reads
 * them; `project` gives the public value of a typed one, which is what the
 * API returns: the public values are a projection of the typed ones, never
 * another computation.
 *
 * In a typed value: a Python `float` is a `PyFloat`, an `int` a number
 * (a `bigint` beyond ±2^53), a `tuple` a `PyTuple`, a `dict` a `PyDict`
 * (its keys in Python's order, integer-like ones included), a `list` an
 * array; the other values are as in public results.
 */

import { PageRef, PDFObjRef, PDFStream, PSKeyword, PSLiteral, PyFloat, PyTuple } from "../pdfobjects.js";

/** A Python `dict` of a typed value: its items in Python's order. */
export class PyDict {
  constructor(readonly entries: [string, unknown][]) {}

  /** The value of a key (`undefined` when absent). */
  get(key: string): unknown {
    for (const [k, v] of this.entries) if (k === key) return v;
    return undefined;
  }

  has(key: string): boolean {
    return this.entries.some(([k]) => k === key);
  }

  keys(): string[] {
    return this.entries.map(([k]) => k);
  }
}

function setOwn(obj: Record<string, unknown>, key: string, value: unknown): void {
  Object.defineProperty(obj, key, { value, writable: true, enumerable: true, configurable: true });
}

interface Frame {
  out: unknown[] | Record<string, unknown>;
  /** The items still to convert: [key or index, typed value]. */
  todo: [string | number, unknown][];
  i: number;
  done: (out: unknown) => unknown;
}

/** The public value of a typed value (iterative: any depth). */
export function project(root: unknown): unknown {
  const leaf = (v: unknown): { value: unknown } | undefined => {
    if (v instanceof PyFloat) return { value: v.value };
    if (v === null || typeof v !== "object") return { value: v };
    if (v instanceof Uint8Array || v instanceof PSLiteral || v instanceof PSKeyword || v instanceof PDFObjRef) return { value: v };
    if (v instanceof PageRef) return { value: v };
    return undefined;
  };
  const open = (v: unknown): Frame => {
    if (v instanceof PyTuple || Array.isArray(v)) {
      const items = v instanceof PyTuple ? v.items : (v as unknown[]);
      return { out: new Array(items.length), todo: items.map((x, i) => [i, x]), i: 0, done: (o) => o };
    }
    if (v instanceof PyDict) return { out: {}, todo: v.entries.map(([k, x]) => [k, x]), i: 0, done: (o) => o };
    if (v instanceof PDFStream) {
      const s = v;
      return {
        out: {},
        todo: [["attrs", s.attrs]],
        i: 0,
        done: (o) =>
          new PDFStream(s.objid, s.genno, (o as { attrs: Record<string, unknown> }).attrs, ...s.snapshot()),
      };
    }
    throw new TypeError(`not a typed value: ${Object.prototype.toString.call(v)}`);
  };
  const first = leaf(root);
  if (first) return first.value;
  const stack: Frame[] = [open(root)];
  let result: unknown;
  while (stack.length) {
    const top = stack[stack.length - 1];
    if (top.i === top.todo.length) {
      stack.pop();
      const value = top.done(top.out);
      if (!stack.length) {
        result = value;
        break;
      }
      const parent = stack[stack.length - 1];
      const [key] = parent.todo[parent.i++];
      put(parent.out, key, value);
      continue;
    }
    const [key, item] = top.todo[top.i];
    const l = leaf(item);
    if (l) {
      put(top.out, key, l.value);
      top.i++;
    } else {
      stack.push(open(item));
    }
  }
  return result;
}

function put(out: unknown[] | Record<string, unknown>, key: string | number, value: unknown): void {
  if (Array.isArray(out)) out[key as number] = value;
  else setOwn(out, key as string, value);
}
