import { describe, expect, it } from "vitest";

import { extractTextFromChars, WordExtractor } from "../../src/text.js";
import type { PDFObject } from "../../src/types.js";

function char(text: string, x0: number, x1: number): PDFObject {
  return {
    object_type: "char",
    page_number: 1,
    text,
    x0,
    x1,
    y0: 0,
    y1: 10,
    top: 0,
    bottom: 10,
    doctop: 0,
    width: x1 - x0,
    height: 10,
    size: 10,
    upright: true
  };
}

describe("text extraction tolerance parity", () => {
  it("splits a word when the computed gap exceeds x_tolerance like pdfplumber", () => {
    expect(extractTextFromChars([char("1", 399.31, 405.54999999999995), char("兆", 408.55, 421.03)])).toBe("1 兆");
  });

  it("still splits a word when the gap meaningfully exceeds x_tolerance", () => {
    expect(extractTextFromChars([char("1", 399.31, 405.54999999999995), char("兆", 408.5511, 421.03)])).toBe("1 兆");
  });

  it("preserves producer-level spacing just above x_tolerance", () => {
    expect(extractTextFromChars([char("y", 418.7099399560001, 423.7099399560001), char("s", 426.7099400000001, 430.60154003700006)])).toBe("y s");
  });

  it("keeps the final RTL glyph with its word before a whitespace separator", () => {
    const chars = [
      char("A", 50, 55),
      char("B", 45, 50),
      char(" ", 42, 44),
      char("C", 37, 42),
      char("D", 32, 37)
    ];

    const words = new WordExtractor({ horizontal_ltr: false }).extractWords(chars);
    expect(words.map((word) => word.text)).toEqual(["AB", "CD"]);
  });

  it("keeps zero-width rotated blanks as word separators", () => {
    const chars = ["A", " ", "B"].map((text) => ({
      ...char(text, 10, 10),
      width: 0,
      height: 0,
      size: 0,
      upright: false
    }));

    const words = new WordExtractor().extractWords(chars);
    expect(words.map((word) => word.text)).toEqual(["A", "B"]);
  });

  it("keeps zero-width upright blanks after text as word separators", () => {
    const chars = ["A", " ", "B"].map((text) => ({
      ...char(text, 10, 10),
      width: 0,
      height: 0,
      size: 0
    }));

    const words = new WordExtractor().extractWords(chars);
    expect(words.map((word) => word.text)).toEqual(["A", "B"]);
  });
});
