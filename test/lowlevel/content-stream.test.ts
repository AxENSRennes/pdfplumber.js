import { describe, expect, it } from "vitest";

import { extractPredefinedCMapCharsFromContent, parseColorOps, parsePathOps, parseTextMatrixOps, parseTextMoveOps, parseTransformOps } from "../../src/pdf.js";
import { parseOperatorStream } from "../../src/pdf/parser.js";
import { parseNumbers } from "../../src/pdf.js";

describe("low-level PDF content stream tokenization", () => {
  it("parses PDF-style numbers including signs, leading decimals, and trailing decimal points", () => {
    expect(parseNumbers("-.002 34.5 -3.62 -1. 0.0 123 -98 +17 .25")).toEqual([-0.002, 34.5, -3.62, -1, 0, 123, -98, 17, 0.25]);
  });

  it("extracts graphics and text matrices while ignoring operators inside strings, arrays, dicts, names, and comments", () => {
    const content = [
      "% 9 9 9 9 9 9 cm",
      "(1 0 0 1 99 99 cm) Tj",
      "[ (0 0 Td) <303120302030203120383820383820546d> ] TJ",
      "<< /Fake (1 0 0 1 77 77 cm) >>",
      "/NameWithTm 12 Tf",
      "1 0 0 1 10 20 cm",
      "0.5 0 0 0.5 30 40 cm",
      "1 0 0 1 50 60 Tm",
      "3 4 Td",
      "5 6 TD"
    ].join("\n");

    expect(parseTransformOps(content)).toEqual([
      [1, 0, 0, 1, 10, 20],
      [0.5, 0, 0, 0.5, 30, 40]
    ]);
    expect(parseTextMatrixOps(content)).toEqual([[1, 0, 0, 1, 50, 60]]);
    expect(parseTextMoveOps(content)).toEqual({ move: [[3, 4]], leadingMove: [[5, 6]] });
  });

  it("drops malformed bare keywords from TJ arrays like pdfminer text rendering", () => {
    expect(parseOperatorStream("[ (Les )1 l3 (bouc)3-5 (a) ] TJ")).toEqual([
      { operator: "TJ", args: [["Les ", 1, "bouc", 3, -5, "a"]] }
    ]);
  });

  it("uses the final text-show operand for malformed Tj stacks like pdfminer", () => {
    const chars = extractPredefinedCMapCharsFromContent(
      "BT /F1 10 Tf 1 0 0 1 10 20 Tm (OK) Tj (DROP)1 Tj [(A) 1 (B)] TJ ET",
      new Map(),
      [],
      1,
      200,
      200,
      0,
      0,
      0,
      0,
      { includeSimpleText: true }
    );

    expect(chars.map((char) => char.text).join("")).toBe("OKAB");
  });

  it("uses StandardEncoding for recovered simple-font bytes without an explicit encoding", () => {
    const chars = extractPredefinedCMapCharsFromContent(
      "BT /Missing 6 Tf (A\\241\\216) Tj ET",
      new Map(),
      [],
      1,
      200,
      200,
      0,
      0,
      0,
      0,
      { includeSimpleText: true }
    );

    expect(chars.map((char) => char.text).join("")).toBe("A\u00a1(cid:142)");
  });

  it("keeps invalid Tf font sizes and unresolved font advances pdfminer-compatible", () => {
    const chars = extractPredefinedCMapCharsFromContent(
      "BT /Missing 6 Tf 1 0 0 1 10 20 Tm (AB) Tj /Missing /BadSize Tf (CD) Tj ET",
      new Map(),
      [],
      1,
      200,
      200,
      0,
      0,
      0,
      0,
      { includeSimpleText: true }
    );

    expect(chars.map((char) => char.text).join("")).toBe("ABCD");
    expect(chars.map((char) => [char.x0, char.x1, char.top, char.bottom, char.adv, char.size])).toEqual([
      [10, 10, 174, 180, 0, 6],
      [10, 10, 174, 180, 0, 6],
      [10, 10, 174, 180, 0, 6],
      [10, 10, 174, 180, 0, 6]
    ]);
  });

  it("marks recovered simple text orientation from the active text matrix", () => {
    const chars = extractPredefinedCMapCharsFromContent(
      "BT /Missing 8 Tf 0 1 -1 0 10 20 Tm (AB) Tj ET",
      new Map(),
      [],
      1,
      200,
      200,
      0,
      0,
      0,
      0,
      { includeSimpleText: true }
    );

    expect(chars.map((char) => [char.text, char.upright, char.matrix])).toEqual([
      ["A", false, [0, 1, -1, 0, 10, 20]],
      ["B", false, [0, 1, -1, 0, 10, 20]]
    ]);
  });

  it("leaves the prior text matrix in place when Tm operands are invalid", () => {
    const chars = extractPredefinedCMapCharsFromContent(
      "BT /Missing 6 Tf 1 0 0 1 10 20 Tm (A) Tj (bad) 0 0 1 30 40 Tm (B) Tj ET",
      new Map(),
      [],
      1,
      200,
      200,
      0,
      0,
      0,
      0,
      { includeSimpleText: true }
    );

    expect(chars.map((char) => [char.text, char.x0, char.x1, char.top, char.bottom, char.matrix])).toEqual([
      ["A", 10, 10, 174, 180, [1, 0, 0, 1, 10, 20]],
      ["B", 10, 10, 174, 180, [1, 0, 0, 1, 10, 20]]
    ]);
  });

  it("restores text state across q/Q like pdfminer", () => {
    const chars = extractPredefinedCMapCharsFromContent(
      "BT /Missing 0 Tf q /Missing 6 Tf Q 1 0 0 1 10 20 Tm (A) Tj ET",
      new Map(),
      [],
      1,
      200,
      200,
      0,
      0,
      0,
      0,
      { includeSimpleText: true }
    );

    expect(chars.map((char) => [char.text, char.x0, char.x1, char.top, char.bottom, char.size])).toEqual([
      ["A", 10, 10, 180, 180, 0]
    ]);
  });

  it("parses path construction operators into PDF.js-like path opcode arrays", () => {
    const content = [
      "10 20 m 30 40 l 50 60 70 80 90 100 c h S",
      "100 200 m 110 210 120 220 v 130 230 140 240 y s",
      "1 2 3 4 re f",
      "n"
    ].join("\n");

    expect(parsePathOps(content)).toEqual([
      [0, 10, 20, 1, 30, 40, 2, 50, 60, 70, 80, 90, 100, 4],
      [0, 100, 200, 2, 100, 200, 110, 210, 120, 220, 3, 130, 230, 140, 240, 4],
      [5, 1, 2, 3, 4],
      []
    ]);
  });

  it("tracks color spaces, colors, and pattern names in paint operators", () => {
    const ops = parseColorOps(
      [
        "0.25 g",
        "0.1 0.2 0.3 RG",
        "/Cs1 cs 0.4 0.5 0.6 sc",
        "/Pattern cs /P1 scn",
        "0 0 0 1 K"
      ].join("\n"),
      { Cs1: "DeviceRGB" }
    );

    expect(ops).toEqual([
      { target: "fill", components: [0.25], colorSpace: "DeviceGray", pattern: undefined },
      { target: "stroke", components: [0.1, 0.2, 0.3], colorSpace: "DeviceRGB", pattern: undefined },
      { target: "fill", components: [0.4, 0.5, 0.6], colorSpace: "DeviceRGB", pattern: undefined },
      { target: "fill", components: [], colorSpace: "Pattern", pattern: "P1" },
      { target: "stroke", components: [0, 0, 0, 1], colorSpace: "DeviceCMYK", pattern: undefined }
    ]);
  });
});
