import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { open } from "../../src/index.js";
import {
  extractPageContent,
  parseFontRecords,
  parseImageResources,
  parsePathOps,
  parsePdfObjects,
  parseTextMatrixOps,
  parseTransformOps,
  resolvePageBoxes
} from "../../src/pdf.js";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "../..");

function rawPdf(relativePath: string): string {
  return readFileSync(path.join(repoRoot, relativePath), "latin1");
}

function firstPage(objects: Map<number, string>): string {
  const page = [...objects.values()].find((object) => /\/Type\s*\/Page\b/.test(object) && !/\/Type\s*\/Pages\b/.test(object));
  if (!page) throw new Error("fixture has no page object");
  return page;
}

describe("low-level parsing against imported PDF fixtures", () => {
  it("extracts content and font records from small PDF.js text fixtures", () => {
    const identityObjects = parsePdfObjects(rawPdf("pdfjs/test/pdfs/IdentityToUnicodeMap_charCodeOf.pdf"));
    const identityPageContent = extractPageContent(firstPage(identityObjects), identityObjects);

    expect(parseFontRecords(identityObjects)).toMatchObject([{ baseFont: "Times-Roman", subtype: "Type1", hasToUnicode: false }]);
    expect(identityPageContent).toContain("(ABCdef) Tj");

    const trueTypeObjects = parsePdfObjects(rawPdf("pdfjs/test/pdfs/TrueType_without_cmap.pdf"));
    const trueTypeContent = extractPageContent(firstPage(trueTypeObjects), trueTypeObjects);

    expect(parseFontRecords(trueTypeObjects)).toMatchObject([{ baseFont: "NRKWIM+Masis", subtype: "TrueType", hasToUnicode: false }]);
    expect(parseTextMatrixOps(trueTypeContent)).toEqual([[11.6758, 0, 3.608, 11.6758, 10, 20]]);
  });

  it("finds image resources and transforms in a compact bitmap fixture", () => {
    const objects = parsePdfObjects(rawPdf("pdfjs/test/pdfs/bitmap-mmr.pdf"));
    const page = firstPage(objects);
    const content = extractPageContent(page, objects);

    expect(parseTransformOps(content)).toEqual([[399, 0, 0, 400, 0, 0]]);
    expect(parseImageResources(page, objects, content).map((image) => [image.name, image.width, image.height, image.bits])).toEqual([["Im", 399, 400, 1]]);
  });

  it("keeps raw XObject names and metadata when PDF.js emits image masks as inline paint ops", async () => {
    const pdf = await open(path.join(repoRoot, "pdfplumber-python/tests/pdfs/issue-203-decimalize.pdf"));
    try {
      expect(pdf.pages[0].images.slice(0, 3).map((image) => [image.name, image.srcsize, image.bits, image.colorspace])).toEqual([
        ["Im0", [1204, 1718], 8, ["/'DeviceRGB'"]],
        ["Ma0", [8, 25], 1, [null]],
        ["Ma1", [1480, 1630], 1, [null]]
      ]);
    } finally {
      await pdf.close();
    }
  });

  it("uses trailer Info metadata from the latest incremental object revision", async () => {
    const pdf = await open(path.join(repoRoot, "pdfplumber-python/tests/pdfs/issue-1279-example.pdf"));
    try {
      expect(pdf.metadata).toMatchObject({
        Author: "Dan Carter",
        Creator: "Finale 2012",
        Producer: "Adobe Mac PDF Plug-in",
        Title: "018TheVoiceofGod000.mus"
      });
      expect(pdf.metadata.BaseFont).toBeUndefined();
    } finally {
      await pdf.close();
    }
  });

  it("keeps pdfminer byte-repr font names for invalid UTF-8 PDF names", async () => {
    const pdf = await open(path.join(repoRoot, "pdfplumber-python/tests/pdfs/issue-842-example.pdf"));
    try {
      const fontnames = new Set(pdf.pages[0].chars.map((char) => String(char.fontname)));
      expect(fontnames).toContain("b'KXAQYK+\\xba\\xda\\xcc\\xe5'");
      expect(fontnames).toContain("b'DTLNCR+\\xb7\\xc2\\xcb\\xce_GB2312'");
    } finally {
      await pdf.close();
    }
  });

  it("uses ToUnicode CMap control characters before CID fallback", async () => {
    const pdf = await open(path.join(repoRoot, "pdfplumber-python/tests/pdfs/test-punkt.pdf"));
    try {
      expect(pdf.pages[0].chars.at(-1)?.text).toBe("\n");
      expect(await pdf.pages[0].extractText()).toBe("https://dell-research-harvard.github.io/HJDataset/");
      expect((await pdf.pages[0].extractWords({ split_at_punctuation: true })).map((word) => word.text)).toHaveLength(16);
    } finally {
      await pdf.close();
    }
  });

  it("keeps overlapping trailing blanks from splitting the preceding word", async () => {
    const pdf = await open(path.join(repoRoot, "pdfplumber-python/tests/pdfs/pr-136-example.pdf"));
    try {
      const text = await pdf.pages[0].extractText();
      const words = await pdf.pages[0].extractWords();
      expect(text).toContain("Hangzhou,China");
      expect(text).not.toContain("Hangzhou,Chin a");
      expect(words.map((word) => word.text)).toContain("XxRd.,Hangzhou,China");
    } finally {
      await pdf.close();
    }
  }, 15_000);

  it("splits words at pdfplumber's exact x-tolerance boundary", async () => {
    const pdf = await open(path.join(repoRoot, "pdfplumber-python/tests/pdfs/issue-336-example.pdf"));
    try {
      const text = await pdf.pages[0].extractText();
      const words = await pdf.pages[0].extractWords();
      expect(text).toContain("第 7.9条");
      expect(text).toContain("4.0 公顷/处。");
      expect(text).toContain("第 7.10条");
      expect(text).toContain("第 7.11条");
      expect(words).toHaveLength(155);
    } finally {
      await pdf.close();
    }
  });

  it("uses raw subset TimesNewRoman sizes for extra_attrs grouping", async () => {
    const pdf = await open(path.join(repoRoot, "pdfplumber-python/tests/pdfs/issue-192-example.pdf"));
    try {
      const words = await pdf.pages[0].extractWords({ extra_attrs: ["size"], vertical_ttb: false });
      expect(words).toHaveLength(373);
      expect(words.slice(333, 345).map((word) => word.text)).toEqual([
        "A",
        "A",
        "A",
        "Ab",
        "abbabaabbg",
        "A",
        "Ab",
        "abbabaabbg",
        "babbaabab",
        "babbaabab",
        "A",
        "A"
      ]);
    } finally {
      await pdf.close();
    }
  });

  it("classifies filled and stroked axis-aligned closed paths as rects", async () => {
    const pdf = await open(path.join(repoRoot, "pdfplumber-python/tests/pdfs/issue-1147-example.pdf"));
    try {
      expect(pdf.pages[0].curves).toHaveLength(0);
      expect(pdf.pages[0].rects).toHaveLength(35);
    } finally {
      await pdf.close();
    }
  });

  it("restores form graphics state around XObject drawing like pdfminer", async () => {
    const pdf = await open(path.join(repoRoot, "test/fixtures/external-holdout-pdfs/arxiv-attention-1706.03762.pdf"), { pages: [15] });
    try {
      expect(pdf.pages[0].rects.at(-1)?.linewidth).toBe(0);
      expect(pdf.pages[0].rects.at(-2)?.linewidth).toBe(0);
    } finally {
      await pdf.close();
    }
  });

  it("drops trailing move-only subpaths when counting curves like pdfminer", async () => {
    const pdf = await open(path.join(repoRoot, "pdfplumber-python/tests/pdfs/issue-71-duplicate-chars-2.pdf"));
    try {
      expect(pdf.pages[0].curves).toHaveLength(69);
    } finally {
      await pdf.close();
    }
  });

  it("keeps standalone move-only stroked paths as curves", async () => {
    const pdf = await open(path.join(repoRoot, "pdfplumber-python/tests/pdfs/pdffill-demo.pdf"));
    try {
      expect(pdf.pages[6].curves).toHaveLength(37);
      expect(pdf.pages.flatMap((page) => page.curves)).toHaveLength(135);
      expect(pdf.pages[6].rects.at(-1)?.linewidth).toBe(1);
    } finally {
      await pdf.close();
    }
  });

  it("keeps image dimensions at pdfminer bbox precision", async () => {
    const pdf = await open(path.join(repoRoot, "pdfplumber-python/tests/pdfs/chelsea_pdta.pdf"));
    try {
      expect(pdf.pages[7].images).toHaveLength(29);
      expect(pdf.pages[7].images[15].width).toBe(85.1499995);
    } finally {
      await pdf.close();
    }
  });

  it("maps single-glyph symbolic Type1 bullets and avoids JS-only decimal spacing", async () => {
    const publishable = await open(path.join(repoRoot, "test/fixtures/external-pdfs/arxiv-publaynet-1908.07836.pdf"), { pages: [2] });
    try {
      const text = await publishable.pages[0].extractText();
      expect(text).toContain("\u2022 Sorted:");
      expect(text).not.toContain("(cid:15) Sorted:");
    } finally {
      await publishable.close();
    }

    const boj = await open(path.join(repoRoot, "test/fixtures/external-holdout-pdfs/boj-semiannual-2024-jp.pdf"), { pages: [83] });
    try {
      const text = await boj.pages[0].extractText();
      expect(text).toContain("それぞれ 1.9兆円、5.5兆円となっ");
      expect(text).not.toContain("1.9 兆円");
    } finally {
      await boj.close();
    }
  });

  it("suppresses PDF.js-recovered text when all page font objects are missing like pdfminer", async () => {
    const pdf = await open(path.join(repoRoot, "pdfjs/test/pdfs/operator-in-TJ-array.pdf"));
    try {
      expect(pdf.pages[0].chars).toHaveLength(0);
      expect(await pdf.pages[0].extractText()).toBe("");
      expect(await pdf.pages[0].extractWords()).toHaveLength(0);
    } finally {
      await pdf.close();
    }
  });

  it("suppresses Type0 text when pdfminer falls back to an empty unsupported Encoding CMap", async () => {
    const pdf = await open(path.join(repoRoot, "pdfjs/test/pdfs/issue18117.pdf"));
    try {
      expect(pdf.pages[0].chars).toHaveLength(0);
      expect(await pdf.pages[0].extractText()).toBe("");
      expect(await pdf.pages[0].extractWords()).toHaveLength(0);
    } finally {
      await pdf.close();
    }
  });

  it("raises pdfminer-compatible errors for malformed annotation entries", async () => {
    const pdf = await open(path.join(repoRoot, "pdfplumber-python/tests/pdfs/federal-register-2020-17221.pdf"));
    try {
      expect(() => pdf.pages[0].annots).toThrow(/maximum recursion depth exceeded/);
      expect(() => pdf.annots).toThrow(/maximum recursion depth exceeded/);
    } finally {
      await pdf.close();
    }
  });

  it("keeps full-height text clusters as table column edges", async () => {
    const pdf = await open(path.join(repoRoot, "pdfplumber-python/tests/pdfs/senate-expenditures.pdf"));
    try {
      const table = await pdf.pages[0]
        .crop([70.332, 130.986, 420, 509.106])
        .extractTable({ horizontal_strategy: "text", vertical_strategy: "text", min_words_vertical: 20, text_x_tolerance: 1 });
      expect(table).not.toBeNull();
      if (!table) throw new Error("Expected a text-strategy table.");
      expect(table).toHaveLength(54);
      expect(table[0]).toHaveLength(5);
      expect(table.at(-1)).toEqual(["DHAW20190070", "09/09/2019", "CITIBANK - TRAVEL CBA CARD", "08/12/2019", "08/14/2019"]);
    } finally {
      await pdf.close();
    }
  });

  it("keeps page-box normalization stable on an imported pdfplumber fixture", () => {
    const objects = parsePdfObjects(rawPdf("pdfplumber-python/tests/pdfs/page-boxes-example.pdf"));
    const page = firstPage(objects);
    const boxes = resolvePageBoxes({ view: [0, 0, 612, 792], rotate: 0 }, page);

    expect(boxes).toMatchObject({
      bbox: [0, 0, 623.62205, 870.23622],
      mediabox: [0, 0, 623.62205, 870.23622],
      cropbox: [14.17323, 42.51968, 581.10236, 856.06299],
      artbox: [42.51969, 70.86614, 552.75591, 827.71653],
      trimbox: [28.34646, 56.69291, 566.92913, 841.88976],
      width: 623.62205,
      height: 870.23622
    });
    expect(parsePathOps(extractPageContent(page, objects))).toEqual([]);
  });
});
