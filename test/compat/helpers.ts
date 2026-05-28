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
        case "page.geometry":
          actual = pageGeometry(selectedPage);
          break;
        case "page.objectCounts":
          actual = pageObjectCounts(selectedPage);
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
        case "page.extractText":
          actual = await valueOf(selectedPage.extractText(check.args ?? {}));
          break;
        case "page.extractWords":
          actual = await valueOf(selectedPage.extractWords(check.args ?? {}));
          break;
        case "page.search":
          actual = await valueOf(selectedPage.search(String(check.args?.pattern), check.args ?? {}));
          break;
        case "page.extractTable":
          actual = await valueOf(selectedPage.extractTable(check.args ?? {}));
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
