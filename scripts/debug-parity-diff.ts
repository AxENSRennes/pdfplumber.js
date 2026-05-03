import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import path from "node:path";

import { open, type PDFObject, type PDFPlumberDocument, type PDFPlumberPage, type Table } from "../src/index.ts";

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

function finiteNumber(value: unknown, digits = 6): number | null {
  if (typeof value !== "number" || !Number.isFinite(value)) return null;
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

function sha256Value(value: unknown): string {
  return createHash("sha256").update(pythonJson(stableForJson(value, HASH_NUMERIC_DIGITS)), "utf8").digest("hex");
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
    return { status: "ok", value: summarize(await fn()) };
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
  for (const key of ["char", "line", "rect", "curve", "image", "annot", "textboxhorizontal", "textlinehorizontal", "textboxvertical", "textlinevertical"]) {
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

async function documentSnapshot(pdfPath: string, openOptions: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
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
    const pages = [];
    for (const page of document.pages) pages.push(await pageSnapshot(page));
    const metadata: Record<string, unknown> = {};
    for (const key of ["Author", "Creator", "Producer", "Title", "Subject", "Keywords", "CreationDate", "ModDate", "Copies"]) {
      if (key in document.metadata) metadata[key] = document.metadata[key];
    }
    return {
      status: "ok",
      metadata: clean(metadata),
      pageCount: document.pages.length,
      objectCounts: objectCounts(document.objects),
      annots: objectListSummary(document.annots),
      hyperlinks: objectListSummary(document.hyperlinks),
      pages
    };
  } finally {
    await document.close();
  }
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
  const encoded = JSON.stringify(stableForJson(cleaned));
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

function scenarioPage(document: PDFPlumberDocument, check: Record<string, unknown>): PDFPlumberPage {
  const selected = document.pages[Number(check.page ?? 0)];
  if (!selected) throw new Error(`Missing page at index ${Number(check.page ?? 0)}`);
  let page = selected;
  const bbox = check.bbox as readonly number[] | undefined;
  if (bbox) {
    if (check.bboxMethod === "outside") page = page.outsideBbox(bbox);
    else if (check.bboxMethod === "within") page = page.withinBbox(bbox);
    else page = page.crop(bbox);
  }
  if (check.dedupe) page = page.dedupe_chars((check.dedupeOptions as Record<string, unknown> | undefined) ?? {});
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

async function actualForCheck(scenario: Record<string, unknown>, check: Record<string, unknown>): Promise<unknown> {
  const pdfPath = path.join(process.cwd(), "pdfplumber-python/tests/pdfs", String(scenario.pdf));
  if (check.type === "document.snapshot") return documentSnapshot(pdfPath, (scenario.openOptions as Record<string, unknown> | undefined) ?? {});
  const document = await open(pdfPath, (scenario.openOptions as Record<string, unknown> | undefined) ?? {});
  try {
    const page = scenarioPage(document, check);
    switch (check.type) {
      case "page.textSummary":
        return withStatus(() => page.extractText((check.options as Record<string, unknown> | undefined) ?? {}), textSummary);
      case "page.wordsSummary":
        return withStatus(() => page.extractWords((check.options as Record<string, unknown> | undefined) ?? {}), objectListSummary);
      case "page.extractTableSummary":
        return withStatus(() => page.extractTable((check.options as Record<string, unknown> | undefined) ?? {}), jsonValueSummary);
      case "page.extractTablesSummary":
        return withStatus(() => page.extractTables((check.options as Record<string, unknown> | undefined) ?? {}), jsonValueSummary);
      case "page.findTablesSummary":
        return withStatus(() => page.findTables((check.options as Record<string, unknown> | undefined) ?? {}), findTablesSummary);
      case "page.mcidText":
        return buildMcidText(page);
      default:
        throw new Error(`Unsupported check ${String(check.type)}`);
    }
  } finally {
    await document.close();
  }
}

function firstDiff(actual: unknown, expected: unknown, pathKey = ""): Record<string, unknown> | null {
  if (typeof actual === "number" && typeof expected === "number") {
    return Math.abs(actual - expected) > 0.000005 ? { path: pathKey, actual, expected } : null;
  }
  if (Array.isArray(actual) || Array.isArray(expected)) {
    if (!Array.isArray(actual) || !Array.isArray(expected)) return { path: pathKey, actual, expected };
    if (actual.length !== expected.length) return { path: `${pathKey}.length`, actual: actual.length, expected: expected.length };
    for (let i = 0; i < expected.length; i += 1) {
      const diff = firstDiff(actual[i], expected[i], `${pathKey}[${i}]`);
      if (diff) return diff;
    }
    return null;
  }
  if (actual && expected && typeof actual === "object" && typeof expected === "object") {
    const actualKeys = Object.keys(actual as Record<string, unknown>).sort();
    const expectedKeys = Object.keys(expected as Record<string, unknown>).sort();
    if (JSON.stringify(actualKeys) !== JSON.stringify(expectedKeys)) return { path: `${pathKey}.keys`, actual: actualKeys, expected: expectedKeys };
    for (const key of expectedKeys) {
      const diff = firstDiff((actual as Record<string, unknown>)[key], (expected as Record<string, unknown>)[key], pathKey ? `${pathKey}.${key}` : key);
      if (diff) return diff;
    }
    return null;
  }
  return JSON.stringify(actual) === JSON.stringify(expected) ? null : { path: pathKey, actual, expected };
}

const goldenPath = path.join(process.cwd(), "test/fixtures/goldens/pdfplumber-parity.json");
const goldens = JSON.parse(readFileSync(goldenPath, "utf8")) as { scenarios: Array<Record<string, unknown>> };
const requested = process.argv.slice(2);

for (const id of requested) {
  const scenario = goldens.scenarios.find((item) => item.id === id);
  if (!scenario) throw new Error(`Unknown scenario: ${id}`);
  console.log(`\n${id}`);
  for (const [index, check] of (scenario.checks as Array<Record<string, unknown>>).entries()) {
    const actual = clean(await actualForCheck(scenario, check));
    const diff = firstDiff(actual, check.expected, `checks[${index}]`);
    if (diff) {
      console.dir({ type: check.type, ...diff }, { depth: 12 });
      break;
    }
  }
}
