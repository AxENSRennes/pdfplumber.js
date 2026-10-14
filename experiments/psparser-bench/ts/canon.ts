// Corpus reader and the canonical serialization shared with dump_corpus.py.
import { InlineStream, PDFContentParser, PSEOF, PSKeyword, PSLiteral, PyError, type PSValue } from "./psparser.ts";

export function readCorpus(data: Uint8Array): Uint8Array[][] {
  const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
  if (String.fromCharCode(...data.subarray(0, 4)) !== "PSC1") throw new Error("bad corpus");
  const n = view.getUint32(4, true);
  let off = 8;
  const jobs: Uint8Array[][] = [];
  for (let j = 0; j < n; j += 1) {
    const ns = view.getUint32(off, true);
    off += 4;
    const streams: Uint8Array[] = [];
    for (let s = 0; s < ns; s += 1) {
      const len = view.getUint32(off, true);
      off += 4;
      streams.push(data.subarray(off, off + len));
      off += len;
    }
    jobs.push(streams);
  }
  return jobs;
}

const HEX = Array.from({ length: 256 }, (_, i) => i.toString(16).padStart(2, "0"));
const hex = (b: Uint8Array) => {
  let s = "";
  for (const c of b) s += HEX[c];
  return s;
};
const utf8hex = (s: string) => hex(new TextEncoder().encode(s));
const f64 = new DataView(new ArrayBuffer(8));

export function canon(v: PSValue): string {
  if (v === true) return "T";
  if (v === false) return "F";
  if (typeof v === "bigint") return "d" + v.toString();
  if (typeof v === "number") {
    if (Number.isSafeInteger(v)) return "d" + String(v === 0 ? 0 : v);
    f64.setFloat64(0, v);
    let s = "f";
    for (let i = 0; i < 8; i += 1) s += HEX[f64.getUint8(i)];
    return s;
  }
  if (v instanceof Uint8Array) return "s" + hex(v);
  if (v instanceof PSLiteral) return typeof v.name === "string" ? "n" + utf8hex(v.name) : "N" + hex(v.name);
  if (v instanceof PSKeyword) return "k" + hex(v.name);
  if (Array.isArray(v)) return "[" + v.map(canon).join(",") + "]";
  if (v instanceof Map) return "{" + [...v].map(([k, x]) => utf8hex(k) + "=" + canon(x)).join(",") + "}";
  if (v instanceof InlineStream) return "S" + canon(v.attrs) + "|" + hex(v.rawdata);
  throw new Error("unexpected value");
}

/** Same text as dump_corpus.serialize(): one `pos:canon` line per object, then the outcome. */
export function serializeJob(streams: Uint8Array[], next: (streams: Uint8Array[]) => Iterable<[number, PSValue]> | { error: string; items: Array<[number, PSValue]> }): string {
  const lines: string[] = [];
  let outcome = "eof";
  const result = next(streams);
  const items = "items" in result ? result.items : [...result];
  for (const [pos, obj] of items) lines.push(`${pos}:${canon(obj)}`);
  if ("error" in result) outcome = result.error;
  lines.push("!" + outcome);
  return lines.join("\n") + "\n";
}

/** Run the TS parser over a job the way pdfminer's execute() loop does. */
export function parseJobTs(streams: Uint8Array[]): { error: string; items: Array<[number, PSValue]> } {
  const items: Array<[number, PSValue]> = [];
  let parser: PDFContentParser;
  try {
    parser = new PDFContentParser(streams);
  } catch (e) {
    if (e instanceof PSEOF) return { error: "eof", items };
    throw e;
  }
  for (;;) {
    try {
      const obj = parser.nextobject();
      items.push([parser.objpos, obj]);
    } catch (e) {
      if (e instanceof PSEOF) return { error: "eof", items };
      if (e instanceof PyError) return { error: "error:" + e.pyName, items };
      throw e;
    }
  }
}
