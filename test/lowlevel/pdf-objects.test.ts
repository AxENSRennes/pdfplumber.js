import { describe, expect, it } from "vitest";

import { parsePdfObjects } from "../../src/pdf.js";

describe("low-level PDF object parsing", () => {
  it("extracts simple objects with generation numbers", () => {
    const raw = [
      "%PDF-1.7",
      "1 0 obj",
      "<< /Type /Catalog /Pages 2 0 R >>",
      "endobj",
      "2 3 obj",
      "<< /Type /Pages /Count 0 >>",
      "endobj"
    ].join("\n");
    const objects = parsePdfObjects(raw);

    expect(objects.get(1)).toContain("/Type /Catalog");
    expect(objects.get(2)).toContain("2 3 obj");
  });

  it("does not stop at endobj text inside literal strings or streams", () => {
    const raw = [
      "1 0 obj",
      "<< /Title (literal endobj marker) /Length 23 >>",
      "stream",
      "stream bytes say endobj",
      "endstream",
      "endobj",
      "2 0 obj",
      "<< /Next true >>",
      "endobj"
    ].join("\n");
    const objects = parsePdfObjects(raw);

    expect(objects.get(1)).toContain("literal endobj marker");
    expect(objects.get(1)).toContain("stream bytes say endobj");
    expect(objects.get(2)).toContain("/Next true");
  });

  it("expands object streams", () => {
    const objStream = "7 0 8 17 << /Answer 42 >>\n<< /Name /Embedded#20Name >>";
    const raw = [
      "%PDF-1.5",
      "1 0 obj",
      "<< /Type /Catalog /Pages 2 0 R >>",
      "endobj",
      "6 0 obj",
      `<< /Type /ObjStm /N 2 /First 10 /Length ${objStream.length} >>`,
      "stream",
      objStream,
      "endstream",
      "endobj"
    ].join("\n");
    const objects = parsePdfObjects(raw);

    expect(objects.get(7)).toContain("/Answer 42");
    expect(objects.get(8)).toContain("/Name /Embedded#20Name");
  });
});
