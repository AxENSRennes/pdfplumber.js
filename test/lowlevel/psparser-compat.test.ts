import { execFileSync } from "node:child_process";

import { describe, expect, it } from "vitest";

import { parseContentStream } from "../../src/pdf/parser.js";

interface PsParserBoundaryOracle {
  beginPositions: number[];
  keywordValues: string[];
}

type TokenOracle = Array<{ type: string; value: string | number; start: number }>;

function boundaryData(): string {
  const first = "beginbfchar\n";
  const boundaryStart = 4093;
  return `${first}${" ".repeat(boundaryStart - first.length)}beginbfchar\nbeginbfchar\nend\nend`;
}

function pdfminerBoundaryOracle(data: string): PsParserBoundaryOracle {
  const code = `
import json
from io import BytesIO
from pdfminer.psparser import KWD, PSEOF, PSBaseParser, PSKeyword

data = ${JSON.stringify(data)}.encode("latin-1")
parser = PSBaseParser(BytesIO(data))
beginbfchar = KWD(b"beginbfchar")
positions = []
keywords = []
while True:
    try:
        pos, token = parser.nexttoken()
        if isinstance(token, PSKeyword):
            keywords.append(token.name.decode("latin-1"))
        if token is beginbfchar:
            positions.append(pos)
    except PSEOF:
        break
print(json.dumps({"beginPositions": positions, "keywordValues": keywords}))
`;
  return JSON.parse(execFileSync("wsl_venv/bin/python", ["-c", code], { encoding: "utf8" })) as PsParserBoundaryOracle;
}

function pdfminerTokenOracle(data: string): TokenOracle {
  const code = `
import json
from io import BytesIO
from pdfminer.psparser import PSEOF, PSBaseParser, PSKeyword, PSLiteral

data = ${JSON.stringify(data)}.encode("latin-1")
parser = PSBaseParser(BytesIO(data))
tokens = []
def name(value):
    return value.decode("latin-1") if isinstance(value, bytes) else str(value)
while True:
    try:
        pos, token = parser.nexttoken()
        if isinstance(token, PSKeyword):
            tokens.append({"type": "keyword", "value": name(token.name), "start": pos})
        elif isinstance(token, PSLiteral):
            tokens.append({"type": "name", "value": name(token.name), "start": pos})
        elif isinstance(token, bytes):
            tokens.append({"type": "string", "value": token.decode("latin-1"), "start": pos})
        elif isinstance(token, (int, float)):
            tokens.append({"type": "number", "value": token, "start": pos})
    except PSEOF:
        break
print(json.dumps(tokens))
`;
  return JSON.parse(execFileSync("wsl_venv/bin/python", ["-c", code], { encoding: "utf8" })) as TokenOracle;
}

describe("low-level pdfminer PS parser compatibility", () => {
  it("keeps keyword tokens whole when they cross the pdfminer buffer boundary", () => {
    const data = boundaryData();
    const oracle = pdfminerBoundaryOracle(data);
    const keywords = parseContentStream(data).filter((token) => token.type === "keyword");

    expect(oracle.beginPositions).toContain(4093);
    expect(keywords.map((token) => token.value)).toEqual(oracle.keywordValues);
    expect(keywords.filter((token) => token.value === "beginbfchar").map((token) => token.start)).toEqual(oracle.beginPositions);
    expect(keywords.slice(-2).map((token) => token.value)).toEqual(oracle.keywordValues.slice(-2));
  });

  it("splits malformed numeric prefixes out of bare content tokens like pdfminer", () => {
    const data = "2-33-2m503 66l 3-5 --3 1.2.3 .400.3k +abc l3";
    const actual = parseContentStream(data).map((token) => ({
      type: token.type,
      value: token.type === "number" || token.type === "keyword" ? token.value : "",
      start: token.start
    }));

    expect(actual).toEqual(pdfminerTokenOracle(data));
  });
});
