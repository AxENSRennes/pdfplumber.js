import path from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { open } from "../../src/index.js";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "../..");

function fixture(relativePath: string): string {
  return path.join(repoRoot, relativePath);
}

describe("stable public extraction surface", () => {
  it("keeps documented snake_case aliases equivalent to camelCase methods", async () => {
    const transcript = await open(fixture("pdfplumber-python/tests/pdfs/scotus-transcript-p1.pdf"));
    try {
      const page = transcript.pages[0];
      const bbox = [0, 0, page.width / 2, page.height / 2] as const;

      expect(await page.extract_text()).toBe(await page.extractText());
      expect((await page.extract_text_lines()).slice(0, 3)).toEqual((await page.extractTextLines()).slice(0, 3));
      expect((await page.extract_words()).slice(0, 8)).toEqual((await page.extractWords()).slice(0, 8));
      expect(page.within_bbox(bbox).chars.length).toBe(page.withinBbox(bbox).chars.length);
      expect(page.outside_bbox(bbox).chars.length).toBe(page.outsideBbox(bbox).chars.length);
      expect(page.dedupe_chars().chars.length).toBe(page.dedupeChars().chars.length);
    } finally {
      await transcript.close();
    }

    const markedAndTables = await open(fixture("pdfplumber-python/tests/pdfs/mcid_example.pdf"));
    try {
      const page = markedAndTables.pages[0];
      const tableOptions = { vertical_strategy: "lines" as const, horizontal_strategy: "lines" as const };

      expect((await page.find_table(tableOptions))?.bbox).toEqual((await page.findTable(tableOptions))?.bbox);
      expect((await page.find_tables(tableOptions)).map((table) => table.bbox)).toEqual((await page.findTables(tableOptions)).map((table) => table.bbox));
      expect(await page.extract_table(tableOptions)).toEqual(await page.extractTable(tableOptions));
      expect(await page.extract_tables(tableOptions)).toEqual(await page.extractTables(tableOptions));
    } finally {
      await markedAndTables.close();
    }
  });

  it("exposes the documented subsystems through the default open() path", async () => {
    const transcript = await open(fixture("pdfplumber-python/tests/pdfs/scotus-transcript-p1.pdf"));
    try {
      const page = transcript.pages[0];
      expect(transcript.metadata).toMatchObject({ Producer: expect.any(String) });
      expect(page.bbox).toEqual([0, 0, 612, 792]);
      expect(page.cropbox).toEqual(page.bbox);
      expect(page.chars.length).toBeGreaterThan(700);
      expect((await page.extractWords()).length).toBeGreaterThan(100);
      expect(page.extractText()).toContain("Official - Subject to Final Review");
      const finalMatches = await page.search("Final");
      expect(finalMatches.map((match) => match.text)).toEqual(["Final"]);

      const crop = page.crop([0, 0, page.width / 2, page.height / 2]);
      expect(crop.bbox).toEqual([0, 0, 306, 396]);
      expect(page.withinBbox(crop.bbox).chars.length).toBeGreaterThan(0);
      expect(page.outsideBbox(crop.bbox).chars.length).toBeGreaterThan(0);
      expect(page.filter((object) => object.object_type !== "char").chars).toHaveLength(0);
    } finally {
      await transcript.close();
    }

    const pageBoxes = await open(fixture("pdfplumber-python/tests/pdfs/page-boxes-example.pdf"));
    try {
      const page = pageBoxes.pages[0];
      expect(page).toMatchObject({
        bbox: [0, 0, 623.62205, 870.23622],
        mediabox: [0, 0, 623.62205, 870.23622],
        cropbox: [14.17323, 42.51968, 581.10236, 856.06299],
        artbox: [42.51969, 70.86614, 552.75591, 827.71653],
        bleedbox: [0, 0, 623.62205, 870.23622],
        trimbox: [28.34646, 56.69291, 566.92913, 841.88976]
      });
    } finally {
      await pageBoxes.close();
    }

    const colors = await open(fixture("test/fixtures/micro-pdfs/graphics-state-colors.pdf"));
    try {
      const page = colors.pages[0];
      expect(page.rects.length).toBeGreaterThanOrEqual(4);
      expect(page.lines.length).toBeGreaterThanOrEqual(1);
      expect(page.rects.some((rect) => Array.isArray(rect.non_stroking_color) || Array.isArray(rect.stroking_color))).toBe(true);
      expect(page.lines[0]).toMatchObject({ linewidth: expect.any(Number), object_type: "line" });
    } finally {
      await colors.close();
    }

    const vectors = await open(fixture("test/fixtures/micro-pdfs/vector-objects.pdf"));
    try {
      const page = vectors.pages[0];
      expect(vectors.edges.length).toBe(page.edges.length);
      expect(vectors.horizontal_edges.length).toBe(page.horizontal_edges.length);
      expect(vectors.vertical_edges.length).toBe(page.vertical_edges.length);
      expect(page.rects).toHaveLength(1);
      expect(page.lines).toHaveLength(1);
      expect(page.curves).toHaveLength(1);
      expect(page.lines[0].path).toEqual([
        ["m", [10, 130]],
        ["l", [100, 130]]
      ]);
      expect(page.rects[0].path).toEqual([
        ["m", [20, 120]],
        ["l", [60, 120]],
        ["l", [60, 90]],
        ["l", [20, 90]],
        ["h"]
      ]);
      expect(page.curves[0].path).toEqual([
        ["m", [10, 60]],
        ["c", [30, 20], [70, 20], [90, 60]]
      ]);
      expect(page.edges.length).toBeGreaterThanOrEqual(page.rect_edges.length + page.curve_edges.length);
      expect(page.horizontal_edges.length + page.vertical_edges.length).toBe(page.edges.length);
    } finally {
      await vectors.close();
    }

    const annotations = await open(fixture("test/fixtures/micro-pdfs/annotations-extended.pdf"));
    try {
      const page = annotations.pages[0];
      expect(page.annots.length).toBeGreaterThanOrEqual(8);
      expect(page.annots.some((annot) => annot.contents || annot.title || annot.name)).toBe(true);
      expect(page.annots[0].contents).toBeNull();
      expect(page.annots[0].data).toMatchObject({ Subtype: "/'Square'", Rect: [20, 170, 60, 210] });
      expect(page.annots[2].data).toMatchObject({ Subtype: "/'FreeText'", Contents: "b'Free text'" });
    } finally {
      await annotations.close();
    }

    const images = await open(fixture("test/fixtures/micro-pdfs/images-advanced.pdf"));
    try {
      const page = images.pages[0];
      expect(page.images).toHaveLength(2);
      expect(page.images[0]).toMatchObject({
        object_type: "image",
        name: expect.any(String),
        srcsize: expect.any(Array),
        imagemask: null,
        bits: expect.any(Number)
      });
      expect("colorspace" in page.images[0]).toBe(true);
      expect(page.images[1]).toMatchObject({ imagemask: null, bits: 8, colorspace: ["/'RGB'"] });
    } finally {
      await images.close();
    }

    const markedAndTables = await open(fixture("pdfplumber-python/tests/pdfs/mcid_example.pdf"));
    try {
      const page = markedAndTables.pages[0];
      expect(page.chars.some((char) => char.mcid != null && char.tag != null)).toBe(true);
      const tables = await page.findTables({ vertical_strategy: "lines", horizontal_strategy: "lines" });
      expect(tables).toHaveLength(1);
      expect(tables[0].rows[0].bbox).toEqual([105.95, 139.8125, 460.25, 171.45]);
      expect(tables[0].columns[0].bbox).toEqual([105.95, 139.8125, 460.25, 329.655556]);
      expect(page.extractTable({ vertical_strategy: "lines", horizontal_strategy: "lines" })).toEqual(expect.any(Array));
      expect(
        page.findTables({
          vertical_strategy: "explicit",
          horizontal_strategy: "explicit",
          explicit_vertical_lines: page.vertical_edges,
          explicit_horizontal_lines: page.horizontal_edges
        })
      ).toHaveLength(1);
    } finally {
      await markedAndTables.close();
    }

    const dedupe = await open(fixture("pdfplumber-python/tests/pdfs/issue-71-duplicate-chars.pdf"), { pages: [1] });
    try {
      const page = dedupe.pages[0];
      expect(page.dedupeChars().chars.length).toBeLessThan(page.chars.length);
    } finally {
      await dedupe.close();
    }
  });
});
