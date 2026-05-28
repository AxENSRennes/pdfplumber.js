import { execFileSync } from "node:child_process";

import { describe, expect, it } from "vitest";

import { open } from "../../src/index.js";
import { parseColorOps } from "../../src/pdf.js";
import { patternColorValueLikePdfminer } from "../../src/pdf/content.js";
import type { PDFObject } from "../../src/types.js";

interface PatternScenario {
  target: "stroke" | "fill";
  components: number[];
  pattern: string;
  color: unknown;
}

interface PatternFixtureObject {
  type: string;
  stroking_color: unknown;
  non_stroking_color: unknown;
}

interface CharColorSummary {
  colorSpaces: Record<string, number>;
  colorShapes: Record<string, number>;
}

function pythonJson<T>(code: string): T {
  return JSON.parse(execFileSync("wsl_venv/bin/python", ["-c", code], { encoding: "utf8" })) as T;
}

function pdfminerPatternOperatorOracle(): PatternScenario[] {
  const code = `
import json
from pdfminer.converter import PDFPageAggregator
from pdfminer.layout import LAParams
from pdfminer.pdfinterp import PDFPageInterpreter, PDFResourceManager
from pdfminer.psparser import PSLiteral

rsrcmgr = PDFResourceManager()
device = PDFPageAggregator(rsrcmgr, laparams=LAParams())
interpreter = PDFPageInterpreter(rsrcmgr, device)
interpreter.init_resources({})
interpreter.init_state((1, 0, 0, 1, 0, 0))

out = []
for target, ncomponents, operands in [
    ("stroke", 1, [PSLiteral("P1444")]),
    ("fill", 1, [PSLiteral("P1445")]),
    ("stroke", 2, [0.5, PSLiteral("P2000")]),
    ("fill", 4, [0.77, 0.2, 0.0, PSLiteral("P3000")]),
    ("stroke", 5, [0.1, 0.2, 0.3, 0.4, PSLiteral("P4000")]),
]:
    if target == "stroke":
        interpreter.graphicstate.scs.name = "Pattern"
        interpreter.graphicstate.scs.ncomponents = ncomponents
    else:
        interpreter.graphicstate.ncs.name = "Pattern"
        interpreter.graphicstate.ncs.ncomponents = ncomponents
    for operand in operands:
        interpreter.push(operand)
    if target == "stroke":
        interpreter.do_SCN()
        color = interpreter.graphicstate.scolor
    else:
        interpreter.do_scn()
        color = interpreter.graphicstate.ncolor
    out.append({
        "target": target,
        "components": [operand for operand in operands if not isinstance(operand, PSLiteral)],
        "pattern": operands[-1].name,
        "color": color,
    })
print(json.dumps(out))
`;
  return pythonJson<PatternScenario[]>(code);
}

function pdfminerPatternFixtureOracle(): PatternFixtureObject[] {
  const code = `
import json
from pdfminer.high_level import extract_pages
from pdfminer.layout import LTCurve, LTLine, LTRect

out = []
for page in extract_pages("pdfminer-six/samples/test_pattern_colors.pdf"):
    for item in page:
        if isinstance(item, (LTCurve, LTLine, LTRect)):
            out.append({
                "type": type(item).__name__[2:].lower(),
                "stroking_color": item.stroking_color,
                "non_stroking_color": item.non_stroking_color,
            })
print(json.dumps(out))
`;
  return pythonJson<PatternFixtureObject[]>(code);
}

function pdfminerCharColorSummary(path: string): CharColorSummary {
  const code = `
import json
from pdfminer.high_level import extract_pages
from pdfminer.layout import LTChar, LTContainer

def get_chars(el):
    if isinstance(el, LTContainer):
        for item in el:
            yield from get_chars(item)
    elif isinstance(el, LTChar):
        yield el

color_spaces = {}
color_shapes = {}
for page in extract_pages(${JSON.stringify(path)}):
    for char in get_chars(page):
        color_spaces[char.ncs.name] = color_spaces.get(char.ncs.name, 0) + 1
        color = char.graphicstate.ncolor
        if isinstance(color, tuple):
            shape = f"{char.ncs.name}:{len(color)}"
        elif isinstance(color, (int, float)):
            shape = f"{char.ncs.name}:scalar"
        else:
            shape = f"{char.ncs.name}:{type(color).__name__}"
        color_shapes[shape] = color_shapes.get(shape, 0) + 1
print(json.dumps({"colorSpaces": color_spaces, "colorShapes": color_shapes}, sort_keys=True))
`;
  return pythonJson<CharColorSummary>(code);
}

function jsCharColorSummary(chars: PDFObject[]): CharColorSummary {
  const colorSpaces: Record<string, number> = {};
  const colorShapes: Record<string, number> = {};
  for (const char of chars) {
    const colorSpace = String(char.ncs);
    colorSpaces[colorSpace] = (colorSpaces[colorSpace] ?? 0) + 1;
    const color = char.non_stroking_color;
    const shape = Array.isArray(color)
      ? `${colorSpace}:${color.length === 1 ? "scalar" : color.length}`
      : typeof color === "number"
        ? `${colorSpace}:scalar`
        : `${colorSpace}:${typeof color}`;
    colorShapes[shape] = (colorShapes[shape] ?? 0) + 1;
  }
  return { colorSpaces, colorShapes };
}

describe("low-level pdfminer color compatibility", () => {
  it("keeps rgb/gray character color spaces compatible with pdfminer", async () => {
    const path = "pdfminer-six/samples/contrib/issue-00352-hash-twos-complement.pdf";
    const expected = pdfminerCharColorSummary(path);
    const pdf = await open(path);
    try {
      expect(jsCharColorSummary(pdf.pages.flatMap((page) => page.chars))).toEqual(expected);
    } finally {
      await pdf.close();
    }
  });

  it("parses SCN/scn pattern names and uncolored pattern colors like pdfminer", () => {
    const expected = pdfminerPatternOperatorOracle();
    const content = [
      "/Pattern CS /P1444 SCN",
      "/Pattern cs /P1445 scn",
      "/Pattern CS 0.5 /P2000 SCN",
      "/Pattern cs 0.77 0.2 0.0 /P3000 scn",
      "/Pattern CS 0.1 0.2 0.3 0.4 /P4000 SCN"
    ].join("\n");
    const ops = parseColorOps(content);
    expect(ops.map((op) => ({
      target: op.target,
      components: op.components,
      pattern: op.pattern,
      color: op.pattern ? patternColorValueLikePdfminer(op.components, op.pattern) : undefined
    }))).toEqual(expected);
  });

  it("preserves pattern colors on upstream vector fixtures", async () => {
    const expected = pdfminerPatternFixtureOracle().find((item) => item.stroking_color === "P1444" && item.non_stroking_color === "P1444");
    expect(expected).toBeDefined();
    const pdf = await open("pdfminer-six/samples/test_pattern_colors.pdf");
    try {
      const vectors = [...pdf.pages[0].rects, ...pdf.pages[0].curves, ...pdf.pages[0].lines];
      expect(vectors.some((item) => item.stroking_color === expected?.stroking_color && item.non_stroking_color === expected?.non_stroking_color)).toBe(true);
    } finally {
      await pdf.close();
    }
  });
});
