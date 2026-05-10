import { deflateSync } from "node:zlib";

import { describe, expect, it } from "vitest";

import { decodePdfStream } from "../../src/pdf.js";

function latin1(bytes: Uint8Array): string {
  return Buffer.from(bytes).toString("latin1");
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

  it("decodes ASCIIHex, RunLength, LZW, and filter chains", () => {
    expect(decodePdfStream("1 0 obj\n<< /Filter /ASCIIHexDecode >>\nstream\n61 62 2e6364 657>\nendstream\nendobj")).toBe("ab.cdep");
    const runLength = String.fromCharCode(5) + "123456" + String.fromCharCode(250) + "7" + String.fromCharCode(4) + "abcde" + String.fromCharCode(128) + "junk";
    expect(decodePdfStream(`1 0 obj\n<< /Filter /RunLengthDecode >>\nstream\n${runLength}\nendstream\nendobj`)).toBe("1234567777777abcde");
    const lzw = String.fromCharCode(128, 11, 96, 80, 34, 12, 12, 133, 1);
    expect(decodePdfStream(`1 0 obj\n<< /Filter /LZWDecode >>\nstream\n${lzw}\nendstream\nendobj`)).toBe("-----A---B");

    const flateHex = deflateSync(Buffer.from("chained", "latin1")).toString("hex");
    expect(decodePdfStream(`1 0 obj\n<< /Filter [/ASCIIHexDecode /FlateDecode] >>\nstream\n${flateHex}>\nendstream\nendobj`)).toBe("chained");
  });

  it("does not treat malformed or absent streams as decoded content", () => {
    expect(decodePdfStream("1 0 obj << /Length 3 >> not-a-stream endobj")).toBe("");
    expect(decodePdfStream("1 0 obj << /Filter /FlateDecode >> stream\nnot-deflated\nendstream endobj")).toBe("");
  });
});
