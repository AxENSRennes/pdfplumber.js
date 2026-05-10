import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { expect } from "vitest";
import { open, type BBox, type PDFObject, type PDFPlumberDocument, type PDFPlumberPage, type Table } from "../../src/index.js";

export interface ParityCheck {
  type: string;
  page?: number;
  pageIndices?: number[];
  expected: unknown;
  options?: Record<string, unknown>;
  bbox?: BBox;
  bboxMethod?: "crop" | "outside" | "within";
  dedupe?: boolean;
  dedupeOptions?: Record<string, unknown>;
  pattern?: string;
}

export interface ParityScenario {
  id: string;
  pdf: string;
  openOptions?: Record<string, unknown>;
  checks: ParityCheck[];
}

export interface ParityGoldenFile {
  reference: Record<string, unknown>;
  coverage: Record<string, unknown>;
  scenarios: ParityScenario[];
}

const here = path.dirname(fileURLToPath(import.meta.url));
export const repoRoot = path.resolve(here, "../..");
export const goldenPath = path.join(repoRoot, "test/fixtures/goldens/pdfplumber-parity.json");

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
  "non_stroking_pattern",
  "stroking_pattern",
  "ncs",
  "linewidth",
  "dash",
  "fill",
  "stroke",
  "evenodd",
  "orientation",
  "mcid",
  "tag",
  "uri",
  "title",
  "contents",
  "name",
  "srcsize",
  "colorspace",
  "bits"
];

const HASH_NUMERIC_DIGITS = 3;

export function readParityGoldens(): ParityGoldenFile {
  return JSON.parse(readFileSync(goldenPath, "utf8")) as ParityGoldenFile;
}

export function readParityGoldensAt(pathname: string): ParityGoldenFile {
  return JSON.parse(readFileSync(pathname, "utf8")) as ParityGoldenFile;
}

function resolvePdfPath(scenario: ParityScenario): string {
  if (scenario.pdf.includes("/") || scenario.pdf.includes("\\")) {
    return path.resolve(repoRoot, scenario.pdf);
  }
  return path.join(repoRoot, "pdfplumber-python/tests/pdfs", scenario.pdf);
}

async function valueOf<T>(value: T | Promise<T>): Promise<T> {
  return await value;
}

function finiteNumber(value: unknown, digits = 6): number | null {
  if (typeof value !== "number") return null;
  if (!Number.isFinite(value)) return null;
  if (digits < 6) {
    const sign = value < 0 ? -1 : 1;
    const fixed = Math.abs(value).toFixed(6);
    const [whole, fraction = ""] = fixed.split(".");
    const kept = fraction.slice(0, digits).padEnd(digits, "0");
    const next = fraction[digits] ?? "0";
    const rest = fraction.slice(digits + 1);
    const lastKept = digits > 0 ? Number(kept[kept.length - 1] ?? "0") : Number(whole[whole.length - 1] ?? "0");
    const roundUp = next > "5" || (next === "5" && (/[1-9]/.test(rest) || lastKept % 2 === 1));
    const scale = 10n ** BigInt(digits);
    let scaled = BigInt(whole) * scale + BigInt(kept || "0");
    if (roundUp) scaled += 1n;
    const rounded = sign * Number(scaled) / Number(scale);
    if (Math.abs(rounded - Math.round(rounded)) < 1e-9) return Math.round(rounded);
    return Object.is(rounded, -0) ? 0 : rounded;
  }
  const factor = 10 ** digits;
  const scaled = Math.abs(value) * factor;
  const floor = Math.floor(scaled);
  const fraction = scaled - floor;
  const roundedInt =
    Math.abs(fraction - 0.5) < 1e-12
      ? floor + (floor % 2 === 0 ? 0 : 1)
      : Math.round(scaled);
  const rounded = Math.sign(value) * roundedInt / factor;
  if (Math.abs(rounded - Math.round(rounded)) < 1e-9) return Math.round(rounded);
  return Object.is(rounded, -0) ? 0 : rounded;
}

function clean(value: unknown, digits = 6): unknown {
  const number = finiteNumber(value, digits);
  if (number !== null || value === null) return number;
  if (Array.isArray(value)) return value.map((item) => clean(item, digits));
  if (typeof value === "object" && value !== null) {
    const out: Record<string, unknown> = {};
    for (const [key, item] of Object.entries(value as Record<string, unknown>)) {
      if (["stream", "data", "graphicstate"].includes(key)) continue;
      if (item === undefined || typeof item === "function" || typeof item === "symbol") continue;
      out[key] = clean(item, digits);
    }
    return out;
  }
  return value;
}

function stableForJson(value: unknown, digits = 6): unknown {
  const cleaned = clean(value, digits);
  if (Array.isArray(cleaned)) return cleaned.map((item) => stableForJson(item, digits));
  if (typeof cleaned === "object" && cleaned !== null) {
    return Object.fromEntries(
      Object.entries(cleaned as Record<string, unknown>)
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([key, item]) => [key, stableForJson(item, digits)])
    );
  }
  return cleaned;
}

function stableJson(value: unknown): string {
  return pythonJson(stableForJson(value));
}

function pythonJsonNumber(value: number): string {
  if (Number.isInteger(value)) return String(value);
  const abs = Math.abs(value);
  if (abs !== 0 && abs < 0.0001) {
    const [mantissa, exponent] = value.toExponential(15).split("e");
    const trimmedMantissa = mantissa.replace(/(\.\d*?)0+$/, "$1").replace(/\.$/, "");
    const exponentValue = Number(exponent);
    const sign = exponentValue < 0 ? "-" : "+";
    const digits = String(Math.abs(exponentValue)).padStart(2, "0");
    return `${trimmedMantissa}e${sign}${digits}`;
  }
  return value.toFixed(6).replace(/(\.\d*?)0+$/, "$1").replace(/\.$/, "");
}

function pythonJson(value: unknown): string {
  if (value === null) return "null";
  if (typeof value === "number") return pythonJsonNumber(value);
  if (typeof value === "string") return JSON.stringify(value);
  if (typeof value === "boolean") return value ? "true" : "false";
  if (Array.isArray(value)) return `[${value.map(pythonJson).join(",")}]`;
  if (typeof value === "object" && value !== null) {
    const entries = Object.entries(value as Record<string, unknown>).sort(([a], [b]) => a.localeCompare(b));
    return `{${entries.map(([key, item]) => `${JSON.stringify(key)}:${pythonJson(item)}`).join(",")}}`;
  }
  return "null";
}

function sha256Text(value: string): string {
  return createHash("sha256").update(value, "utf8").digest("hex");
}

function sha256Value(value: unknown): string {
  return sha256Text(pythonJson(stableForJson(value, HASH_NUMERIC_DIGITS)));
}

function sampleIndices(length: number, limit = 8): number[] {
  if (length <= limit) return Array.from({ length }, (_, i) => i);
  return Array.from(new Set([0, 1, Math.floor(length / 2) - 1, Math.floor(length / 2), Math.floor(length / 2) + 1, length - 2, length - 1]))
    .filter((i) => i >= 0 && i < length)
    .sort((a, b) => a - b);
}

function slimObj(obj: PDFObject | Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const key of objectKeys) {
    if (key in obj) out[key] = (obj as Record<string, unknown>)[key];
  }
  return clean(out) as Record<string, unknown>;
}

function objectListSummary(objects: Array<PDFObject | Record<string, unknown>>, limit = 8): Record<string, unknown> {
  const slimmed = objects.map((obj) => slimObj(obj));
  const indices = sampleIndices(slimmed.length, limit);
  return {
    count: slimmed.length,
    sha256: sha256Value(slimmed),
    sampleIndices: indices,
    samples: indices.map((i) => slimmed[i])
  };
}

function slimSearchResult(result: Record<string, unknown>): Record<string, unknown> {
  const slimmed = slimObj(result);
  if (Array.isArray(result.chars)) {
    slimmed.chars = objectListSummary(result.chars as PDFObject[], 5);
  }
  return slimmed;
}

function searchSummary(results: unknown[]): Record<string, unknown> {
  const slimmed = results.map((result) => slimSearchResult(result as Record<string, unknown>));
  const indices = sampleIndices(slimmed.length, 8);
  return {
    count: slimmed.length,
    sha256: sha256Value(slimmed),
    sampleIndices: indices,
    samples: indices.map((i) => slimmed[i])
  };
}

function textSummary(text: string | null | undefined): Record<string, unknown> {
  const value = text ?? "";
  return {
    length: [...value].length,
    lineCount: value ? value.split("\n").length : 0,
    sha256: sha256Text(value),
    head: [...value].slice(0, 240).join(""),
    tail: [...value].length > 240 ? [...value].slice(-240).join("") : value
  };
}

function tableShapes(value: unknown): unknown {
  if (value === null) return null;
  if (!Array.isArray(value)) return null;
  if (!value.length) return [];
  if (value.every((row) => Array.isArray(row) && (!row.length || !Array.isArray(row[0])))) {
    return [value.length, Math.max(0, ...value.map((row) => row.length))];
  }
  return value.map((table) => tableShapes(table));
}

function clippedTable(value: unknown, limit = 3): unknown {
  if (value === null || !Array.isArray(value)) return clean(value);
  if (value.length <= limit * 2) return clean(value);
  return clean([...value.slice(0, limit), ["..."], ...value.slice(-limit)]);
}

function jsonKind(value: unknown): string {
  if (value === null) return "null";
  if (Array.isArray(value)) return "list";
  switch (typeof value) {
    case "object":
      return "dict";
    case "string":
      return "str";
    case "number":
      return "float";
    case "boolean":
      return "bool";
    default:
      return typeof value;
  }
}

function jsonValueSummary(value: unknown): Record<string, unknown> {
  const cleaned = clean(value);
  const encoded = stableJson(cleaned);
  return {
    kind: jsonKind(cleaned),
    shape: tableShapes(cleaned),
    jsonLength: [...encoded].length,
    sha256: sha256Text(encoded),
    sample: clippedTable(cleaned)
  };
}

function findTablesSummary(tables: Table[]): Record<string, unknown> {
  const summaries = tables.map((table) => {
    const rowShapes = table.rows.slice(0, 6).map((row) => row.length);
    const columnCount = table.rows.reduce((max, row) => Math.max(max, row.length), 0);
    return clean({
      bbox: table.bbox,
      cellCount: table.cells.length,
      rowCount: table.rows.length,
      columnCount,
      rowShapes
    });
  });
  return {
    count: summaries.length,
    sha256: sha256Value(summaries),
    tables: summaries
  };
}

function metadataSubset(metadata: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const key of ["Author", "Creator", "Producer", "Title", "Subject", "Keywords", "CreationDate", "ModDate", "Copies"]) {
    if (key in metadata) out[key] = metadata[key];
  }
  return clean(out) as Record<string, unknown>;
}

function objectCounts(objects: Record<string, PDFObject[]>): Record<string, number> {
  return Object.fromEntries(Object.entries(objects).sort(([a], [b]) => a.localeCompare(b)).map(([key, value]) => [key, value.length]));
}

function pageGeometry(page: PDFPlumberPage): Record<string, unknown> {
  const out: Record<string, unknown> = {
    page_number: page.page_number,
    width: page.width,
    height: page.height,
    bbox: page.bbox,
    mediabox: page.mediabox,
    cropbox: page.cropbox
  };
  for (const key of ["artbox", "bleedbox", "trimbox"] as const) {
    if (page[key] !== undefined) out[key] = page[key];
  }
  return clean(out) as Record<string, unknown>;
}

async function withStatus<T>(fn: () => T | Promise<T>, summarize: (value: T) => unknown = (value) => clean(value)): Promise<Record<string, unknown>> {
  try {
    return { status: "ok", value: summarize(await valueOf(fn())) };
  } catch (error) {
    return {
      status: "error",
      errorType: error instanceof Error ? error.name : typeof error,
      message: error instanceof Error ? error.message.slice(0, 240) : String(error).slice(0, 240)
    };
  }
}

async function pageSnapshot(page: PDFPlumberPage): Promise<Record<string, unknown>> {
  const objectSamples: Record<string, unknown> = {};
  for (const key of [
    "char",
    "line",
    "rect",
    "curve",
    "image",
    "annot",
    "textboxhorizontal",
    "textlinehorizontal",
    "textboxvertical",
    "textlinevertical"
  ]) {
    const objects = page.objects[key];
    if (objects) objectSamples[key] = objectListSummary(objects);
  }
  return {
    geometry: pageGeometry(page),
    objectCounts: objectCounts(page.objects),
    edgeCounts: {
      rect_edges: page.rect_edges.length,
      curve_edges: page.curve_edges.length,
      edges: page.edges.length
    },
    objectSamples,
    annots: objectListSummary(page.annots),
    hyperlinks: objectListSummary(page.hyperlinks),
    extractText: await withStatus(() => page.extractText(), textSummary),
    extractWords: await withStatus(() => page.extractWords(), objectListSummary)
  };
}

async function documentSnapshot(
  pdfPath: string,
  openOptions: Record<string, unknown> = {},
  pageIndices?: number[]
): Promise<Record<string, unknown>> {
  let document: PDFPlumberDocument;
  try {
    document = await open(pdfPath, openOptions);
  } catch (error) {
    return {
      status: "error",
      errorType: error instanceof Error ? error.name : typeof error,
      message: error instanceof Error ? error.message.slice(0, 240) : String(error).slice(0, 240)
    };
  }

  try {
    const selectedIndices =
      pageIndices ?? Array.from({ length: document.pages.length }, (_, index) => index);
    const pages = [];
    for (const index of selectedIndices) {
      const page = document.pages[index];
      if (!page) throw new Error(`Missing page at index ${index}`);
      pages.push(await pageSnapshot(page));
    }
    const snapshot: Record<string, unknown> = {
      status: "ok",
      metadata: metadataSubset(document.metadata),
      pageCount: document.pages.length,
      ...(pageIndices ? { pageIndices: selectedIndices } : {}),
      pages
    };
    if (!pageIndices) {
      snapshot.objectCounts = objectCounts(document.objects);
      snapshot.annots = objectListSummary(document.annots);
      snapshot.hyperlinks = objectListSummary(document.hyperlinks);
    }
    return snapshot;
  } finally {
    await document.close();
  }
}

function scenarioPage(document: PDFPlumberDocument, check: ParityCheck): PDFPlumberPage {
  const selected = document.pages[check.page ?? 0];
  if (!selected) throw new Error(`Missing page at index ${check.page ?? 0}`);
  let page = selected;
  if (check.bbox) {
    if (check.bboxMethod === "outside") page = page.outsideBbox(check.bbox);
    else if (check.bboxMethod === "within") page = page.withinBbox(check.bbox);
    else page = page.crop(check.bbox);
  }
  if (check.dedupe) page = page.dedupe_chars(check.dedupeOptions ?? {});
  return page;
}

function buildMcidText(page: PDFPlumberPage): Array<Record<string, unknown>> {
  const mcids: string[] = [];
  for (const char of page.chars) {
    if (typeof char.mcid !== "number") continue;
    while (mcids.length <= char.mcid) mcids.push("");
    if (!mcids[char.mcid]) mcids[char.mcid] = `${String(char.tag)}: `;
    mcids[char.mcid] += String(char.text ?? "");
  }
  return mcids.map((text, mcid) => ({ mcid, text }));
}

function expectClose(actual: unknown, expected: unknown, precision = 5, path = "$"): void {
  if (
    expected !== null &&
    typeof expected === "object" &&
    !Array.isArray(expected) &&
    (expected as Record<string, unknown>).status === "error"
  ) {
    expect((actual as Record<string, unknown>).status, path).toBe("error");
    return;
  }

  if (typeof actual === "number" && typeof expected === "number") {
    expect(actual, path).toBeCloseTo(expected, precision);
    return;
  }

  if (Array.isArray(actual) && Array.isArray(expected)) {
    expect(actual.length, `${path}.length`).toBe(expected.length);
    for (let i = 0; i < expected.length; i += 1) {
      expectClose(actual[i], expected[i], precision, `${path}[${i}]`);
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
    expect(Object.keys(actualObj).sort(), `${path}.keys`).toEqual(Object.keys(expectedObj).sort());
    for (const key of Object.keys(expectedObj)) {
      expectClose(actualObj[key], expectedObj[key], precision, `${path}.${key}`);
    }
    return;
  }

  expect(actual, path).toEqual(expected);
}

export async function runParityScenario(scenario: ParityScenario): Promise<void> {
  const pdfPath = resolvePdfPath(scenario);
  let document: PDFPlumberDocument | null = null;

  const getDocument = async (): Promise<PDFPlumberDocument> => {
    document ??= await open(pdfPath, scenario.openOptions ?? {});
    return document;
  };

  try {
    for (const check of scenario.checks) {
      let actual: unknown;

      switch (check.type) {
        case "document.snapshot":
          actual = await documentSnapshot(pdfPath, scenario.openOptions ?? {}, check.pageIndices);
          break;
        case "page.textSummary": {
          const page = scenarioPage(await getDocument(), check);
          actual = await withStatus(() => page.extractText(check.options ?? {}), textSummary);
          break;
        }
        case "page.wordsSummary": {
          const page = scenarioPage(await getDocument(), check);
          actual = await withStatus(() => page.extractWords(check.options ?? {}), objectListSummary);
          break;
        }
        case "page.searchSummary": {
          const page = scenarioPage(await getDocument(), check);
          actual = await withStatus(() => page.search(String(check.pattern ?? ""), check.options ?? {}), searchSummary);
          break;
        }
        case "page.extractTableSummary": {
          const page = scenarioPage(await getDocument(), check);
          actual = await withStatus(() => page.extractTable(check.options ?? {}), jsonValueSummary);
          break;
        }
        case "page.extractTablesSummary": {
          const page = scenarioPage(await getDocument(), check);
          actual = await withStatus(() => page.extractTables(check.options ?? {}), jsonValueSummary);
          break;
        }
        case "page.findTablesSummary": {
          const page = scenarioPage(await getDocument(), check);
          actual = await withStatus(() => page.findTables(check.options ?? {}), findTablesSummary);
          break;
        }
        case "page.mcidText": {
          const page = scenarioPage(await getDocument(), check);
          actual = buildMcidText(page);
          break;
        }
        default:
          throw new Error(`Unknown parity check type: ${check.type}`);
      }

      expectClose(clean(actual), check.expected);
    }
  } finally {
    const opened = document as PDFPlumberDocument | null;
    if (opened) await opened.close();
  }
}
