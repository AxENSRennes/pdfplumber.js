import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { expect } from "vitest";
import { open, type BBox, type PDFObject, type PDFPlumberDocument, type PDFPlumberPage } from "../../src/index.js";

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

function tableOptions(args: Record<string, unknown> = {}): Record<string, unknown> {
  const { cells: _cells, col: _col, line: _line, row: _row, ...options } = args;
  return options;
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

function clusterNumbers(values: number[], tolerance: number): number[][] {
  const clusters: number[][] = [];
  for (const value of [...values].sort((a, b) => a - b)) {
    const last = clusters.at(-1);
    if (!last || Math.abs(value - last[last.length - 1]) > tolerance) clusters.push([value]);
    else last.push(value);
  }
  return clusters;
}

function caWarnFixRowSpaces(row: Array<string | null>): Array<string | null> {
  return [...row.slice(0, 3).map((value) => (value ?? "").replaceAll(" ", "")), ...row.slice(3)];
}

async function caWarnParseSummary(document: PDFPlumberDocument): Promise<Record<string, unknown>> {
  const firstPage = page(document, 0);
  const secondPage = page(document, 1);
  const vLines = clusterNumbers(secondPage.rects.map((rect) => Number(rect.x0)), 3).map((cluster) => cluster[0]);
  const table = await valueOf(firstPage.extractTable({ vertical_strategy: "explicit", explicit_vertical_lines: vLines }));
  return {
    v_lines: vLines,
    row_count: table?.length ?? 0,
    column_count: table?.[0]?.length ?? 0,
    header: table ? caWarnFixRowSpaces(table[0]) : [],
    first_data_row: table ? caWarnFixRowSpaces(table[1]) : [],
    last_data_row: table ? caWarnFixRowSpaces(table.at(-1) ?? []) : []
  };
}

function issue13CheckboxSummary(selectedPage: PDFPlumberPage): Record<string, unknown> {
  const rectWidth = 9.3;
  const rectHeight = 9.3;
  const tolerance = 2;
  const checklines = selectedPage.lines.filter((line) => Number(Number(line.height).toFixed(2)) === Number(Number(line.width).toFixed(2)));
  const rects = selectedPage.objects.rect.filter((rect) =>
    Number(rect.height) > rectHeight - tolerance &&
    Number(rect.height) < rectHeight + tolerance &&
    Number(rect.width) < rectWidth + tolerance &&
    Number(rect.width) < rectWidth + tolerance
  );
  const checked = (checkbox: Record<string, unknown>) =>
    checklines.some((line) =>
      Math.max(Number(checkbox.x0), Number(line.x0)) <= Math.min(Number(checkbox.x1), Number(line.x1)) &&
      Math.max(Number(checkbox.y0), Number(line.y0)) <= Math.min(Number(checkbox.y1), Number(line.y1))
    );
  return {
    rect_count: rects.length,
    diagonal_line_count: checklines.length,
    checked_count: rects.filter(checked).length
  };
}

async function pr138TableSummary(selectedPage: PDFPlumberPage): Promise<Record<string, unknown>> {
  const tables = await valueOf(selectedPage.extractTables({
    vertical_strategy: "explicit",
    horizontal_strategy: "lines",
    explicit_vertical_lines: [...selectedPage.curves, ...selectedPage.edges]
  }));
  return {
    char_count: selectedPage.chars.length,
    curve_count: selectedPage.curves.length,
    edge_count: selectedPage.edges.length,
    table_shapes: tables.map((table) => [table.length, table[0]?.length ?? 0])
  };
}

function metadataChangesSummary(metadata: Record<string, unknown>): Record<string, unknown> {
  const changes = metadata.Changes;
  const first = Array.isArray(changes) ? changes[0] as Record<string, unknown> | undefined : undefined;
  return {
    changes_is_list: Array.isArray(changes),
    first_creation_date: first?.CreationDate ?? null
  };
}

function fontnameDedupeSummary(selectedPage: PDFPlumberPage): Record<string, unknown> {
  return {
    char_count: selectedPage.chars.length,
    fontnames_are_str: selectedPage.chars.every((char) => typeof char.fontname === "string"),
    dedupe_error: captureErrorName(() => selectedPage.dedupeChars())
  };
}

async function textFlowMatchSummary(selectedPage: PDFPlumberPage): Promise<Record<string, unknown>> {
  const text = (await valueOf(selectedPage.extractText({ use_text_flow: true }))).replace(/\s+/g, " ");
  const words = (await valueOf(selectedPage.extractWords({ use_text_flow: true }))).map((word: PDFObject) => String(word.text)).join(" ");
  return {
    text_head: text.slice(0, 100),
    words_head: words.slice(0, 100),
    heads_match: text.slice(0, 100) === words.slice(0, 100)
  };
}

const textRotations: Array<[string, string, string]> = [
  ["0", "ltr", "ttb"],
  ["-0", "rtl", "ttb"],
  ["180", "rtl", "btt"],
  ["-180", "ltr", "btt"],
  ["90", "ttb", "rtl"],
  ["-90", "btt", "rtl"],
  ["270", "btt", "ltr"],
  ["-270", "ttb", "ltr"]
];

async function textRotationSummary(document: PDFPlumberDocument): Promise<Record<string, unknown>> {
  const expected = await valueOf(document.pages[0].extractText());
  const matches: Record<string, boolean> = {};
  for (let index = 1; index < textRotations.length; index += 1) {
    const [rotation, charDir, lineDir] = textRotations[index];
    const page = document.pages[index].filter((object) => object.text !== " ");
    const output = await valueOf(page.extractText({
      x_tolerance: 2,
      y_tolerance: 2,
      char_dir: charDir,
      line_dir: lineDir,
      char_dir_rotated: charDir,
      line_dir_rotated: lineDir,
      char_dir_render: "ltr",
      line_dir_render: "ttb"
    }));
    matches[rotation] = output === expected;
  }
  return { base_head: expected.slice(0, 120), matches };
}

async function textRotationLayoutSummary(document: PDFPlumberDocument): Promise<Record<string, boolean>> {
  const out: Record<string, boolean> = {};
  for (let index = 0; index < textRotations.length; index += 1) {
    const [rotation, charDir, lineDir] = textRotations[index];
    const page = document.pages[index].filter((object) => object.text !== " ");
    const text = await valueOf(page.extractText({
      layout: true,
      x_tolerance: 2,
      y_tolerance: 2,
      char_dir: charDir,
      line_dir: lineDir,
      char_dir_rotated: charDir,
      line_dir_rotated: lineDir,
      char_dir_render: "ltr",
      line_dir_render: "ttb",
      y_density: 14
    }));
    const first = text.search(/opens with a news report/);
    const second = text.search(/having been transferred/);
    out[rotation] = first >= 0 && second >= 0 && first < second;
  }
  return out;
}

async function textRenderDirectionsSummary(selectedPage: PDFPlumberPage): Promise<Record<string, string>> {
  const out: Record<string, string> = {};
  for (const lineDir of ["ttb", "btt", "ltr", "rtl"]) {
    const charDirs = lineDir === "ttb" || lineDir === "btt" ? ["ltr", "rtl"] : ["ttb", "btt"];
    for (const charDir of charDirs) {
      out[`${lineDir}/${charDir}`] = await valueOf(selectedPage.extractText({ line_dir_render: lineDir, char_dir_render: charDir }));
    }
  }
  return out;
}

function invalidDirectionsSummary(selectedPage: PDFPlumberPage): Record<string, string | null> {
  const checks: Record<string, Record<string, unknown>> = {
    line_invalid: { line_dir: "xxx", char_dir: "ltr" },
    char_invalid: { line_dir: "ttb", char_dir: "a" },
    line_char_incompatible: { line_dir: "rtl", char_dir: "ltr" },
    line_char_axis_incompatible: { line_dir: "ttb", char_dir: "btt" },
    rotated_incompatible: { line_dir_rotated: "ttb", char_dir_rotated: "btt" },
    render_incompatible: { line_dir_render: "ttb", char_dir_render: "btt" }
  };
  return Object.fromEntries(Object.entries(checks).map(([name, options]) => [name, captureErrorName(() => selectedPage.extractText(options))]));
}

async function extraAttrsTextSummary(selectedPage: PDFPlumberPage): Promise<Record<string, unknown>> {
  return {
    default: await valueOf(selectedPage.extractText()),
    color: await valueOf(selectedPage.extractText({ extra_attrs: ["non_stroking_color"] })),
    fontname: await valueOf(selectedPage.extractText({ extra_attrs: ["fontname"] })),
    color_fontname: await valueOf(selectedPage.extractText({ extra_attrs: ["non_stroking_color", "fontname"] })),
    layout_flow_error: captureErrorName(() => selectedPage.extractText({ layout: true, use_text_flow: true, extra_attrs: ["non_stroking_color", "fontname"] }))
  };
}

function wordsToText(words: Array<Record<string, unknown>>): string {
  const lines: string[] = [];
  let currentTop: unknown = Symbol("unset");
  let current: string[] = [];
  for (const word of words) {
    if (word.top !== currentTop) {
      if (current.length) lines.push(current.join(" "));
      currentTop = word.top;
      current = [];
    }
    current.push(String(word.text ?? ""));
  }
  if (current.length) lines.push(current.join(" "));
  return lines.join("\n");
}

async function textFlowSummary(selectedPage: PDFPlumberPage): Promise<Record<string, boolean>> {
  const target = [
    "The FAA proposes to",
    "supersede Airworthiness Directive (AD)",
    "2018–23–51, which applies to all The",
    "Boeing Company Model 737–8 and 737–",
    "9 (737 MAX) airplanes. Since AD 2018–",
    ""
  ].join("\n");
  return {
    target_in_flow: wordsToText(await valueOf(selectedPage.extractWords({ use_text_flow: true })) as Array<Record<string, unknown>>).includes(target),
    target_in_default: wordsToText(await valueOf(selectedPage.extractWords()) as Array<Record<string, unknown>>).includes(target)
  };
}

async function textFlowOverlappingSummary(selectedPage: PDFPlumberPage): Promise<Record<string, boolean>> {
  const usingFlow = await valueOf(selectedPage.extractText({ use_text_flow: true, layout: true, x_tolerance: 1 }));
  const notUsingFlow = await valueOf(selectedPage.extractText({ layout: true, x_tolerance: 1 }));
  return {
    flow_has_payment: /2015 RICE PAYMENT 26406576 0 1207631 Cr/.test(usingFlow),
    flow_has_bad_merge: /124644,06155766/.test(usingFlow),
    default_has_payment: /2015 RICE PAYMENT 26406576 0 1207631 Cr/.test(notUsingFlow),
    default_has_bad_merge: /124644,06155766/.test(notUsingFlow)
  };
}

async function textFlowWordsMixedLinesSummary(selectedPage: PDFPlumberPage): Promise<Record<string, boolean>> {
  const texts = new Set((await valueOf(selectedPage.extractWords({ use_text_flow: true }))).map((word: PDFObject) => String(word.text)));
  return {
    has_claim: texts.has("claim"),
    has_lence: texts.has("lence"),
    has_claimlence: texts.has("claimlence")
  };
}

async function extractTextLinesSummary(selectedPage: PDFPlumberPage): Promise<Record<string, unknown>> {
  const results = await valueOf(selectedPage.extractTextLines());
  const alt = await valueOf(selectedPage.extractTextLines({ layout: true, strip: false, return_chars: false }));
  const stripped = await valueOf(selectedPage.extractTextLines({ layout: true }));
  return {
    count: results.length,
    first_has_chars: Object.prototype.hasOwnProperty.call(results[0], "chars"),
    first_text: results[0]?.text,
    alt_first_has_chars: Object.prototype.hasOwnProperty.call(alt[0], "chars"),
    alt_first_text: alt[0]?.text,
    line10_text: results[10]?.text,
    alt_line10_text: alt[10]?.text,
    stripped_line10_text: stripped[10]?.text
  };
}

async function layoutWidthsSummary(selectedPage: PDFPlumberPage): Promise<Record<string, unknown>> {
  const text = await valueOf(selectedPage.extractText({ layout: true, layout_width_chars: 75 }));
  return {
    all_lines_75: text.split("\n").every((line) => line.length === 75),
    width_conflict: captureErrorName(() => selectedPage.extractText({ layout: true, layout_width: 300, layout_width_chars: 50 })),
    height_conflict: captureErrorName(() => selectedPage.extractText({ layout: true, layout_height: 300, layout_height_chars: 50 }))
  };
}

async function charlessTextSummary(selectedPage: PDFPlumberPage): Promise<Record<string, string>> {
  const charless = selectedPage.filter((object) => object.object_type !== "char");
  return {
    plain: await valueOf(charless.extractText()),
    layout: await valueOf(charless.extractText({ layout: true }))
  };
}

function slimSearchResult(result: Record<string, unknown>): Record<string, unknown> {
  return {
    text: result.text,
    groups: result.groups
  };
}

function searchRegexCompiledSummary(selectedPage: PDFPlumberPage): Record<string, unknown> {
  const results = selectedPage.search(/supreme\s+(\w+)/i) as unknown as Array<Record<string, unknown>>;
  return {
    first_two: results.slice(0, 2).map(slimSearchResult),
    regex_false_error: captureErrorName(() => selectedPage.search(/x/, { regex: false })),
    case_false_error: captureErrorName(() => selectedPage.search(/x/, { case: false }))
  };
}

function searchRegexUncompiledSummary(selectedPage: PDFPlumberPage): Array<Record<string, unknown>> {
  return (selectedPage.search("supreme\\s+(\\w+)", { case: false }) as unknown as Array<Record<string, unknown>>).slice(0, 2).map(slimSearchResult);
}

async function searchEmptySummary(selectedPage: PDFPlumberPage): Promise<Record<string, number>> {
  return {
    newline_regex: (await valueOf(selectedPage.search("\n", { regex: true }))).length,
    newline_literal: (await valueOf(selectedPage.search("\n", { regex: false }))).length,
    optional_empty: (await valueOf(selectedPage.search("(sdfsd)?"))).length,
    empty: (await valueOf(selectedPage.search(""))).length
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
  const rows = table.rows.map((row) => row.cells);
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

async function tableRowsColumnsSummary(selectedPage: PDFPlumberPage): Promise<Record<string, unknown>> {
  const table = await valueOf(selectedPage.findTable());
  if (!table) {
    return {
      cell_count: 0,
      row_count: 0,
      column_count: 0,
      row0: [],
      column1: []
    };
  }
  const row0 = await Promise.all(
    table.rows[0].cells.map(async (bbox) => (bbox ? await valueOf(selectedPage.crop(bbox).extractText()) : null))
  );
  const column1 = await Promise.all(
    table.columns[1].cells.map(async (bbox) => (bbox ? await valueOf(selectedPage.crop(bbox).extractText()) : null))
  );
  return {
    cell_count: table.cells.length,
    row_count: table.rows.length,
    column_count: table.columns.length,
    row0,
    column1
  };
}

function captureErrorName(fn: () => unknown): string | null {
  try {
    fn();
  } catch (error) {
    return error instanceof Error ? error.name : typeof error;
  }
  return null;
}

function tableSettingsErrorSummary(selectedPage: PDFPlumberPage): Record<string, string | null> {
  return {
    non_mapping: captureErrorName(() => selectedPage.extractTables([] as unknown as Record<string, unknown>)),
    unknown_setting: captureErrorName(() => selectedPage.extractTables({ strategy: "x" })),
    invalid_vertical_strategy: captureErrorName(() => selectedPage.extractTables({ vertical_strategy: "x" })),
    explicit_vertical_lines: captureErrorName(() => selectedPage.extractTables({ vertical_strategy: "explicit", explicit_vertical_lines: [] })),
    negative_join_tolerance: captureErrorName(() => selectedPage.extractTables({ join_tolerance: -1 }))
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
      const selectedPage = check.type.startsWith("pdf.") ? (undefined as unknown as PDFPlumberPage) : page(document, check.page ?? 0);
      let actual: unknown;

      switch (check.type) {
        case "pdf.pageCount":
          actual = document.pages.length;
          break;
        case "pdf.metadata":
          actual = document.metadata;
          break;
        case "pdf.metadataHasKeys":
          actual = Object.keys(document.metadata).length > 0;
          break;
        case "pdf.metadataChangesSummary":
          actual = metadataChangesSummary(document.metadata);
          break;
        case "pdf.textRotationSummary":
          actual = await textRotationSummary(document);
          break;
        case "pdf.textRotationLayoutSummary":
          actual = await textRotationLayoutSummary(document);
          break;
        case "pdf.objectCounts":
          actual = Object.fromEntries(Object.entries(document.objects).map(([key, value]) => [key, value.length]));
          break;
        case "pdf.annots.count":
          actual = document.annots.length;
          break;
        case "pdf.annots.error":
          actual = captureErrorName(() => document.annots);
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
        case "pdf.caWarnParseSummary":
          actual = await caWarnParseSummary(document);
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
        case "page.issue13CheckboxSummary":
          actual = issue13CheckboxSummary(selectedPage);
          break;
        case "page.pr138TableSummary":
          actual = await pr138TableSummary(selectedPage);
          break;
        case "page.fontnameDedupeSummary":
          actual = fontnameDedupeSummary(selectedPage);
          break;
        case "page.textFlowMatchSummary":
          actual = await textFlowMatchSummary(selectedPage);
          break;
        case "page.textRenderDirectionsSummary":
          actual = await textRenderDirectionsSummary(selectedPage);
          break;
        case "page.invalidDirectionsSummary":
          actual = invalidDirectionsSummary(selectedPage);
          break;
        case "page.extraAttrsTextSummary":
          actual = await extraAttrsTextSummary(selectedPage);
          break;
        case "page.textFlowSummary":
          actual = await textFlowSummary(selectedPage);
          break;
        case "page.textFlowOverlappingSummary":
          actual = await textFlowOverlappingSummary(selectedPage);
          break;
        case "page.textFlowWordsMixedLinesSummary":
          actual = await textFlowWordsMixedLinesSummary(selectedPage);
          break;
        case "page.extractTextLinesSummary":
          actual = await extractTextLinesSummary(selectedPage);
          break;
        case "page.layoutWidthsSummary":
          actual = await layoutWidthsSummary(selectedPage);
          break;
        case "page.charlessTextSummary":
          actual = await charlessTextSummary(selectedPage);
          break;
        case "page.searchRegexCompiledSummary":
          actual = searchRegexCompiledSummary(selectedPage);
          break;
        case "page.searchRegexUncompiledSummary":
          actual = searchRegexUncompiledSummary(selectedPage);
          break;
        case "page.searchEmptySummary":
          actual = await searchEmptySummary(selectedPage);
          break;
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
        case "page.tableRowsColumnsSummary":
          actual = await tableRowsColumnsSummary(selectedPage);
          break;
        case "page.tableSettingsErrorSummary":
          actual = tableSettingsErrorSummary(selectedPage);
          break;
        case "page.extractWords":
          actual = await valueOf(selectedPage.extractWords(check.args ?? {}));
          break;
        case "page.extractWords.count":
          actual = (await valueOf(selectedPage.extractWords(check.args ?? {}))).length;
          break;
        case "page.extractWords.error": {
          let errorName = null;
          try {
            await valueOf(selectedPage.extractWords(check.args ?? {}));
          } catch (error) {
            errorName = error instanceof Error ? error.name : typeof error;
          }
          actual = errorName;
          break;
        }
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
          actual = await valueOf(selectedPage.extractTable(tableOptions(check.args)));
          break;
        case "page.extractTable.cellLine":
          actual = tableCellLine(
            await valueOf(selectedPage.extractTable(tableOptions(check.args))),
            (check.args?.row as number | undefined) ?? 0,
            (check.args?.col as number | undefined) ?? 0,
            (check.args?.line as number | undefined) ?? -1
          );
          break;
        case "page.dedupe.extractTable.cellLine":
          actual = tableCellLine(
            await valueOf(selectedPage.dedupeChars(check.args ?? {}).extractTable(tableOptions(check.args))),
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
          }).extractTable(tableOptions(check.args)));
          break;
        case "page.crop.extractTables":
          actual = await valueOf(selectedPage.crop(check.bbox as BBox, {
            relative: check.relative,
            strict: check.strict
          }).extractTables(tableOptions(check.args)));
          break;
        case "page.crop.extractTableSummary":
          actual = tableSampleSummary(
            await valueOf(selectedPage.crop(check.bbox as BBox, {
              relative: check.relative,
              strict: check.strict
            }).extractTable(tableOptions(check.args))),
            (check.args?.cells as number[][] | undefined) ?? []
          );
          break;
        case "page.crop.extractTableNumericSummary":
          actual = nicsPlainTableSummary(await valueOf(selectedPage.crop(check.bbox as BBox, {
            relative: check.relative,
            strict: check.strict
          }).extractTable(tableOptions(check.args))));
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
