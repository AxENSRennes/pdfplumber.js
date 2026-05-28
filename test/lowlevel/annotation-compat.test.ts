import { execFileSync } from "node:child_process";

import { describe, expect, it } from "vitest";

import { open } from "../../src/index.js";

const annotationDataKeys = ["Subtype", "Rect", "Contents", "T", "C", "FT", "V", "AS", "Ff", "Opt"] as const;

function clean(value: unknown): unknown {
  if (typeof value === "number") return Number(value.toFixed(6));
  if (Array.isArray(value)) return value.map(clean);
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.entries(value).filter(([, item]) => item !== undefined).map(([key, item]) => [key, clean(item)]));
  }
  return value;
}

function simplifyAnnot(annot: Record<string, unknown>): Record<string, unknown> {
  const data = (annot.data && typeof annot.data === "object" ? annot.data : {}) as Record<string, unknown>;
  return clean({
    x0: annot.x0,
    y0: annot.y0,
    x1: annot.x1,
    y1: annot.y1,
    top: annot.top,
    bottom: annot.bottom,
    uri: annot.uri,
    title: annot.title,
    contents: annot.contents,
    data: Object.fromEntries(annotationDataKeys.filter((key) => key in data).map((key) => [key, data[key]]))
  }) as Record<string, unknown>;
}

function pdfplumberAnnotationOracle(paths: string[]): Record<string, Record<string, unknown>[]> {
  const code = `
import json
import pdfplumber

keys = ${JSON.stringify([...annotationDataKeys])}
paths = json.loads(${JSON.stringify(JSON.stringify(paths))})

def ser(value):
    if isinstance(value, bytes):
        return repr(value)
    if isinstance(value, (str, int, float, bool)) or value is None:
        return value
    if isinstance(value, list):
        return [ser(item) for item in value]
    if isinstance(value, tuple):
        return [ser(item) for item in value]
    if isinstance(value, dict):
        return {str(key): ser(item) for key, item in value.items() if str(key) in keys}
    return str(value)

def clean(value):
    if isinstance(value, float):
        return round(value, 6)
    if isinstance(value, list):
        return [clean(item) for item in value]
    if isinstance(value, dict):
        return {key: clean(item) for key, item in value.items()}
    return value

out = {}
for path in paths:
    with pdfplumber.open(path) as pdf:
        annots = []
        for annot in pdf.pages[0].annots:
            data = annot.get("data") or {}
            annots.append(clean({
                "x0": annot.get("x0"),
                "y0": annot.get("y0"),
                "x1": annot.get("x1"),
                "y1": annot.get("y1"),
                "top": annot.get("top"),
                "bottom": annot.get("bottom"),
                "uri": annot.get("uri"),
                "title": annot.get("title"),
                "contents": annot.get("contents"),
                "data": {key: ser(data[key]) for key in keys if key in data},
            }))
        out[path] = annots
print(json.dumps(out))
`;
  return JSON.parse(execFileSync("wsl_venv/bin/python", ["-c", code], { encoding: "utf8" })) as Record<string, Record<string, unknown>[]>;
}

describe("low-level pdfplumber annotation compatibility", () => {
  it("exposes decoded fields and parsed data dictionaries like Python pdfplumber", async () => {
    const paths = [
      "test/fixtures/micro-pdfs/annotations-extended.pdf",
      "pdfplumber-python/tests/pdfs/annotations.pdf",
      "pdfplumber-python/tests/pdfs/annotations-rotated-90.pdf",
      "pdfplumber-python/tests/pdfs/annotations-rotated-180.pdf",
      "pdfplumber-python/tests/pdfs/annotations-rotated-270.pdf"
    ];
    const expected = pdfplumberAnnotationOracle(paths);
    for (const path of paths) {
      const pdf = await open(path);
      try {
        expect(pdf.pages[0].annots.map(simplifyAnnot), path).toEqual(expected[path]);
      } finally {
        await pdf.close();
      }
    }
  });
});
