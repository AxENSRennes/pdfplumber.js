import { execFileSync } from "node:child_process";

import { describe, expect, it } from "vitest";

import { buildLayoutObjects, findLineNeighborsLikePdfminer, type LayoutLineLikePdfminer, type LayoutLineLikePdfminerType } from "../../src/layout.js";
import type { PDFObject } from "../../src/types.js";

type BBox = readonly [number, number, number, number];
type NeighborLine = LayoutLineLikePdfminer & { label: string };

const parentHeight = 50;
const neighborPlaneHeight = 55;

function pdfminerPython(code: string): string {
  return execFileSync("wsl_venv/bin/python", ["-c", code], { encoding: "utf8" }).trim();
}

function pdfminerWrongParentTextboxCount(): number {
  const code = `
from pdfminer.layout import LAParams, LTLayoutContainer, LTTextLineHorizontal

laparams = LAParams()
layout = LTLayoutContainer((0, 0, 50, 50))
line1 = LTTextLineHorizontal(laparams.word_margin)
line1.set_bbox((0, 0, 50, 5))
line2 = LTTextLineHorizontal(laparams.word_margin)
line2.set_bbox((0, 50, 50, 55))
print(len(list(layout.group_textlines(laparams, [line1, line2]))))
`;
  return Number(pdfminerPython(code));
}

function pdfminerNeighborLabels(type: LayoutLineLikePdfminerType): string[] {
  const cls = type === "textlinehorizontal" ? "LTTextLineHorizontal" : "LTTextLineVertical";
  const specs =
    type === "textlinehorizontal"
      ? [
          ["line", [10, 4, 20, 6]],
          ["left_aligned_above", [10, 6, 15, 8]],
          ["right_aligned_below", [15, 2, 20, 4]],
          ["centrally_aligned_overlapping", [13, 5, 17, 7]],
          ["not_aligned", [0, 6, 5, 8]],
          ["wrong_height", [10, 6, 15, 10]]
        ]
      : [
          ["line", [4, 10, 6, 20]],
          ["bottom_aligned_right", [6, 10, 8, 15]],
          ["top_aligned_left", [2, 15, 4, 20]],
          ["centrally_aligned_overlapping", [5, 13, 7, 17]],
          ["not_aligned", [6, 0, 8, 5]],
          ["wrong_width", [6, 10, 10, 15]]
        ];
  const code = `
import json
from pdfminer.layout import LAParams, ${cls}
from pdfminer.utils import Plane

laparams = LAParams()
plane = Plane((0, 0, 50, 50))
lines = []
for label, bbox in json.loads(${JSON.stringify(JSON.stringify(specs))}):
    line = ${cls}(laparams.word_margin)
    line.set_bbox(tuple(bbox))
    plane.add(line)
    lines.append((label, line))

neighbors = lines[0][1].find_neighbors(plane, laparams.line_margin)
print(json.dumps(sorted(label for label, line in lines if line in neighbors)))
`;
  return JSON.parse(pdfminerPython(code)) as string[];
}

function pdfObjectFromPdfBBox(objectType: string, bbox: BBox, pageHeight = neighborPlaneHeight, text?: string): PDFObject {
  const [x0, y0, x1, y1] = bbox;
  return {
    object_type: objectType,
    page_number: 1,
    text,
    x0,
    x1,
    y0,
    y1,
    top: pageHeight - y1,
    bottom: pageHeight - y0,
    doctop: pageHeight - y1,
    width: x1 - x0,
    height: y1 - y0
  };
}

function lineFromPdfBBox(type: LayoutLineLikePdfminerType, label: string, bbox: BBox): NeighborLine {
  return { type, label, obj: pdfObjectFromPdfBBox(type, bbox) };
}

function neighborLines(type: LayoutLineLikePdfminerType): NeighborLine[] {
  return type === "textlinehorizontal"
    ? [
        lineFromPdfBBox(type, "line", [10, 4, 20, 6]),
        lineFromPdfBBox(type, "left_aligned_above", [10, 6, 15, 8]),
        lineFromPdfBBox(type, "right_aligned_below", [15, 2, 20, 4]),
        lineFromPdfBBox(type, "centrally_aligned_overlapping", [13, 5, 17, 7]),
        lineFromPdfBBox(type, "not_aligned", [0, 6, 5, 8]),
        lineFromPdfBBox(type, "wrong_height", [10, 6, 15, 10])
      ]
    : [
        lineFromPdfBBox(type, "line", [4, 10, 6, 20]),
        lineFromPdfBBox(type, "bottom_aligned_right", [6, 10, 8, 15]),
        lineFromPdfBBox(type, "top_aligned_left", [2, 15, 4, 20]),
        lineFromPdfBBox(type, "centrally_aligned_overlapping", [5, 13, 7, 17]),
        lineFromPdfBBox(type, "not_aligned", [6, 0, 8, 5]),
        lineFromPdfBBox(type, "wrong_width", [6, 10, 10, 15])
      ];
}

describe("low-level pdfminer layout neighbor compatibility", () => {
  it("does not clip line grouping to a too-small parent bbox like pdfminer", () => {
    const chars = [
      pdfObjectFromPdfBBox("char", [0, 0, 50, 5], parentHeight, "A"),
      pdfObjectFromPdfBBox("char", [0, 50, 50, 55], parentHeight, "B")
    ];

    const expected = pdfminerWrongParentTextboxCount();
    const { objects } = buildLayoutObjects(chars, [], parentHeight, 0, {});

    expect(objects.textboxhorizontal).toHaveLength(expected);
  });

  it.each(["textlinehorizontal", "textlinevertical"] as const)("finds %s neighbors like pdfminer", (type) => {
    const lines = neighborLines(type);
    const actual = findLineNeighborsLikePdfminer(lines[0], lines, 0.5)
      .map((line) => line.label)
      .sort();

    expect(actual).toEqual(pdfminerNeighborLabels(type));
  });
});
