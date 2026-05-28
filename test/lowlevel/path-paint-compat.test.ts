import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { open } from "../../src/index.js";
import { paintPathLikePdfminer, type PdfminerPathOp } from "../../src/pdf/path-paint.js";

interface PaintPathOracle {
  types: string[];
  pts: number[][][];
  original_path: unknown[];
  dashing_style: unknown[];
}

function pdfminerPaintPathOracle(path: PdfminerPathOp[], dashing_style?: unknown): PaintPathOracle {
  const code = `
import json
from pdfminer.converter import PDFLayoutAnalyzer
from pdfminer.layout import LTContainer
from pdfminer.pdfinterp import PDFGraphicState

payload = json.loads(${JSON.stringify(JSON.stringify({ path, dashing_style: dashing_style ?? null }))})
analyzer = PDFLayoutAnalyzer(None)
analyzer.set_ctm([1, 0, 0, 1, 0, 0])
analyzer.cur_item = LTContainer([0, 1000, 0, 1000])
graphicstate = PDFGraphicState()
if payload["dashing_style"] is not None:
    graphicstate.dash = (payload["dashing_style"][0], payload["dashing_style"][1])
analyzer.paint_path(graphicstate, False, False, False, [tuple(item) for item in payload["path"]])
items = list(analyzer.cur_item._objs)
print(json.dumps({
    "types": [type(item).__name__[2:].lower() for item in items],
    "pts": [getattr(item, "pts", None) for item in items],
    "original_path": [getattr(item, "original_path", None) for item in items],
    "dashing_style": [getattr(item, "dashing_style", None) for item in items],
}))
`;
  return JSON.parse(execFileSync("wsl_venv/bin/python", ["-c", code], { encoding: "utf8" })) as PaintPathOracle;
}

function pdfminerFixtureOracle(path: string): { lines: number; linewidths: number[] } {
  const code = `
import json
from pdfminer.high_level import extract_pages
from pdfminer.layout import LTLine

page = next(iter(extract_pages(${JSON.stringify(path)})))
lines = [item for item in page if type(item) is LTLine]
print(json.dumps({
    "lines": len(lines),
    "linewidths": sorted([line.linewidth for line in lines]),
}))
`;
  return JSON.parse(execFileSync("wsl_venv/bin/python", ["-c", code], { encoding: "utf8" })) as { lines: number; linewidths: number[] };
}

function latin1Bytes(value: string): Uint8Array {
  const bytes = new Uint8Array(value.length);
  for (let index = 0; index < value.length; index += 1) bytes[index] = value.charCodeAt(index) & 0xff;
  return bytes;
}

function concatBytes(...chunks: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(chunks.reduce((sum, chunk) => sum + chunk.length, 0));
  let offset = 0;
  for (const chunk of chunks) {
    out.set(chunk, offset);
    offset += chunk.length;
  }
  return out;
}

function onePagePdf(content: string): Uint8Array {
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>",
    `<< /Length ${latin1Bytes(content).length} >>\nstream\n${content}\nendstream`
  ].map(latin1Bytes);
  const chunks: Uint8Array[] = [latin1Bytes("%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")];
  const offsets = [0];
  let length = chunks[0].length;
  for (let index = 0; index < objects.length; index += 1) {
    offsets.push(length);
    const prefix = latin1Bytes(`${index + 1} 0 obj\n`);
    const suffix = latin1Bytes("\nendobj\n");
    chunks.push(prefix, objects[index], suffix);
    length += prefix.length + objects[index].length + suffix.length;
  }
  const xrefOffset = length;
  chunks.push(latin1Bytes(`xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`));
  for (const offset of offsets.slice(1)) chunks.push(latin1Bytes(`${String(offset).padStart(10, "0")} 00000 n \n`));
  chunks.push(latin1Bytes(`trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xrefOffset}\n%%EOF\n`));
  return concatBytes(...chunks);
}

function pdfminerLinewidthOracle(pdfBytes: Uint8Array): number[] {
  const dir = mkdtempSync(join(tmpdir(), "pdfplumber-js-linewidth-"));
  const path = join(dir, "line-width.pdf");
  try {
    writeFileSync(path, pdfBytes);
    const code = `
import json
from pdfminer.high_level import extract_pages
from pdfminer.layout import LTLine

page = next(iter(extract_pages(${JSON.stringify(path)})))
print(json.dumps([line.linewidth for line in page if isinstance(line, LTLine)]))
`;
    return JSON.parse(execFileSync("wsl_venv/bin/python", ["-c", code], { encoding: "utf8" })) as number[];
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

function pdfminerDashOracle(pdfBytes: Uint8Array): unknown[] {
  const dir = mkdtempSync(join(tmpdir(), "pdfplumber-js-dash-"));
  const path = join(dir, "dash.pdf");
  try {
    writeFileSync(path, pdfBytes);
    const code = `
import json
import pdfplumber

with pdfplumber.open(${JSON.stringify(path)}) as pdf:
    print(json.dumps([line.get("dash") for line in pdf.pages[0].lines], default=str))
`;
    return JSON.parse(execFileSync("wsl_venv/bin/python", ["-c", code], { encoding: "utf8" })) as unknown[];
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

function types(path: PdfminerPathOp[]): string[] {
  return paintPathLikePdfminer(path).map((item) => item.type);
}

function expectTypesLikePdfminer(path: PdfminerPathOp[]): void {
  expect(types(path)).toEqual(pdfminerPaintPathOracle(path).types);
}

describe("low-level pdfminer path painting compatibility", () => {
  it("classifies simple lines, closed rectangles, and multi-subpath rectangles like pdfminer", () => {
    expectTypesLikePdfminer([["m", 6, 7], ["l", 7, 7]]);
    expectTypesLikePdfminer([["m", 6, 7], ["l", 7, 7], ["l", 7, 91], ["l", 6, 91], ["h"]]);
    expect(types([
      ["m", 6, 7],
      ["l", 7, 7],
      ["l", 7, 91],
      ["l", 6, 91],
      ["h"],
      ["m", 4, 7],
      ["l", 6, 7],
      ["l", 6, 91],
      ["l", 4, 91],
      ["h"],
      ["m", 67, 2],
      ["l", 68, 2],
      ["l", 68, 3],
      ["l", 67, 3],
      ["h"]
    ])).toEqual(pdfminerPaintPathOracle([
      ["m", 6, 7],
      ["l", 7, 7],
      ["l", 7, 91],
      ["l", 6, 91],
      ["h"],
      ["m", 4, 7],
      ["l", 6, 7],
      ["l", 6, 91],
      ["l", 4, 91],
      ["h"],
      ["m", 67, 2],
      ["l", 68, 2],
      ["l", 68, 3],
      ["l", 67, 3],
      ["h"]
    ]).types);
  });

  it("classifies quadrilateral edge cases like pdfminer issue 473", () => {
    expectTypesLikePdfminer([["m", 10, 90], ["l", 90, 90], ["l", 90, 10], ["l", 10, 10], ["h"]]);
    expectTypesLikePdfminer([["m", 10, 90], ["l", 90, 90], ["l", 90, 10], ["l", 10, 10], ["l", 10, 90]]);
    expectTypesLikePdfminer([["m", 110, 90], ["l", 190, 10], ["l", 190, 90], ["l", 110, 10], ["h"]]);
    expectTypesLikePdfminer([["m", 210, 90], ["l", 290, 60], ["l", 290, 10], ["l", 210, 10], ["h"]]);
    const linesPath: PdfminerPathOp[] = [
      ["m", 10, 30],
      ["l", 10, 40],
      ["h"],
      ["m", 10, 50],
      ["l", 70, 50],
      ["h"],
      ["m", 10, 10],
      ["l", 30, 30],
      ["h"]
    ];
    expect(types(linesPath)).toEqual(pdfminerPaintPathOracle(linesPath).types);
  });

  it("keeps Bezier endpoints, raw path operators, and dash style like pdfminer", () => {
    const cPathInput: PdfminerPathOp[] = [["m", 72.41, 433.89], ["c", 72.41, 434.45, 71.96, 434.89, 71.41, 434.89]];
    const cPath = paintPathLikePdfminer(cPathInput);
    const cOracle = pdfminerPaintPathOracle(cPathInput);
    expect(cPath[0].pts).toEqual(cOracle.pts[0]);
    expect(cPath[0].original_path).toEqual(cOracle.original_path[0]);

    const vPathInput: PdfminerPathOp[] = [["m", 72.41, 433.89], ["v", 71.96, 434.89, 71.41, 434.89]];
    expect(paintPathLikePdfminer(vPathInput)[0].pts).toEqual(pdfminerPaintPathOracle(vPathInput).pts[0]);

    const yPathInput: PdfminerPathOp[] = [["m", 72.41, 433.89], ["y", 72.41, 434.45, 71.41, 434.89]];
    expect(paintPathLikePdfminer(yPathInput)[0].pts).toEqual(pdfminerPaintPathOracle(yPathInput).pts[0]);

    const dashed = paintPathLikePdfminer(cPathInput, { dashing_style: [[1, 1], 0] });
    expect(dashed[0].dashing_style).toEqual(pdfminerPaintPathOracle(cPathInput, [[1, 1], 0]).dashing_style[0]);
  });

  it("ignores paths without an initial move like pdfminer", () => {
    expect(paintPathLikePdfminer([["h"]])).toEqual([]);
    expect(paintPathLikePdfminer([["l", 72.41, 433.89], ["l", 82.41, 433.89], ["h"]])).toEqual([]);
  });

  it("matches pdfminer line counts and line widths on upstream path fixtures", async () => {
    for (const path of ["pdfminer-six/samples/contrib/pr-00530-ml-lines.pdf", "pdfminer-six/samples/contrib/issue_1165_linewidth.pdf"]) {
      const expected = pdfminerFixtureOracle(path);
      const pdf = await open(path);
      try {
        expect(pdf.pages[0].lines).toHaveLength(expected.lines);
        expect(pdf.pages[0].lines.map((line) => line.linewidth).sort((a, b) => Number(a) - Number(b))).toEqual(expected.linewidths);
      } finally {
        await pdf.close();
      }
    }
  });

  it("preserves diagnostic negative line widths like pdfminer", async () => {
    const pdfBytes = onePagePdf("-5 w 10 10 m 90 10 l S");
    const expected = pdfminerLinewidthOracle(pdfBytes);
    const pdf = await open(pdfBytes);
    try {
      expect(pdf.pages[0].lines.map((line) => line.linewidth)).toEqual(expected);
    } finally {
      await pdf.close();
    }
  });

  it("preserves diagnostic invalid dash arrays like pdfplumber", async () => {
    const pdfBytes = onePagePdf("[ none ] 0 d 10 10 m 90 10 l S");
    const expected = pdfminerDashOracle(pdfBytes);
    const pdf = await open(pdfBytes);
    try {
      expect(pdf.pages[0].lines.map((line) => line.dash)).toEqual(expected);
    } finally {
      await pdf.close();
    }
  });
});
