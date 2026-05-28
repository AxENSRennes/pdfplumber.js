import { describe, expect, it } from "vitest";

import { safeFloatLikePdfminer, safeRectListLikePdfminer } from "../../src/utils.js";

describe("low-level pdfminer geometry casting", () => {
  it("casts rect lists like pdfminer.casting.safe_rect_list", () => {
    expect(safeRectListLikePdfminer([0, 0, 0, 0])).toEqual([0, 0, 0, 0]);
    expect(safeRectListLikePdfminer([1, 2, 3, 4])).toEqual([1, 2, 3, 4]);
    expect(safeRectListLikePdfminer([0, 0, 0, null])).toBeNull();
    expect(safeRectListLikePdfminer([0, 0, 0, "0"])).toEqual([0, 0, 0, 0]);
    expect(safeRectListLikePdfminer([])).toBeNull();
    expect(safeRectListLikePdfminer([0, 0, 0])).toBeNull();
    expect(safeRectListLikePdfminer([1, 2, 3, 4, 5])).toEqual([1, 2, 3, 4]);
    expect(safeRectListLikePdfminer(null)).toBeNull();
    expect(safeRectListLikePdfminer({})).toBeNull();
  });

  it("casts floats like pdfminer.casting.safe_float", () => {
    expect(safeFloatLikePdfminer(0)).toBe(0);
    expect(safeFloatLikePdfminer(1)).toBe(1);
    expect(safeFloatLikePdfminer("0")).toBe(0);
    expect(safeFloatLikePdfminer("1.5")).toBe(1.5);
    expect(safeFloatLikePdfminer(null)).toBeNull();
    expect(safeFloatLikePdfminer({})).toBeNull();
    expect(safeFloatLikePdfminer(2 ** 1024)).toBeNull();
  });
});
