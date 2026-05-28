import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import { CMapLikePdfminer, parseCompressedCMapJsonLikePdfminer, UnicodeMapLikePdfminer } from "../../src/pdf/cmapdb.js";

function pdfminerOracle(): { cmap: string; unicodeMap: string; cid1: string; hDecode: number[] } {
  const code = `
import json
from pdfminer.cmapdb import CMapDB

cmap = CMapDB.get_cmap("H")
umap = CMapDB.get_unicode_map("Adobe-Japan1", vertical=False)
print(json.dumps({
    "cmap": str(cmap),
    "unicodeMap": str(umap),
    "cid1": umap.cid2unichr[1],
    "hDecode": list(cmap.decode(bytes([33, 33, 33, 34]))),
}))
`;
  return JSON.parse(execFileSync("wsl_venv/bin/python", ["-c", code], { encoding: "utf8" })) as { cmap: string; unicodeMap: string; cid1: string; hDecode: number[] };
}

function allKeysAreNumbers(map: Map<number, unknown>): boolean {
  for (const [key, value] of map) {
    if (typeof key !== "number") return false;
    if (value instanceof Map && !allKeysAreNumbers(value)) return false;
  }
  return true;
}

describe("low-level pdfminer CMapDB JSON compatibility", () => {
  it("loads standard CMap JSON data and restores nested CODE2CID integer keys", () => {
    const expected = pdfminerOracle();
    const data = parseCompressedCMapJsonLikePdfminer(readFileSync("pdfminer-six/pdfminer/cmap/H.json.gz"));
    const cmap = new CMapLikePdfminer("H", data);

    expect(String(cmap)).toBe(expected.cmap);
    expect(allKeysAreNumbers(cmap.code2cid)).toBe(true);
    expect(cmap.decode([33, 33, 33, 34])).toEqual(expected.hDecode);
  });

  it("loads Unicode CMap JSON data and restores CID integer keys", () => {
    const expected = pdfminerOracle();
    const data = parseCompressedCMapJsonLikePdfminer(readFileSync("pdfminer-six/pdfminer/cmap/to-unicode-Adobe-Japan1.json.gz"));
    const unicodeMap = new UnicodeMapLikePdfminer("Adobe-Japan1", data, false);

    expect(String(unicodeMap)).toBe(expected.unicodeMap);
    expect([...unicodeMap.cid2unichr.keys()].every((key) => typeof key === "number")).toBe(true);
    expect(unicodeMap.getUnicode(1)).toBe(expected.cid1);
  });
});
