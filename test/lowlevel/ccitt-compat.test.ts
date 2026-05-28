import { execFileSync } from "node:child_process";

import { describe, expect, it } from "vitest";

import { CCITTFaxDecoderLikePdfminer, CCITTG4ParserLikePdfminer } from "../../src/pdf/ccitt.js";

type CcittOperation = readonly ["vertical", number] | readonly ["pass"] | readonly ["horizontal", number, number];

interface CcittCase {
  name: string;
  bits: string;
  operations: CcittOperation[];
  curpos?: number;
  color?: number;
}

interface CcittOracle {
  curpos: number;
  color: number;
  bits: string;
}

const upstreamParserCases: CcittCase[] = [
  { name: "b1", bits: "00000", operations: [["vertical", 0]] },
  { name: "b2", bits: "10000", operations: [["vertical", -1]] },
  { name: "b3", bits: "000111", operations: [["pass"]] },
  { name: "b4", bits: "00000", operations: [["vertical", 2]] },
  { name: "b5", bits: "11111111100", operations: [["horizontal", 0, 3], ["vertical", 1]] },
  { name: "e1", bits: "10000", operations: [["vertical", 0], ["vertical", 0]] },
  { name: "e2", bits: "10011", operations: [["vertical", 0], ["vertical", 2]] },
  { name: "e3", bits: "011111", color: 0, operations: [["vertical", 0], ["vertical", -2], ["vertical", 0]] },
  { name: "e4", bits: "10000", operations: [["vertical", 0], ["vertical", -2], ["vertical", 0]] },
  { name: "e5", bits: "011000", color: 0, operations: [["vertical", 0], ["vertical", 3]] },
  { name: "e6", bits: "11001", operations: [["pass"], ["vertical", 0]] },
  { name: "e7", bits: "0000000000", curpos: 2, color: 1, operations: [["horizontal", 2, 6]] },
  { name: "e8", bits: "001100000", curpos: 1, color: 0, operations: [["vertical", 0], ["horizontal", 7, 0]] },
  { name: "m1", bits: "10101", operations: [["pass"], ["pass"]] },
  { name: "m2", bits: "101011", operations: [["vertical", -1], ["vertical", -1], ["vertical", 1], ["horizontal", 1, 1]] },
  { name: "m3", bits: "10111011", operations: [["vertical", -1], ["pass"], ["vertical", 1], ["vertical", 1]] }
];

function pdfminerParserOracle(testCase: CcittCase): CcittOracle {
  const code = `
import json
from pdfminer.ccitt import CCITTG4Parser

payload = json.loads(${JSON.stringify(JSON.stringify(testCase))})
parser = CCITTG4Parser(len(payload["bits"]))
parser._curline = [int(c) for c in payload["bits"]]
parser._reset_line()
if payload.get("curpos") is not None:
    parser._curpos = payload["curpos"]
if payload.get("color") is not None:
    parser._color = payload["color"]

for operation in payload["operations"]:
    if operation[0] == "vertical":
        parser._do_vertical(operation[1])
    elif operation[0] == "pass":
        parser._do_pass()
    elif operation[0] == "horizontal":
        parser._do_horizontal(operation[1], operation[2])

print(json.dumps({
    "curpos": parser._curpos,
    "color": parser._color,
    "bits": parser._get_bits(),
}))
`;
  return JSON.parse(execFileSync("wsl_venv/bin/python", ["-c", code], { encoding: "utf8" })) as CcittOracle;
}

function runJsParserCase(testCase: CcittCase): CcittOracle {
  const parser = new CCITTG4ParserLikePdfminer(testCase.bits.length);
  parser.setCurrentLine(testCase.bits);
  if (testCase.curpos != null) parser.curposLikePdfminer = testCase.curpos;
  if (testCase.color != null) parser.colorLikePdfminer = testCase.color;

  for (const operation of testCase.operations) {
    if (operation[0] === "vertical") parser.doVerticalLikePdfminer(operation[1]);
    else if (operation[0] === "pass") parser.doPassLikePdfminer();
    else parser.doHorizontalLikePdfminer(operation[1], operation[2]);
  }

  return {
    curpos: parser.curposLikePdfminer,
    color: parser.colorLikePdfminer,
    bits: parser.bitsLikePdfminer()
  };
}

function pdfminerFaxOutputOracle(width: number, bits: string): number[] {
  const code = `
import json
from pdfminer.ccitt import CCITTFaxDecoder

width = ${JSON.stringify(width)}
bits = [int(c) for c in ${JSON.stringify(bits)}]
decoder = CCITTFaxDecoder(width)
decoder.output_line(0, bits)
print(json.dumps(list(decoder.close())))
`;
  return JSON.parse(execFileSync("wsl_venv/bin/python", ["-c", code], { encoding: "utf8" })) as number[];
}

describe("low-level pdfminer CCITT compatibility", () => {
  it.each(upstreamParserCases)("matches CCITTG4Parser line state for upstream $name", (testCase) => {
    expect(runJsParserCase(testCase)).toEqual(pdfminerParserOracle(testCase));
  });

  it("packs CCITTFaxDecoder output lines like pdfminer", () => {
    const decoder = new CCITTFaxDecoderLikePdfminer(5);
    decoder.outputLineLikePdfminer([0]);

    expect([...decoder.closeLikePdfminer()]).toEqual(pdfminerFaxOutputOracle(5, "0"));
  });
});
