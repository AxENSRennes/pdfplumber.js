// The table decoder's generic cells (js/src/core/wire.ts, `Reader.item`,
// the PERF fast path of wave 5): a table whose generic column holds a cell
// decodes to the value the general reader (`value()`) gives the same bytes
// in a plain list, for every kind of value a cell can hold (the scalars
// `item` reads itself, nested lists and tuples up to its depth and beyond,
// and what it hands to `value()`). The tables the core writes are covered by
// the runtime tests and the API sessions; these are blocks made here.
import { describe, expect, it } from "vitest";

import { decodeBinary, encodeValue } from "../../src/core/wire.js";
import { PyTuple } from "../../src/pdfobjects.js";

/** The bytes of a value written by `encodeValue`, without its header. */
const body = (v: unknown): number[] => [...encodeValue(v).subarray(16)];

/** A block: header, `value`, then the string table holding `strings`
 * (none without strings). */
function block(value: number[], strings: string[]): Uint8Array {
  const b = [0x50, 0x4a, 0x42, 0x31, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, ...value];
  if (strings.length === 0) return new Uint8Array(b);
  const u32 = (v: number): void => void b.push(v & 255, (v >> 8) & 255, (v >> 16) & 255, v >>> 24);
  while (b.length % 4) b.push(0);
  const at = b.length;
  b[8] = at & 255;
  b[9] = (at >> 8) & 255;
  b[10] = (at >> 16) & 255;
  const blob = new TextEncoder().encode(strings.join(""));
  u32(strings.length);
  u32(0);
  u32(blob.length);
  b.push(...blob);
  while (b.length % 4) b.push(0);
  let end = 0;
  for (const s of strings) u32((end += s.length));
  return new Uint8Array(b);
}

const STR = (i: number): number[] => [0x07, i];
const LIST = (n: number, ...items: number[][]): number[] => [0x09, n, ...items.flat()];
const TUPLE = (n: number, ...items: number[][]): number[] => [0x0a, n, ...items.flat()];

// Cells as PJB1 bytes: the string table holds "k" (#0) and "s" (#1).
const cells: [string, number[]][] = [
  ["floats and ints", body([1.5, -0, 2, -3, 2 ** 40, NaN])],
  ["points", body([[1.5, 2.25], [-1, 3e300]])],
  ["a path", LIST(2, TUPLE(2, STR(1), body(new PyTuple([1.5, 2]))), TUPLE(3, STR(1), body(new PyTuple([0.5, -1])), body(new PyTuple([7, 8]))))],
  ["strings", LIST(3, STR(1), STR(0), body("inline é"))],
  ["scalars item leaves to value()", body([null, true, false, 2n ** 70n, new Uint8Array([1, 2]), { a: [1] }])],
  ["an empty list", body([])],
  ["an empty tuple", body(new PyTuple([]))],
  ["deeper than item reads", body([[[[[1.5, [2]]]]]])],
  ["a bare float", body(0.1)],
  ["a bare int", body(-7)],
  ["a bare str", STR(1)],
  ["a bare None", body(null)],
  ["a tuple of tuples", body(new PyTuple([new PyTuple([1, 2]), new PyTuple(["x", null])]))],
];

/** A table of one row per cell (one shape, one generic column "k"). */
function table(rows: number[][]): number[] {
  return [0x10, rows.length, 1, rows.length, 1, 0, 0, ...rows.flat()];
}

describe("wire: generic cells of a table", () => {
  for (const [name, cell] of cells) {
    it(`${name}: a table cell decodes as the value does`, () => {
      const expected = (decodeBinary(block(LIST(1, cell), ["k", "s"])) as unknown[])[0];
      const rows = decodeBinary(block(table([cell, cell]), ["k", "s"])) as { k: unknown }[];
      expect(rows).toHaveLength(2);
      for (const row of rows) expect(row.k).toStrictEqual(expected);
    });
  }

  it("all the cells in one column, in order", () => {
    const expected = decodeBinary(block(LIST(cells.length, ...cells.map(([, c]) => c)), ["k", "s"])) as unknown[];
    const rows = decodeBinary(block(table(cells.map(([, c]) => c)), ["k", "s"])) as { k: unknown }[];
    expect(rows.map((r) => r.k)).toStrictEqual(expected);
  });

  it("a cell nested far beyond the stack of a recursion decodes (item hands it to value())", () => {
    const n = 100000;
    const rows = decodeBinary(block(table([[...Array(n).fill([0x09, 1]).flat(), 0x00]]), ["k"])) as { k: unknown }[];
    let v: unknown = rows[0].k;
    let depth = 0;
    while (Array.isArray(v)) {
      expect(v).toHaveLength(1);
      v = v[0];
      depth++;
    }
    expect([depth, v]).toStrictEqual([n, null]);
  });

  it("a list longer than its block is an error, as in value()", () => {
    for (const tag of [0x09, 0x0a]) {
      expect(() => decodeBinary(block(table([[tag, 100, 0x00]]), ["k"]))).toThrow(/longer than its block/);
      expect(() => decodeBinary(block(LIST(1, [tag, 100, 0x00]), ["k"]))).toThrow(/longer than its block/);
    }
  });
});
