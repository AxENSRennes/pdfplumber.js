import { describe, expect, it } from "vitest";

import { parsePathOps, parseTextMatrixOps, parseTextMoveOps, parseTransformOps } from "../../src/pdf.js";

function lcg(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (1664525 * state + 1013904223) >>> 0;
    return state / 0x100000000;
  };
}

function pick<T>(random: () => number, values: T[]): T {
  return values[Math.floor(random() * values.length)];
}

describe("targeted content stream fuzz regressions", () => {
  it("ignores operators hidden in comments, strings, arrays, names, hex strings, and dictionaries", () => {
    const random = lcg(0x5eed);
    const traps = [
      "% 1 0 0 1 999 999 cm",
      "(1 0 0 1 998 998 Tm)",
      "[(7 8 Td) (9 10 TD) (10 10 m 20 20 l S)] TJ",
      "<31203020302031203939372039393720636d>",
      "<< /Trap (1 0 0 1 996 996 cm) /Nested [ (1 2 Td) ] >>",
      "/NameWithcm /OtherTm"
    ];
    const realOps = [
      "1 0 0 1 10 20 cm",
      "1 0 0 1 30 40 Tm",
      "3 4 Td",
      "10 10 m 20 20 l S"
    ];
    const content = Array.from({ length: 80 }, (_unused, index) => (index % 17 === 0 ? realOps[index / 17] ?? pick(random, traps) : pick(random, traps))).join("\n");

    expect(parseTransformOps(content)).toEqual([[1, 0, 0, 1, 10, 20]]);
    expect(parseTextMatrixOps(content)).toEqual([[1, 0, 0, 1, 30, 40]]);
    expect(parseTextMoveOps(content)).toEqual({ move: [[3, 4]], leadingMove: [] });
    expect(parsePathOps(content)).toEqual([[0, 10, 10, 1, 20, 20]]);
  });

  it("handles randomized whitespace around real operators", () => {
    const random = lcg(0xc0ffee);
    const ws = [" ", "\n", "\r", "\r\n", "\t", "\f"];
    const join = (tokens: string[]) => tokens.map((token) => `${pick(random, ws)}${token}`).join("");
    const content = [
      join(["1", "0", "0", "1", "11", "22", "cm"]),
      join(["1", "0", "0", "1", "33", "44", "Tm"]),
      join(["5", "6", "TD"]),
      join(["7", "8", "m", "9", "10", "l", "s"])
    ].join("");

    expect(parseTransformOps(content)).toEqual([[1, 0, 0, 1, 11, 22]]);
    expect(parseTextMatrixOps(content)).toEqual([[1, 0, 0, 1, 33, 44]]);
    expect(parseTextMoveOps(content)).toEqual({ move: [], leadingMove: [[5, 6]] });
    expect(parsePathOps(content)).toEqual([[0, 7, 8, 1, 9, 10, 4]]);
  });
});
