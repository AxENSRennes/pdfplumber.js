import { deflateSync } from "node:zlib";
import { execFileSync } from "node:child_process";

import { describe, expect, it } from "vitest";

import { decodePdfStream } from "../../src/pdf.js";

function latin1(bytes: Uint8Array): string {
  return Buffer.from(bytes).toString("latin1");
}

function pdfminerPngPredictorOracle(bytes: number[], options: { predictor: number; colors: number; columns: number; bitsPerComponent: number }): string {
  const code = `
import json
from pdfminer.utils import apply_png_predictor
data = bytes(${JSON.stringify(bytes)})
result = apply_png_predictor(${options.predictor}, ${options.colors}, ${options.columns}, ${options.bitsPerComponent}, data)
print(json.dumps(list(result)))
`;
  const result = execFileSync("wsl_venv/bin/python", ["-c", code], { encoding: "utf8" });
  return latin1(Uint8Array.from(JSON.parse(result) as number[]));
}

describe("low-level PDF stream decoding", () => {
  it("decodes raw streams with LF, CRLF, and CR stream line endings", () => {
    expect(decodePdfStream("1 0 obj\n<< /Length 5 >>\nstream\nhello\nendstream\nendobj")).toBe("hello");
    expect(decodePdfStream("1 0 obj\r\n<< /Length 5 >>\r\nstream\r\nhello\r\nendstream\r\nendobj")).toBe("hello");
    expect(decodePdfStream("1 0 obj\r<< /Length 5 >>\rstream\rhello\rendstream\rendobj")).toBe("hello");
  });

  it("decodes empty, ASCII85, and Flate streams while preserving PDF byte strings", () => {
    expect(decodePdfStream("1 0 obj\n<< /Length 0 >>\nstream\n\nendstream\nendobj")).toBe("");
    expect(decodePdfStream("1 0 obj\n<< /Filter /ASCII85Decode /Length 8 >>\nstream\n87cURD]j7BEbo80~>\nendstream\nendobj")).toBe("Hello world!");

    const compressed = latin1(deflateSync(Buffer.from("BT /F1 12 Tf (Hello) Tj ET", "latin1")));
    expect(decodePdfStream(`2 0 obj\n<< /Filter /FlateDecode /Length ${compressed.length} >>\nstream\n${compressed}\nendstream\nendobj`)).toBe(
      "BT /F1 12 Tf (Hello) Tj ET"
    );
  });

  it("decodes ASCII85 edge cases like pdfminer.ascii85decode", () => {
    const decode = (value: string): string => decodePdfStream(`1 0 obj\n<< /Filter /ASCII85Decode >>\nstream\n${value}\nendstream\nendobj`);

    expect(decode("9jqo^BlbD-BleB1DJ+*+F(f,q")).toBe("Man is distinguished");
    expect(decode("E,9)oF*2M7/c~>")).toBe("pleasure.");
    expect(decode("zE,9)oF*2M7/c~>")).toBe("\0\0\0\0pleasure.");
    expect(decode("E,9)oF*2M7/c")).toBe("pleasure.");
    expect(decode("E,9)oF*2M7/c~")).toBe("pleasure.");
    expect(decode("<~E,9)oF*2M7/c~")).toBe("pleasure.");
    expect(decode("<~E,9)oF*2M7/c~\n>")).toBe("pleasure.");
    expect(decode("<^BVT:K:=9<E)pd;BS_1:/aSV;ag~>")).toBe("VARIOUS UTTER NONSENSE");
    expect(decode("<~<^BVT:K:=9<E)pd;BS_1:/aSV;ag~>")).toBe("VARIOUS UTTER NONSENSE");
    expect(decode("<^BVT:K:=9<E)pd;BS_1:/aSV;ag~")).toBe("VARIOUS UTTER NONSENSE");
  });

  it("decodes ASCIIHex, RunLength, LZW, and filter chains", () => {
    expect(decodePdfStream("1 0 obj\n<< /Filter /ASCIIHexDecode >>\nstream\n61 62 2e6364   65\nendstream\nendobj")).toBe("ab.cde");
    expect(decodePdfStream("1 0 obj\n<< /Filter /ASCIIHexDecode >>\nstream\n61 62 2e6364 657>\nendstream\nendobj")).toBe("ab.cdep");
    expect(decodePdfStream("1 0 obj\n<< /Filter /ASCIIHexDecode >>\nstream\n7>\nendstream\nendobj")).toBe("p");
    const runLength = String.fromCharCode(5) + "123456" + String.fromCharCode(250) + "7" + String.fromCharCode(4) + "abcde" + String.fromCharCode(128) + "junk";
    expect(decodePdfStream(`1 0 obj\n<< /Filter /RunLengthDecode >>\nstream\n${runLength}\nendstream\nendobj`)).toBe("1234567777777abcde");
    const lzw = String.fromCharCode(128, 11, 96, 80, 34, 12, 12, 133, 1);
    expect(decodePdfStream(`1 0 obj\n<< /Filter /LZWDecode >>\nstream\n${lzw}\nendstream\nendobj`)).toBe("-----A---B");

    const flateHex = deflateSync(Buffer.from("chained", "latin1")).toString("hex");
    expect(decodePdfStream(`1 0 obj\n<< /Filter [/ASCIIHexDecode /FlateDecode] >>\nstream\n${flateHex}>\nendstream\nendobj`)).toBe("chained");
  });

  it("applies Flate PNG predictor DecodeParms like pdfminer", () => {
    const examples = [
      { bytes: [2, 100, 3, 2, 1, 255, 2, 1, 255], options: { predictor: 12, colors: 1, columns: 2, bitsPerComponent: 8 } },
      { bytes: [0, 0b10101010, 2, 0b00001111], options: { predictor: 12, colors: 1, columns: 8, bitsPerComponent: 1 } }
    ];

    for (const example of examples) {
      const compressed = latin1(deflateSync(Uint8Array.from(example.bytes)));
      const objectText = `1 0 obj
<< /Filter /FlateDecode /DecodeParms << /Predictor ${example.options.predictor} /Colors ${example.options.colors} /BitsPerComponent ${example.options.bitsPerComponent} /Columns ${example.options.columns} >> >>
stream
${compressed}
endstream
endobj`;

      expect(decodePdfStream(objectText)).toBe(pdfminerPngPredictorOracle(example.bytes, example.options));
    }
  });

  it("does not treat malformed or absent streams as decoded content", () => {
    expect(decodePdfStream("1 0 obj << /Length 3 >> not-a-stream endobj")).toBe("");
    expect(decodePdfStream("1 0 obj << /Filter /FlateDecode >> stream\nnot-deflated\nendstream endobj")).toBe("");
  });
});
