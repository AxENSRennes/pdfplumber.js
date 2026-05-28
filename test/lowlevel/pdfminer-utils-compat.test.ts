import { describe, expect, it } from "vitest";

import {
  formatIntAlphaLikePdfminer,
  formatIntRomanLikePdfminer,
  PlaneLikePdfminer,
  shortenStrLikePdfminer
} from "../../src/utils.js";

interface Box {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

function givenPlaneWithOneObject(objectSize = 50, gridsize = 50): { plane: PlaneLikePdfminer<Box>; obj: Box } {
  const plane = new PlaneLikePdfminer<Box>([0, 0, 100, 100], gridsize);
  const obj = { x0: 0, y0: 0, x1: objectSize, y1: objectSize };
  plane.add(obj);
  return { plane, obj };
}

describe("low-level pdfminer utility compatibility", () => {
  it("shortens strings like pdfminer.utils.shorten_str", () => {
    expect(shortenStrLikePdfminer("Hello there World", 15)).toBe("Hello ... World");
    expect(shortenStrLikePdfminer("Hello World", 50)).toBe("Hello World");
    expect(shortenStrLikePdfminer("Hello World", 5)).toBe("Hello");
  });

  it("formats positive integers like pdfminer alpha and roman helpers", () => {
    expect([1, 2, 26, 27, 28, 26 * 2, 26 * 2 + 1, 26 * 27, 26 * 27 + 1].map(formatIntAlphaLikePdfminer)).toEqual([
      "a",
      "b",
      "z",
      "aa",
      "ab",
      "az",
      "ba",
      "zz",
      "aaa"
    ]);

    expect([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 20, 40, 45, 50, 90, 91, 100].map(formatIntRomanLikePdfminer)).toEqual([
      "i",
      "ii",
      "iii",
      "iv",
      "v",
      "vi",
      "vii",
      "viii",
      "ix",
      "x",
      "xi",
      "xx",
      "xl",
      "xlv",
      "l",
      "xc",
      "xci",
      "c"
    ]);
  });

  it("finds and removes objects like pdfminer.utils.Plane", () => {
    expect(givenPlaneWithOneObject().plane.find([50, 50, 100, 100])).toEqual([]);

    const removed = givenPlaneWithOneObject();
    removed.plane.remove(removed.obj);
    expect(removed.plane.find([0, 0, 100, 100])).toEqual([]);

    const whole = givenPlaneWithOneObject();
    expect(whole.plane.find([0, 0, 100, 100])).toEqual([whole.obj]);

    const small = givenPlaneWithOneObject(1, 100);
    expect(small.plane.find([0, 0, 100, 100])).toEqual([small.obj]);

    const large = givenPlaneWithOneObject(100, 10);
    expect(large.plane.find([0, 0, 100, 100])).toEqual([large.obj]);
  });
});
