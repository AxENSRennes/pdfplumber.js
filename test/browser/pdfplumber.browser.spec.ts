import { createServer, type Server } from "node:http";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { expect, test } from "@playwright/test";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "../..");
const fixturePath = path.join(repoRoot, "pdfplumber-python/tests/pdfs/scotus-transcript-p1.pdf");
const featureFixtures: Record<string, string> = {
  transcript: fixturePath,
  annotations: path.join(repoRoot, "test/fixtures/micro-pdfs/annotations-extended.pdf"),
  images: path.join(repoRoot, "test/fixtures/micro-pdfs/images-advanced.pdf"),
  colors: path.join(repoRoot, "test/fixtures/micro-pdfs/graphics-state-colors.pdf"),
  markedTable: path.join(repoRoot, "pdfplumber-python/tests/pdfs/mcid_example.pdf"),
  dedupe: path.join(repoRoot, "pdfplumber-python/tests/pdfs/issue-71-duplicate-chars.pdf"),
  pageBoxes: path.join(repoRoot, "pdfplumber-python/tests/pdfs/page-boxes-example.pdf")
};

interface Summary {
  metadata: Record<string, unknown>;
  pageCount: number;
  page: {
    page_number: number;
    bbox: readonly number[];
    cropbox: readonly number[];
    chars: number;
    words: Array<Record<string, unknown>>;
    search: string[];
    textHead: string;
    objectCounts: Record<string, number>;
  };
}

type PublicSurfaceSummary = Record<string, unknown>;

function clean(value: unknown): unknown {
  if (typeof value === "number") return Number.isFinite(value) ? Number(value.toFixed(3)) : null;
  if (Array.isArray(value)) return value.map(clean);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>)
        .filter(([, item]) => item !== undefined && typeof item !== "function")
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([key, item]) => [key, clean(item)])
    );
  }
  return value;
}

const objectKeys = [
  "object_type",
  "page_number",
  "text",
  "x0",
  "x1",
  "y0",
  "y1",
  "top",
  "bottom",
  "doctop",
  "width",
  "height",
  "fontname",
  "size",
  "adv",
  "upright",
  "direction",
  "matrix",
  "non_stroking_color",
  "stroking_color",
  "linewidth",
  "dash",
  "pts",
  "path",
  "fill",
  "stroke",
  "evenodd",
  "orientation",
  "mcid",
  "tag",
  "uri",
  "title",
  "contents",
  "data",
  "name",
  "srcsize",
  "imagemask",
  "colorspace",
  "bits"
];

function slimObject(object: Record<string, unknown>): Record<string, unknown> {
  return Object.fromEntries(objectKeys.filter((key) => key in object).map((key) => [key, object[key]]));
}

function listSummary(items: Array<Record<string, unknown>> | undefined, limit = 4): Record<string, unknown> {
  const values = items ?? [];
  return clean({
    count: values.length,
    samples: values.slice(0, limit).map(slimObject)
  }) as Record<string, unknown>;
}

function objectCounts(objects: Record<string, unknown[]>): Record<string, number> {
  return Object.fromEntries(Object.entries(objects).sort(([a], [b]) => a.localeCompare(b)).map(([key, value]) => [key, value.length]));
}

function clipTable(value: unknown): unknown {
  if (value == null || !Array.isArray(value)) return clean(value);
  return clean(value.slice(0, 4).map((row) => Array.isArray(row) ? row.slice(0, 5) : row));
}

async function summarizePublicSurface(document: any): Promise<PublicSurfaceSummary> {
  try {
    const page = document.pages[0];
    const halfBox = [0, 0, page.width / 2, page.height / 2] as const;
    const text = page.extractText();
    const words = page.extractWords();
    const tables = page.findTables();
    const extractedTable = page.extractTable();
    const cropped = page.crop(halfBox);
    const within = page.withinBbox(halfBox);
    const outside = page.outsideBbox(halfBox);
    const filtered = page.filter((object: Record<string, unknown>) => object.object_type !== "char");
    const deduped = page.dedupeChars();
    const snakeText = await page.extract_text();
    const snakeWords = await page.extract_words();
    const snakeTextLines = await page.extract_text_lines();
    const snakeWithin = page.within_bbox(halfBox);
    const snakeOutside = page.outside_bbox(halfBox);
    const snakeDeduped = page.dedupe_chars();
    const snakeTables = await page.find_tables();
    const snakeExtractedTable = await page.extract_table();

    return clean({
      metadata: Object.fromEntries(Object.entries(document.metadata).filter(([, value]) => value != null)),
      pageCount: document.pages.length,
      documentObjectCounts: objectCounts(document.objects),
      documentAnnots: listSummary(document.annots),
      documentHyperlinks: listSummary(document.hyperlinks),
      documentEdges: {
        edges: document.edges.length,
        horizontal_edges: document.horizontal_edges.length,
        vertical_edges: document.vertical_edges.length
      },
      page: {
        page_number: page.page_number,
        pageNumber: page.pageNumber,
        label: page.label,
        width: page.width,
        height: page.height,
        bbox: page.bbox,
        mediabox: page.mediabox,
        cropbox: page.cropbox,
        artbox: page.artbox,
        bleedbox: page.bleedbox,
        trimbox: page.trimbox,
        objectCounts: objectCounts(page.objects),
        chars: listSummary(page.chars),
        words: listSummary(words),
        search: listSummary(page.search(/[A-Za-z]+/g)),
        textHead: text.slice(0, 240),
        textLines: page.extractTextLines().slice(0, 4).map((line: Record<string, unknown>) => slimObject(line)),
        rects: listSummary(page.rects),
        lines: listSummary(page.lines),
        curves: listSummary(page.curves),
        edges: {
          rect_edges: page.rect_edges.length,
          curve_edges: page.curve_edges.length,
          edges: page.edges.length,
          horizontal_edges: page.horizontal_edges.length,
          vertical_edges: page.vertical_edges.length
        },
        images: listSummary(page.images),
        annots: listSummary(page.annots),
        hyperlinks: listSummary(page.hyperlinks),
        markedContent: page.chars
          .filter((char: Record<string, unknown>) => char.mcid != null || char.tag != null)
          .slice(0, 8)
          .map((char: Record<string, unknown>) => ({ text: char.text, mcid: char.mcid, tag: char.tag })),
        cropFilterDedupe: {
          crop: { bbox: cropped.bbox, counts: objectCounts(cropped.objects), chars: cropped.chars.length },
          within: { bbox: within.bbox, counts: objectCounts(within.objects), chars: within.chars.length },
          outside: { bbox: outside.bbox, counts: objectCounts(outside.objects), chars: outside.chars.length },
          filter: { counts: objectCounts(filtered.objects), chars: filtered.chars.length },
          dedupe: { counts: objectCounts(deduped.objects), chars: deduped.chars.length }
        },
        tables: {
          findTables: tables.map((table: Record<string, unknown>) => ({
            bbox: table.bbox,
            cellCount: Array.isArray(table.cells) ? table.cells.length : null,
            rowCount: Array.isArray(table.rows) ? table.rows.length : null,
            columnCount: Array.isArray(table.columns) ? table.columns.length : null,
            firstRow: Array.isArray(table.rows) ? { bbox: table.rows[0]?.bbox, cells: table.rows[0]?.cells } : null,
            firstColumn: Array.isArray(table.columns) ? { bbox: table.columns[0]?.bbox, cells: table.columns[0]?.cells } : null
          })),
          extractTable: clipTable(extractedTable)
        },
        snakeCaseAliases: {
          textHead: snakeText.slice(0, 240),
          words: listSummary(snakeWords),
          textLines: snakeTextLines.slice(0, 4).map((line: Record<string, unknown>) => slimObject(line)),
          within: { bbox: snakeWithin.bbox, counts: objectCounts(snakeWithin.objects), chars: snakeWithin.chars.length },
          outside: { bbox: snakeOutside.bbox, counts: objectCounts(snakeOutside.objects), chars: snakeOutside.chars.length },
          dedupe: { counts: objectCounts(snakeDeduped.objects), chars: snakeDeduped.chars.length },
          findTables: snakeTables.map((table: Record<string, unknown>) => ({
            bbox: table.bbox,
            cellCount: Array.isArray(table.cells) ? table.cells.length : null,
            rowCount: Array.isArray(table.rows) ? table.rows.length : null,
            columnCount: Array.isArray(table.columns) ? table.columns.length : null
          })),
          extractTable: clipTable(snakeExtractedTable)
        }
      }
    }) as PublicSurfaceSummary;
  } finally {
    await document.close();
  }
}

async function summarizePageBoxes(document: any): Promise<PublicSurfaceSummary> {
  try {
    const page = document.pages[0];
    return clean({
      pageCount: document.pages.length,
      page: {
        page_number: page.page_number,
        pageNumber: page.pageNumber,
        width: page.width,
        height: page.height,
        bbox: page.bbox,
        mediabox: page.mediabox,
        cropbox: page.cropbox,
        artbox: page.artbox,
        bleedbox: page.bleedbox,
        trimbox: page.trimbox
      }
    }) as PublicSurfaceSummary;
  } finally {
    await document.close();
  }
}

async function summarizeDocument(document: any): Promise<Summary> {
  try {
    const page = document.pages[0];
    return {
      metadata: Object.fromEntries(Object.entries(document.metadata).filter(([, value]) => value != null)),
      pageCount: document.pages.length,
      page: {
        page_number: page.page_number,
        bbox: page.bbox,
        cropbox: page.cropbox,
        chars: page.chars.length,
        words: page.extractWords().slice(0, 8).map((word: Record<string, unknown>) => ({
          text: word.text,
          x0: word.x0,
          top: word.top,
          x1: word.x1,
          bottom: word.bottom
        })),
        search: page.search("Final").map((match: Record<string, unknown>) => match.text),
        textHead: page.extractText().slice(0, 240),
        objectCounts: Object.fromEntries(Object.entries(page.objects).map(([key, value]) => [key, (value as unknown[]).length]))
      }
    };
  } finally {
    await document.close();
  }
}

async function nodeExpected(): Promise<Summary> {
  const { open } = await import(pathToFileURL(path.join(repoRoot, "dist/src/index.js")).href);
  return summarizeDocument(await open(fixturePath));
}

async function nodePublicSurfaceExpected(): Promise<Record<string, PublicSurfaceSummary>> {
  const { open } = await import(pathToFileURL(path.join(repoRoot, "dist/src/index.js")).href);
  const out: Record<string, PublicSurfaceSummary> = {};
  for (const [name, pathname] of Object.entries(featureFixtures)) {
    out[name] = name === "pageBoxes" ? await summarizePageBoxes(await open(pathname)) : await summarizePublicSurface(await open(pathname, name === "dedupe" ? { pages: [1] } : {}));
  }
  return out;
}

async function startServer(): Promise<{ server: Server; baseUrl: string }> {
  const server = createServer(async (request, response) => {
    try {
      const url = new URL(request.url ?? "/", "http://127.0.0.1");
      if (url.pathname === "/dist/browser/index.js") {
        response.setHeader("content-type", "text/javascript");
        response.end(await readFile(path.join(repoRoot, "dist/browser/index.js")));
        return;
      }
      if (url.pathname === "/dist/browser/pdf.worker.js") {
        response.setHeader("content-type", "text/javascript");
        response.end(await readFile(path.join(repoRoot, "dist/browser/pdf.worker.js")));
        return;
      }
      if (url.pathname === "/fixture.pdf") {
        response.setHeader("content-type", "application/pdf");
        response.end(await readFile(fixturePath));
        return;
      }
      const featureMatch = /^\/fixtures\/([^/]+)\.pdf$/.exec(url.pathname);
      if (featureMatch) {
        const pathname = featureFixtures[featureMatch[1]];
        if (pathname) {
          response.setHeader("content-type", "application/pdf");
          response.end(await readFile(pathname));
          return;
        }
      }
      response.statusCode = 404;
      response.end("not found");
    } catch (error) {
      response.statusCode = 500;
      response.end(String(error));
    }
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("Failed to start browser test server");
  return { server, baseUrl: `http://127.0.0.1:${address.port}` };
}

test.describe("browser ESM package", () => {
  test.setTimeout(90_000);

  let server: Server;
  let baseUrl: string;
  let expected: Summary;
  let publicSurfaceExpected: Record<string, PublicSurfaceSummary>;

  test.beforeAll(async () => {
    expected = await nodeExpected();
    publicSurfaceExpected = await nodePublicSurfaceExpected();
    const started = await startServer();
    server = started.server;
    baseUrl = started.baseUrl;
  });

  test.afterAll(async () => {
    if (server) await new Promise<void>((resolve, reject) => server.close((error) => (error ? reject(error) : resolve())));
  });

  test("matches Node public extraction behavior for ArrayBuffer, Uint8Array, Blob, and URL inputs", async ({ page }) => {
    await page.goto(`${baseUrl}/dist/browser/index.js`);
    const actual = await page.evaluate(async ({ moduleUrl, pdfUrl }) => {
      const { open } = await import(moduleUrl);

      async function summarize(input: ArrayBuffer | Uint8Array | Blob | string | URL) {
        const document = await open(input);
        try {
          const page = document.pages[0];
          return {
            metadata: Object.fromEntries(Object.entries(document.metadata).filter(([, value]) => value != null)),
            pageCount: document.pages.length,
            page: {
              page_number: page.page_number,
              bbox: page.bbox,
              cropbox: page.cropbox,
              chars: page.chars.length,
              words: page.extractWords().slice(0, 8).map((word: Record<string, unknown>) => ({
                text: word.text,
                x0: word.x0,
                top: word.top,
                x1: word.x1,
                bottom: word.bottom
              })),
              search: page.search("Final").map((match: Record<string, unknown>) => match.text),
              textHead: page.extractText().slice(0, 240),
              objectCounts: Object.fromEntries(Object.entries(page.objects).map(([key, value]) => [key, (value as unknown[]).length]))
            }
          };
        } finally {
          await document.close();
        }
      }

      const bytes = await (await fetch(pdfUrl)).arrayBuffer();
      return {
        arrayBuffer: await summarize(bytes.slice(0)),
        uint8Array: await summarize(new Uint8Array(bytes.slice(0))),
        blob: await summarize(new Blob([bytes], { type: "application/pdf" })),
        urlObject: await summarize(new URL(pdfUrl)),
        urlString: await summarize(pdfUrl)
      };
    }, { moduleUrl: `${baseUrl}/dist/browser/index.js`, pdfUrl: `${baseUrl}/fixture.pdf` });

    expect(actual.arrayBuffer).toEqual(expected);
    expect(actual.uint8Array).toEqual(expected);
    expect(actual.blob).toEqual(expected);
    expect(actual.urlObject).toEqual(expected);
    expect(actual.urlString).toEqual(expected);
  });

  test("matches Node public extraction behavior across the stable browser surface", async ({ page }) => {
    await page.goto(`${baseUrl}/dist/browser/index.js`);
    const actual = await page.evaluate(async ({ moduleUrl, baseUrl }) => {
      const { open } = await import(moduleUrl);

      function clean(value: unknown): unknown {
        if (typeof value === "number") return Number.isFinite(value) ? Number(value.toFixed(3)) : null;
        if (Array.isArray(value)) return value.map(clean);
        if (value && typeof value === "object") {
          return Object.fromEntries(
            Object.entries(value as Record<string, unknown>)
              .filter(([, item]) => item !== undefined && typeof item !== "function")
              .sort(([a], [b]) => a.localeCompare(b))
              .map(([key, item]) => [key, clean(item)])
          );
        }
        return value;
      }

      const objectKeys = [
        "object_type",
        "page_number",
        "text",
        "x0",
        "x1",
        "y0",
        "y1",
        "top",
        "bottom",
        "doctop",
        "width",
        "height",
        "fontname",
        "size",
        "adv",
        "upright",
        "direction",
        "matrix",
        "non_stroking_color",
        "stroking_color",
        "linewidth",
        "dash",
        "pts",
        "path",
        "fill",
        "stroke",
        "evenodd",
        "orientation",
        "mcid",
        "tag",
        "uri",
        "title",
        "contents",
        "data",
        "name",
        "srcsize",
        "imagemask",
        "colorspace",
        "bits"
      ];

      function slimObject(object: Record<string, unknown>): Record<string, unknown> {
        return Object.fromEntries(objectKeys.filter((key) => key in object).map((key) => [key, object[key]]));
      }

      function listSummary(items: Array<Record<string, unknown>> | undefined, limit = 4): Record<string, unknown> {
        const values = items ?? [];
        return clean({
          count: values.length,
          samples: values.slice(0, limit).map(slimObject)
        }) as Record<string, unknown>;
      }

      function objectCounts(objects: Record<string, unknown[]>): Record<string, number> {
        return Object.fromEntries(Object.entries(objects).sort(([a], [b]) => a.localeCompare(b)).map(([key, value]) => [key, value.length]));
      }

      function clipTable(value: unknown): unknown {
        if (value == null || !Array.isArray(value)) return clean(value);
        return clean(value.slice(0, 4).map((row) => Array.isArray(row) ? row.slice(0, 5) : row));
      }

      async function summarize(input: ArrayBuffer | Uint8Array | Blob | string | URL, options: Record<string, unknown> = {}) {
        const document = await open(input, options);
        try {
          const page = document.pages[0];
          const halfBox = [0, 0, page.width / 2, page.height / 2] as const;
          const text = page.extractText();
          const words = page.extractWords();
          const tables = page.findTables();
          const extractedTable = page.extractTable();
          const cropped = page.crop(halfBox);
          const within = page.withinBbox(halfBox);
          const outside = page.outsideBbox(halfBox);
          const filtered = page.filter((object: Record<string, unknown>) => object.object_type !== "char");
          const deduped = page.dedupeChars();
          const snakeText = await page.extract_text();
          const snakeWords = await page.extract_words();
          const snakeTextLines = await page.extract_text_lines();
          const snakeWithin = page.within_bbox(halfBox);
          const snakeOutside = page.outside_bbox(halfBox);
          const snakeDeduped = page.dedupe_chars();
          const snakeTables = await page.find_tables();
          const snakeExtractedTable = await page.extract_table();

          return clean({
            metadata: Object.fromEntries(Object.entries(document.metadata).filter(([, value]) => value != null)),
            pageCount: document.pages.length,
            documentObjectCounts: objectCounts(document.objects),
            documentAnnots: listSummary(document.annots),
            documentHyperlinks: listSummary(document.hyperlinks),
            documentEdges: {
              edges: document.edges.length,
              horizontal_edges: document.horizontal_edges.length,
              vertical_edges: document.vertical_edges.length
            },
            page: {
              page_number: page.page_number,
              pageNumber: page.pageNumber,
              label: page.label,
              width: page.width,
              height: page.height,
              bbox: page.bbox,
              mediabox: page.mediabox,
              cropbox: page.cropbox,
              artbox: page.artbox,
              bleedbox: page.bleedbox,
              trimbox: page.trimbox,
              objectCounts: objectCounts(page.objects),
              chars: listSummary(page.chars),
              words: listSummary(words),
              search: listSummary(page.search(/[A-Za-z]+/g)),
              textHead: text.slice(0, 240),
              textLines: page.extractTextLines().slice(0, 4).map((line: Record<string, unknown>) => slimObject(line)),
              rects: listSummary(page.rects),
              lines: listSummary(page.lines),
              curves: listSummary(page.curves),
              edges: {
                rect_edges: page.rect_edges.length,
                curve_edges: page.curve_edges.length,
                edges: page.edges.length,
                horizontal_edges: page.horizontal_edges.length,
                vertical_edges: page.vertical_edges.length
              },
              images: listSummary(page.images),
              annots: listSummary(page.annots),
              hyperlinks: listSummary(page.hyperlinks),
              markedContent: page.chars
                .filter((char: Record<string, unknown>) => char.mcid != null || char.tag != null)
                .slice(0, 8)
                .map((char: Record<string, unknown>) => ({ text: char.text, mcid: char.mcid, tag: char.tag })),
              cropFilterDedupe: {
                crop: { bbox: cropped.bbox, counts: objectCounts(cropped.objects), chars: cropped.chars.length },
                within: { bbox: within.bbox, counts: objectCounts(within.objects), chars: within.chars.length },
                outside: { bbox: outside.bbox, counts: objectCounts(outside.objects), chars: outside.chars.length },
                filter: { counts: objectCounts(filtered.objects), chars: filtered.chars.length },
                dedupe: { counts: objectCounts(deduped.objects), chars: deduped.chars.length }
              },
              tables: {
                findTables: tables.map((table: Record<string, unknown>) => ({
                  bbox: table.bbox,
                  cellCount: Array.isArray(table.cells) ? table.cells.length : null,
                  rowCount: Array.isArray(table.rows) ? table.rows.length : null,
                  columnCount: Array.isArray(table.columns) ? table.columns.length : null,
                  firstRow: Array.isArray(table.rows) ? { bbox: table.rows[0]?.bbox, cells: table.rows[0]?.cells } : null,
                  firstColumn: Array.isArray(table.columns) ? { bbox: table.columns[0]?.bbox, cells: table.columns[0]?.cells } : null
                })),
                extractTable: clipTable(extractedTable)
              },
              snakeCaseAliases: {
                textHead: snakeText.slice(0, 240),
                words: listSummary(snakeWords),
                textLines: snakeTextLines.slice(0, 4).map((line: Record<string, unknown>) => slimObject(line)),
                within: { bbox: snakeWithin.bbox, counts: objectCounts(snakeWithin.objects), chars: snakeWithin.chars.length },
                outside: { bbox: snakeOutside.bbox, counts: objectCounts(snakeOutside.objects), chars: snakeOutside.chars.length },
                dedupe: { counts: objectCounts(snakeDeduped.objects), chars: snakeDeduped.chars.length },
                findTables: snakeTables.map((table: Record<string, unknown>) => ({
                  bbox: table.bbox,
                  cellCount: Array.isArray(table.cells) ? table.cells.length : null,
                  rowCount: Array.isArray(table.rows) ? table.rows.length : null,
                  columnCount: Array.isArray(table.columns) ? table.columns.length : null
                })),
                extractTable: clipTable(snakeExtractedTable)
              }
            }
          });
        } finally {
          await document.close();
        }
      }

      async function summarizePageBoxes(input: ArrayBuffer | Blob | string | URL) {
        const document = await open(input);
        try {
          const page = document.pages[0];
          return clean({
            pageCount: document.pages.length,
            page: {
              page_number: page.page_number,
              pageNumber: page.pageNumber,
              width: page.width,
              height: page.height,
              bbox: page.bbox,
              mediabox: page.mediabox,
              cropbox: page.cropbox,
              artbox: page.artbox,
              bleedbox: page.bleedbox,
              trimbox: page.trimbox
            }
          });
        } finally {
          await document.close();
        }
      }

      const names = ["transcript", "annotations", "images", "colors", "markedTable", "dedupe", "pageBoxes"];
      const out: Record<string, unknown> = {};
      for (const name of names) {
        const pdfUrl = `${baseUrl}/fixtures/${name}.pdf`;
        const bytes = await (await fetch(pdfUrl)).arrayBuffer();
        if (name === "markedTable") {
          out[`${name}:arrayBuffer`] = await summarize(bytes.slice(0));
          out[`${name}:blob`] = await summarize(new Blob([bytes], { type: "application/pdf" }));
          out[`${name}:url`] = await summarize(new URL(pdfUrl));
        } else if (name === "pageBoxes") {
          out[name] = await summarizePageBoxes(pdfUrl);
        } else {
          out[name] = await summarize(pdfUrl, name === "dedupe" ? { pages: [1] } : {});
        }
      }
      return out;
    }, { moduleUrl: `${baseUrl}/dist/browser/index.js`, baseUrl });

    expect(actual.transcript).toEqual(publicSurfaceExpected.transcript);
    expect(actual.annotations).toEqual(publicSurfaceExpected.annotations);
    expect(actual.images).toEqual(publicSurfaceExpected.images);
    expect(actual.colors).toEqual(publicSurfaceExpected.colors);
    expect(actual.dedupe).toEqual(publicSurfaceExpected.dedupe);
    expect(actual.pageBoxes).toEqual(publicSurfaceExpected.pageBoxes);
    expect((actual.pageBoxes as any).page).toMatchObject({
      mediabox: [0, 0, 623.622, 870.236],
      cropbox: [14.173, 42.52, 581.102, 856.063],
      artbox: [42.52, 70.866, 552.756, 827.717],
      bleedbox: [0, 0, 623.622, 870.236],
      trimbox: [28.346, 56.693, 566.929, 841.89]
    });
    expect(actual["markedTable:arrayBuffer"]).toEqual(publicSurfaceExpected.markedTable);
    expect(actual["markedTable:blob"]).toEqual(publicSurfaceExpected.markedTable);
    expect(actual["markedTable:url"]).toEqual(publicSurfaceExpected.markedTable);
  });
});
