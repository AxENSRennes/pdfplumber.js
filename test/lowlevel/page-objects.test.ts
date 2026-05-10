import { describe, expect, it } from "vitest";

import { extractPageContent, parseColorSpaceResources, parseImageResources, parsePdfObjects } from "../../src/pdf.js";

describe("low-level PDF page object resolution", () => {
  it("resolves direct, array, and indirect-array /Contents references", () => {
    const raw = [
      "1 0 obj << /Type /Page /Contents 2 0 R >> endobj",
      "2 0 obj << /Length 6 >> stream\n/F1 Tf\nendstream endobj",
      "3 0 obj << /Type /Page /Contents [4 0 R 5 0 R] >> endobj",
      "4 0 obj << /Length 2 >> stream\nq\nendstream endobj",
      "5 0 obj << /Length 2 >> stream\nQ\nendstream endobj",
      "6 0 obj << /Type /Page /Contents 7 0 R >> endobj",
      "7 0 obj [8 0 R 9 0 R] endobj",
      "8 0 obj << /Length 3 >> stream\nBT\nendstream endobj",
      "9 0 obj << /Length 3 >> stream\nET\nendstream endobj"
    ].join("\n");
    const objects = parsePdfObjects(raw);

    expect(extractPageContent(objects.get(1), objects)).toBe("/F1 Tf");
    expect(extractPageContent(objects.get(3), objects)).toBe("q\nQ");
    expect(extractPageContent(objects.get(6), objects)).toBe("BT\nET");
  });

  it("expands nested Form XObjects with their matrices", () => {
    const raw = [
      "1 0 obj",
      "<< /Type /Page /Contents 2 0 R /Resources << /XObject << /Fm1 4 0 R >> >> >>",
      "endobj",
      "2 0 obj << /Length 7 >> stream\n/Fm1 Do\nendstream endobj",
      "4 0 obj",
      "<< /Subtype /Form /Matrix [2 0 0 2 10 20] /Resources << /XObject << /Fm2 5 0 R >> >> /Length 7 >>",
      "stream",
      "/Fm2 Do",
      "endstream",
      "endobj",
      "5 0 obj",
      "<< /Subtype /Form /Matrix [1 0 0 1 3 4] /Resources << >> /Length 13 >>",
      "stream",
      "0 0 10 10 re f",
      "endstream",
      "endobj"
    ].join("\n");
    const objects = parsePdfObjects(raw);

    expect(extractPageContent(objects.get(1), objects)).toBe("q\n2 0 0 2 10 20 cm\nq\n1 0 0 1 3 4 cm\n0 0 10 10 re f\nQ\nQ");
  });

  it("collects image resources in draw order, including images inside drawn forms", () => {
    const raw = [
      "1 0 obj",
      "<< /Type /Page /Contents 2 0 R /Resources << /XObject << /Fm1 4 0 R /ImPage 5 0 R >> >> >>",
      "endobj",
      "2 0 obj << /Length 16 >> stream\n/Fm1 Do /ImPage Do\nendstream endobj",
      "4 0 obj << /Subtype /Form /Resources << /XObject << /ImForm 6 0 R >> >> /Length 10 >> stream\n/ImForm Do\nendstream endobj",
      "5 0 obj << /Subtype /Image /Width 20 /Height 30 /BitsPerComponent 8 /ColorSpace /DeviceRGB /Length 0 >> stream\n\nendstream endobj",
      "6 0 obj << /Subtype /Image /Width 2 /Height 3 /BitsPerComponent 1 /ColorSpace /DeviceGray /Length 0 >> stream\n\nendstream endobj"
    ].join("\n");
    const objects = parsePdfObjects(raw);
    const content = extractPageContent(objects.get(1), objects);

    expect(parseImageResources(objects.get(1), objects, content).map((image) => [image.name, image.width, image.height, image.bits])).toEqual([
      ["ImForm", 2, 3, 1],
      ["ImPage", 20, 30, 8]
    ]);
  });

  it("resolves named color spaces from page resources and nested forms", () => {
    const raw = [
      "1 0 obj",
      "<< /Type /Page /Resources << /ColorSpace << /CSPage [/DeviceCMYK] >> /XObject << /Fm1 2 0 R >> >> >>",
      "endobj",
      "2 0 obj << /Subtype /Form /Resources << /ColorSpace << /CSForm 3 0 R >> >> /Length 0 >> stream\n\nendstream endobj",
      "3 0 obj [ /Indexed /DeviceRGB 1 <000000ffffff> ] endobj"
    ].join("\n");
    const objects = parsePdfObjects(raw);

    expect(parseColorSpaceResources(objects.get(1), objects)).toEqual({ CSPage: "DeviceCMYK", CSForm: "Indexed" });
  });
});
