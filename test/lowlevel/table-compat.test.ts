import { execFileSync } from "node:child_process";

import { describe, expect, it } from "vitest";

import { open } from "../../src/index.js";

function clean(value: unknown): unknown {
  if (typeof value === "number") return Number(value.toFixed(6));
  if (Array.isArray(value)) return value.map(clean);
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, clean(item)]));
  }
  return value;
}

function pdfplumberTableAxisOracle(path: string): Record<string, unknown> {
  const code = `
import json
import pdfplumber

with pdfplumber.open(${JSON.stringify(path)}) as pdf:
    table = pdf.pages[0].find_table()
    print(json.dumps({
        "row_count": len(table.rows),
        "column_count": len(table.columns),
        "first_row": {"bbox": table.rows[0].bbox, "cells": table.rows[0].cells},
        "first_column": {"bbox": table.columns[0].bbox, "cells": table.columns[0].cells},
    }, default=str))
`;
  return clean(JSON.parse(execFileSync("wsl_venv/bin/python", ["-c", code], { encoding: "utf8" }))) as Record<string, unknown>;
}

describe("low-level pdfplumber table compatibility", () => {
  it("exposes row and column bbox/cells like Python pdfplumber", async () => {
    const path = "pdfplumber-python/tests/pdfs/mcid_example.pdf";
    const expected = pdfplumberTableAxisOracle(path);
    const pdf = await open(path);
    try {
      const table = await pdf.pages[0].findTable();
      expect(table).not.toBeNull();
      expect(clean({
        row_count: table?.rows.length,
        column_count: table?.columns.length,
        first_row: { bbox: table?.rows[0].bbox, cells: table?.rows[0].cells },
        first_column: { bbox: table?.columns[0].bbox, cells: table?.columns[0].cells }
      })).toEqual(expected);
    } finally {
      await pdf.close();
    }
  });
});
