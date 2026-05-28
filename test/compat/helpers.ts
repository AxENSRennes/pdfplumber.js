import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { expect } from "vitest";
import { open, type BBox, type PDFPlumberDocument, type PDFPlumberPage } from "../../src/index.js";

export interface GoldenCheck {
  type: string;
  page?: number;
  expected: unknown;
  args?: Record<string, unknown>;
  bbox?: BBox;
  relative?: boolean;
  strict?: boolean;
}

export interface GoldenScenario {
  id: string;
  pdf: string;
  openOptions?: Record<string, unknown>;
  checks: GoldenCheck[];
}

export interface GoldenFile {
  reference: Record<string, unknown>;
  scenarios: GoldenScenario[];
}

const here = path.dirname(fileURLToPath(import.meta.url));
export const repoRoot = path.resolve(here, "../..");
export const goldenPath = path.join(repoRoot, "test/fixtures/goldens/pdfplumber-compat.json");

export function readGoldens(): GoldenFile {
  return JSON.parse(readFileSync(goldenPath, "utf8")) as GoldenFile;
}

async function valueOf<T>(value: T | Promise<T>): Promise<T> {
  return await value;
}

function page(document: PDFPlumberDocument, index = 0): PDFPlumberPage {
  const selected = document.pages[index];
  if (!selected) {
    throw new Error(`Missing page at index ${index}`);
  }
  return selected;
}

function pageGeometry(selectedPage: PDFPlumberPage): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries({
      page_number: selectedPage.page_number,
      width: selectedPage.width,
      height: selectedPage.height,
      bbox: selectedPage.bbox,
      mediabox: selectedPage.mediabox,
      cropbox: selectedPage.cropbox,
      artbox: (selectedPage as PDFPlumberPage & { artbox?: BBox }).artbox,
      bleedbox: (selectedPage as PDFPlumberPage & { bleedbox?: BBox }).bleedbox,
      trimbox: (selectedPage as PDFPlumberPage & { trimbox?: BBox }).trimbox
    }).filter(([, value]) => value !== undefined)
  );
}

function pageObjectCounts(selectedPage: PDFPlumberPage): Record<string, number> {
  return Object.fromEntries(Object.entries(selectedPage.objects).map(([key, value]) => [key, value.length]));
}

function layoutObjectsSummary(selectedPage: PDFPlumberPage): Record<string, unknown> {
  const props = ["textboxhorizontals", "textlinehorizontals", "textboxverticals", "textlineverticals"] as const;
  return {
    object_counts: pageObjectCounts(selectedPage),
    properties: Object.fromEntries(
      props.map((prop) => {
        const items = selectedPage[prop] ?? [];
        return [
          prop,
          {
            count: items.length,
            first_has_text: items.length > 0 && Object.prototype.hasOwnProperty.call(items[0], "text"),
            first_text: items.length > 0 ? items[0].text : null
          }
        ];
      })
    )
  };
}

function itemAt<T>(items: T[], index: number): T | undefined {
  return index < 0 ? items[items.length + index] : items[index];
}

function textLine(text: string, index: number): string | undefined {
  return itemAt(text.split("\n"), index);
}

function tableCellLine(table: Array<Array<string | null>> | null, row: number, col: number, index: number): string | null | undefined {
  const selectedRow = table ? itemAt(table, row) : undefined;
  const value = selectedRow ? itemAt(selectedRow, col) : undefined;
  return value == null ? value : textLine(value, index);
}

function tableSampleSummary(table: Array<Array<string | null>> | null, cells: number[][]): Record<string, unknown> {
  return {
    row_count: table?.length ?? 0,
    column_count: table?.[0]?.length ?? 0,
    cells: Object.fromEntries(
      cells.map(([row, col]) => {
        const selectedRow = table ? itemAt(table, row) : undefined;
        return [`${row},${col}`, selectedRow ? itemAt(selectedRow, col) : undefined];
      })
    )
  };
}

function nicsPlainTableSummary(table: Array<Array<string | null>> | null): Record<string, unknown> {
  if (!table?.length) {
    return {
      row_count: 0,
      column_count: 0,
      all_columns_match_double_total: false,
      sample_column_checks: {}
    };
  }
  const parseValue = (index: number, value: string | null): number | string | null => {
    if (index === 0) return value;
    if (value == null || value === "") return null;
    return Number(value.replaceAll(",", ""));
  };
  const parsed = table.map((row) => row.map((value, index) => parseValue(index, value)));
  const columnChecks: Record<string, Record<string, unknown>> = {};
  for (let index = 1; index < parsed[0].length; index += 1) {
    const total = Number(parsed[parsed.length - 1][index] ?? 0);
    const colsum = parsed.reduce((sum, row) => sum + Number(row[index] ?? 0), 0);
    columnChecks[String(index)] = {
      total,
      colsum,
      matches_double_total: colsum === total * 2
    };
  }
  return {
    row_count: table.length,
    column_count: table[0].length,
    all_columns_match_double_total: Object.values(columnChecks).every((value) => value.matches_double_total),
    sample_column_checks: Object.fromEntries(["1", "22", "24"].filter((key) => key in columnChecks).map((key) => [key, columnChecks[key]]))
  };
}

function tablesEqual(a: Array<Array<string | null>>, b: Array<Array<string | null>>): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

async function nicsExplicitHorizontalSummary(selectedPage: PDFPlumberPage): Promise<Record<string, unknown>> {
  const cropped = selectedPage.crop([0, 80, selectedPage.width, 475]);
  const table = (await valueOf(cropped.findTables({ horizontal_strategy: "text", vertical_strategy: "text" })))[0];
  const rows = table.rows as Array<Array<BBox | null>>;
  const hPositions = rows.map((row) => row[0]?.[1] ?? 0);
  hPositions.push(rows[rows.length - 1]?.[0]?.[3] ?? 0);
  const explicit = (await valueOf(cropped.findTables({
    horizontal_strategy: "explicit",
    vertical_strategy: "text",
    explicit_horizontal_lines: hPositions
  })))[0];
  const hObjects = hPositions.map((top) => ({
    x0: 0,
    x1: selectedPage.width,
    width: selectedPage.width,
    top,
    bottom: top,
    object_type: "line"
  }));
  const explicitObjects = (await valueOf(cropped.findTables({
    horizontal_strategy: "explicit",
    vertical_strategy: "text",
    explicit_horizontal_lines: hObjects
  })))[0];
  const base = table.extract();
  return {
    h_count: hPositions.length,
    first_h: hPositions[0],
    last_h: hPositions[hPositions.length - 1],
    shape: [base.length, base[0]?.length ?? 0],
    numbers_equal: tablesEqual(base, explicit.extract()),
    objects_equal: tablesEqual(base, explicitObjects.extract()),
    samples: {
      "0,0": base[0]?.[0],
      "0,22": base[0]?.[22],
      "-1,0": base[base.length - 1]?.[0],
      "-1,22": base[base.length - 1]?.[22]
    }
  };
}

function firstWordCharsSummary(words: Array<Record<string, unknown>>): Record<string, unknown> {
  const first = words[0] ?? {};
  const chars = first.chars;
  return {
    first_has_chars: Object.prototype.hasOwnProperty.call(first, "chars"),
    first_text: first.text,
    first_chars_text: Array.isArray(chars) ? chars.map((char) => (char as Record<string, unknown>).text ?? "").join("") : null,
    first_chars_count: Array.isArray(chars) ? chars.length : null
  };
}

async function dedupeExtraAttrsLines(selectedPage: PDFPlumberPage): Promise<Record<string, string[]>> {
  const specs: Array<[string, string[] | null]> = [
    ["no_dedupe", null],
    ["none", []],
    ["size", ["size"]],
    ["fontname", ["fontname"]],
    ["size_fontname", ["size", "fontname"]]
  ];
  const out: Record<string, string[]> = {};
  for (const [name, extraAttrs] of specs) {
    const page = extraAttrs === null ? selectedPage : selectedPage.dedupeChars({ tolerance: 2, extra_attrs: extraAttrs });
    out[name] = (await valueOf(page.extractText({ y_tolerance: 5 }))).split("\n");
  }
  return out;
}

function ctmSummary(char: Record<string, unknown>): Record<string, unknown> {
  const matrix = char.matrix;
  if (!Array.isArray(matrix) || matrix.length !== 6) {
    return { matrix };
  }

  const [a, b, c, d, e, f] = matrix.map(Number);
  return {
    matrix,
    translation_x: e,
    translation_y: f,
    skew_x: (Math.atan2(d, c) * 180) / Math.PI - 90,
    skew_y: (Math.atan2(b, a) * 180) / Math.PI,
    scale_x: Math.sqrt(a ** 2 + b ** 2),
    scale_y: Math.sqrt(c ** 2 + d ** 2)
  };
}

function roundTripComparable(value: unknown): unknown {
  return JSON.parse(JSON.stringify(value));
}

function expectClose(actual: unknown, expected: unknown, precision = 5): void {
  if (typeof actual === "number" && typeof expected === "number") {
    expect(actual).toBeCloseTo(expected, precision);
    return;
  }

  if (Array.isArray(actual) && Array.isArray(expected)) {
    expect(actual.length).toBe(expected.length);
    for (let i = 0; i < expected.length; i += 1) {
      expectClose(actual[i], expected[i], precision);
    }
    return;
  }

  if (
    actual !== null &&
    expected !== null &&
    typeof actual === "object" &&
    typeof expected === "object" &&
    !Array.isArray(actual) &&
    !Array.isArray(expected)
  ) {
    const actualObj = actual as Record<string, unknown>;
    const expectedObj = expected as Record<string, unknown>;
    expect(Object.keys(actualObj)).toEqual(expect.arrayContaining(Object.keys(expectedObj)));
    for (const key of Object.keys(expectedObj)) {
      expectClose(actualObj[key], expectedObj[key], precision);
    }
    return;
  }

  expect(actual).toEqual(expected);
}

export async function runScenario(scenario: GoldenScenario): Promise<void> {
  const pdfPath = path.join(repoRoot, "pdfplumber-python/tests/pdfs", scenario.pdf);
  const document = await open(pdfPath, scenario.openOptions ?? {});

  try {
    for (const check of scenario.checks) {
      const selectedPage = page(document, check.page ?? 0);
      let actual: unknown;

      switch (check.type) {
        case "pdf.pageCount":
          actual = document.pages.length;
          break;
        case "pdf.metadata":
          actual = document.metadata;
          break;
        case "pdf.objectCounts":
          actual = Object.fromEntries(Object.entries(document.objects).map(([key, value]) => [key, value.length]));
          break;
        case "pdf.annots.count":
          actual = document.annots.length;
          break;
        case "pdf.hyperlinks.count":
          actual = document.hyperlinks.length;
          break;
        case "pdf.edgeCounts":
          actual = {
            vertical_edges: document.vertical_edges.length,
            horizontal_edges: document.horizontal_edges.length,
            edges: document.edges.length
          };
          break;
        case "page.geometry":
          actual = pageGeometry(selectedPage);
          break;
        case "page.objectCounts":
          actual = pageObjectCounts(selectedPage);
          break;
        case "page.layoutObjectsSummary":
          actual = layoutObjectsSummary(selectedPage);
          break;
        case "page.annots.count":
          actual = selectedPage.annots.length;
          break;
        case "page.hyperlinks.count":
          actual = selectedPage.hyperlinks.length;
          break;
        case "page.edgeCounts":
          actual = {
            rect_edges: selectedPage.rect_edges.length,
            curve_edges: selectedPage.curve_edges.length,
            edges: selectedPage.edges.length
          };
          break;
        case "page.chars.sample":
          actual = selectedPage.chars.slice(0, (check.args?.count as number | undefined) ?? 5);
          break;
        case "page.object": {
          const objectType = String(check.args?.objectType ?? "");
          const index = (check.args?.index as number | undefined) ?? 0;
          actual = selectedPage.objects[objectType]?.[index];
          break;
        }
        case "page.ctmSummary": {
          const index = (check.args?.index as number | undefined) ?? 0;
          actual = ctmSummary(selectedPage.chars[index] as Record<string, unknown>);
          break;
        }
        case "page.extractText":
          actual = await valueOf(selectedPage.extractText(check.args ?? {}));
          break;
        case "page.extractText.line":
          actual = textLine(await valueOf(selectedPage.extractText(check.args ?? {})), (check.args?.line as number | undefined) ?? -1);
          break;
        case "page.dedupe.extractText.line":
          actual = textLine(await valueOf(selectedPage.dedupeChars(check.args ?? {}).extractText(check.args ?? {})), (check.args?.line as number | undefined) ?? -1);
          break;
        case "page.dedupeExtraAttrsLines":
          actual = await dedupeExtraAttrsLines(selectedPage);
          break;
        case "page.filterMinCharSize.extractText": {
          const minSize = Number(check.args?.minSize ?? 0);
          actual = await valueOf(selectedPage.filter((object) => object.object_type !== "char" || Number(object.size) >= minSize).extractText(check.args ?? {}));
          break;
        }
        case "page.filterMinCharSize.objectCounts": {
          const minSize = Number(check.args?.minSize ?? 0);
          actual = pageObjectCounts(selectedPage.filter((object) => object.object_type !== "char" || Number(object.size) >= minSize));
          break;
        }
        case "page.nicsExplicitHorizontalSummary":
          actual = await nicsExplicitHorizontalSummary(selectedPage);
          break;
        case "page.extractWords":
          actual = await valueOf(selectedPage.extractWords(check.args ?? {}));
          break;
        case "page.extractWords.firstCharsSummary":
          actual = firstWordCharsSummary(await valueOf(selectedPage.extractWords(check.args ?? {})) as Array<Record<string, unknown>>);
          break;
        case "page.dedupe.extractWords":
          actual = await valueOf(selectedPage.dedupeChars(check.args ?? {}).extractWords(check.args ?? {}));
          break;
        case "page.search":
          actual = await valueOf(selectedPage.search(String(check.args?.pattern), check.args ?? {}));
          break;
        case "page.extractTable":
          actual = await valueOf(selectedPage.extractTable(check.args ?? {}));
          break;
        case "page.extractTable.cellLine":
          actual = tableCellLine(
            await valueOf(selectedPage.extractTable(check.args ?? {})),
            (check.args?.row as number | undefined) ?? 0,
            (check.args?.col as number | undefined) ?? 0,
            (check.args?.line as number | undefined) ?? -1
          );
          break;
        case "page.dedupe.extractTable.cellLine":
          actual = tableCellLine(
            await valueOf(selectedPage.dedupeChars(check.args ?? {}).extractTable(check.args ?? {})),
            (check.args?.row as number | undefined) ?? 0,
            (check.args?.col as number | undefined) ?? 0,
            (check.args?.line as number | undefined) ?? -1
          );
          break;
        case "page.extractTables":
          actual = await valueOf(selectedPage.extractTables(check.args ?? {}));
          break;
        case "page.crop.extractText":
          actual = await valueOf(selectedPage.crop(check.bbox as BBox, {
            relative: check.relative,
            strict: check.strict
          }).extractText(check.args ?? {}));
          break;
        case "page.crop.geometry":
          actual = pageGeometry(selectedPage.crop(check.bbox as BBox, {
            relative: check.relative,
            strict: check.strict
          }));
          break;
        case "page.crop.objectCounts":
          actual = pageObjectCounts(selectedPage.crop(check.bbox as BBox, {
            relative: check.relative,
            strict: check.strict
          }));
          break;
        case "page.crop.layoutObjectsSummary":
          actual = layoutObjectsSummary(selectedPage.crop(check.bbox as BBox, {
            relative: check.relative,
            strict: check.strict
          }));
          break;
        case "page.crop.annots":
          actual = selectedPage.crop(check.bbox as BBox, {
            relative: check.relative,
            strict: check.strict
          }).annots;
          break;
        case "page.crop.hyperlinks":
          actual = selectedPage.crop(check.bbox as BBox, {
            relative: check.relative,
            strict: check.strict
          }).hyperlinks;
          break;
        case "page.crop.crop.geometry": {
          const base = selectedPage.crop(check.args?.baseBbox as BBox);
          actual = pageGeometry(base.crop(check.bbox as BBox, {
            relative: check.relative,
            strict: check.strict
          }));
          break;
        }
        case "page.crop.crop.objectCounts": {
          const base = selectedPage.crop(check.args?.baseBbox as BBox);
          actual = pageObjectCounts(base.crop(check.bbox as BBox, {
            relative: check.relative,
            strict: check.strict
          }));
          break;
        }
        case "page.withinBbox.geometry":
          actual = pageGeometry(selectedPage.withinBbox(check.bbox as BBox, {
            relative: check.relative,
            strict: check.strict
          }));
          break;
        case "page.crop.withinBbox.geometry": {
          const base = selectedPage.crop(check.args?.baseBbox as BBox);
          actual = pageGeometry(base.withinBbox(check.bbox as BBox, {
            relative: check.relative,
            strict: check.strict
          }));
          break;
        }
        case "page.withinBbox.objectCounts":
          actual = pageObjectCounts(selectedPage.withinBbox(check.bbox as BBox, {
            relative: check.relative,
            strict: check.strict
          }));
          break;
        case "page.withinBbox.extractText":
          actual = await valueOf(selectedPage.withinBbox(check.bbox as BBox, {
            relative: check.relative,
            strict: check.strict
          }).extractText(check.args ?? {}));
          break;
        case "page.crop.filter.objectCounts": {
          const objectType = String(check.args?.objectType ?? "");
          const cropped = selectedPage.crop(check.bbox as BBox, {
            relative: check.relative,
            strict: check.strict
          });
          actual = pageObjectCounts(cropped.filter((object) => object.object_type === objectType));
          break;
        }
        case "page.crop.error": {
          let errorName = null;
          try {
            selectedPage.crop(check.bbox as BBox, {
              relative: check.relative,
              strict: check.strict
            });
          } catch (error) {
            errorName = error instanceof Error ? error.name : typeof error;
          }
          actual = errorName;
          break;
        }
        case "page.crop.crop.error": {
          const base = selectedPage.crop(check.args?.baseBbox as BBox);
          let errorName = null;
          try {
            base.crop(check.bbox as BBox, {
              relative: check.relative,
              strict: check.strict
            });
          } catch (error) {
            errorName = error instanceof Error ? error.name : typeof error;
          }
          actual = errorName;
          break;
        }
        case "page.outsideBbox.extractText":
          actual = await valueOf(selectedPage.outsideBbox(check.bbox as BBox, {
            relative: check.relative,
            strict: check.strict
          }).extractText(check.args ?? {}));
          break;
        case "page.crop.extractTable":
          actual = await valueOf(selectedPage.crop(check.bbox as BBox, {
            relative: check.relative,
            strict: check.strict
          }).extractTable(check.args ?? {}));
          break;
        case "page.crop.extractTableSummary":
          actual = tableSampleSummary(
            await valueOf(selectedPage.crop(check.bbox as BBox, {
              relative: check.relative,
              strict: check.strict
            }).extractTable(check.args ?? {})),
            (check.args?.cells as number[][] | undefined) ?? []
          );
          break;
        case "page.crop.extractTableNumericSummary":
          actual = nicsPlainTableSummary(await valueOf(selectedPage.crop(check.bbox as BBox, {
            relative: check.relative,
            strict: check.strict
          }).extractTable(check.args ?? {})));
          break;
        case "page.annots":
          actual = Array.isArray(check.expected) ? selectedPage.annots.slice(0, check.expected.length) : selectedPage.annots;
          break;
        case "page.hyperlinks":
          actual = Array.isArray(check.expected) ? selectedPage.hyperlinks.slice(0, check.expected.length) : selectedPage.hyperlinks;
          break;
        default:
          throw new Error(`Unknown golden check type: ${check.type}`);
      }

      expectClose(roundTripComparable(actual), check.expected);
    }
  } finally {
    await document.close();
  }
}
