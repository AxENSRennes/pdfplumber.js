import { describe, expect, it } from "vitest";

import { parseColorOps, parsePathOps, parseTextMatrixOps, parseTextMoveOps, parseTransformOps } from "../../src/pdf.js";
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
