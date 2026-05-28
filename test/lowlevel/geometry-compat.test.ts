import { describe, expect, it } from "vitest";

import { applyMatrix, applyMatrixRectLikePdfminer, multiplyMatrix, safeFloatLikePdfminer, safeRectListLikePdfminer, translateMatrixLikePdfminer } from "../../src/utils.js";
import type { Matrix, MutableBBox, Point } from "../../src/types.js";

function expectNumbersClose(actual: readonly number[], expected: readonly number[]): void {
  expect(actual.length).toBe(expected.length);
  for (let index = 0; index < expected.length; index += 1) {
    expect(actual[index]).toBeCloseTo(expected[index], 8);
  }
}

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

describe("low-level pdfminer matrix geometry", () => {
  it("multiplies matrices like pdfminer.utils.mult_matrix", () => {
    const cases: Array<[Matrix, Matrix, Matrix]> = [
      [
        [1, 0, 0, 1, 0, 0],
        [1, 0, 0, 1, 0, 0],
        [1, 0, 0, 1, 0, 0]
      ],
      [
        [1, 2, 3, 2, -4, 1],
        [1, 0, 0, 1, 0, 0],
        [1, 2, 3, 2, -4, 1]
      ],
      [
        [1, 2, 3, 2, -4, 1],
        [3, 4, 1, 2, -2, 1],
        [5, 8, 11, 16, -13, -13]
      ],
      [
        [1, -1, 1, -1, 1, -1],
        [1, 1, 1, 1, 1, 1],
        [0, 0, 0, 0, 1, 1]
      ]
    ];

    for (const [m0, m1, expected] of cases) expect(multiplyMatrix(m1, m0)).toEqual(expected);
  });

  it("translates matrices and applies matrices to points like pdfminer", () => {
    const translateCases: Array<[Matrix, Point, Matrix]> = [
      [
        [1, 2, 3, 2, -4, 1],
        [0, 0],
        [1, 2, 3, 2, -4, 1]
      ],
      [
        [1, 0, 0, 1, 0, 0],
        [12, -32],
        [1, 0, 0, 1, 12, -32]
      ],
      [
        [1, 0, 0, 1, 3, -3],
        [12, -32],
        [1, 0, 0, 1, 15, -35]
      ],
      [
        [2, 0, 0, 2, 0, 0],
        [1, -1],
        [2, 0, 0, 2, 2, -2]
      ],
      [
        [0, 1, -1, 0, 0, 0],
        [1, 0],
        [0, 1, -1, 0, 0, 1]
      ],
      [
        [0, 1, -1, 0, 0, 0],
        [0, 1],
        [0, 1, -1, 0, -1, 0]
      ]
    ];
    const pointCases: Array<[Matrix, Point, Point]> = [
      [
        [1, 0, 0, 1, 0, 0],
        [0, 0],
        [0, 0]
      ],
      [
        [1, 0, 0, 1, 0, 0],
        [33, 21],
        [33, 21]
      ],
      [
        [1, 2, 3, 2, -4, 1],
        [0, 0],
        [-4, 1]
      ]
    ];

    for (const [matrix, point, expected] of translateCases) expect(translateMatrixLikePdfminer(matrix, point)).toEqual(expected);
    for (const [matrix, point, expected] of pointCases) expect(applyMatrix(point, matrix)).toEqual(expected);
  });

  it("applies matrices to outside rectangles like pdfminer.utils.apply_matrix_rect", () => {
    const rotate10: Matrix = [Math.cos(Math.PI / 18), Math.sin(Math.PI / 18), -Math.sin(Math.PI / 18), Math.cos(Math.PI / 18), 0, 0];
    const skewA: Matrix = [1, Math.tan(5 * Math.PI / 180), Math.tan(7 * Math.PI / 180), 1, 0, 0];
    const skewB: Matrix = [1, Math.tan(-11 * Math.PI / 180), Math.tan(-9 * Math.PI / 180), 1, 0, 0];
    const cases: Array<[Matrix, MutableBBox, MutableBBox]> = [
      [
        [1, 0, 0, 1, 0, 0],
        [0, 0, 100, 200],
        [0, 0, 100, 200]
      ],
      [
        [1, 0, 0, 1, 0, 0],
        [20, 30, 40, 50],
        [20, 30, 40, 50]
      ],
      [
        [1, 0, 0, 1, 5, 0],
        [0, 1, 2, 3],
        [5, 1, 7, 3]
      ],
      [
        [1, 0, 0, 1, 0, 7],
        [0, 2, 4, 6],
        [0, 9, 4, 13]
      ],
      [
        [2, 0, 0, 1, 0, 0],
        [0, 1, 2, 3],
        [0, 1, 4, 3]
      ],
      [
        [1, 0, 0, 2, 0, 0],
        [0, 1, 2, 3],
        [0, 2, 2, 6]
      ],
      [
        [0, 1, 1, 0, 0, 0],
        [3, 4, 7, 6],
        [4, 3, 6, 7]
      ],
      [
        [-1, 0, 0, -1, 0, 0],
        [3, 4, 7, 6],
        [-7, -6, -3, -4]
      ],
      [
        [0, -1, 1, 0, 0, 0],
        [3, 4, 7, 6],
        [4, -7, 6, -3]
      ],
      [rotate10, [3, 4, 7, 6], [1.91253419, 4.46017555, 6.19906156, 7.12438376]],
      [skewA, [3, 4, 7, 6], [3.4911382436116183, 4.262465990577772, 7.736707365417428, 6.612420644681468]],
      [skewB, [3, 4, 7, 6], [2.0496933580527825, 2.6393378360359705, 6.366462238701855, 5.416859072586845]]
    ];

    for (const [matrix, rect, expected] of cases) expectNumbersClose(applyMatrixRectLikePdfminer(matrix, rect), expected);
  });
});
