import { decompressSync } from "fflate";

import { asArray, asName, isName, latin1String } from "./primitives.js";
import type { PdfDict, PdfPrimitive, PdfStream } from "./primitives.js";
import { parseObject } from "./parser.js";

function filterNames(filterValue: PdfPrimitive | undefined): string[] {
  if (isName(filterValue)) return [filterValue.name];
  const array = asArray(filterValue);
  if (!array) return [];
  return array.map((item) => asName(item)).filter((name): name is string => name != null);
}

function ascii85Decode(value: string): Uint8Array {
  const out: number[] = [];
  let group: number[] = [];
  const flush = (partial = false): void => {
    if (!group.length) return;
    const length = group.length;
    if (partial) {
      while (group.length < 5) group.push(84);
    }
    let acc = 0;
    for (const code of group) acc = acc * 85 + code;
    const bytes = [(acc >>> 24) & 0xff, (acc >>> 16) & 0xff, (acc >>> 8) & 0xff, acc & 0xff];
    out.push(...bytes.slice(0, partial ? length - 1 : 4));
    group = [];
  };
  for (let i = 0; i < value.length; i += 1) {
    const char = value[i];
    if (/\s/.test(char)) continue;
    if (char === "~") break;
    if (char === "z" && group.length === 0) {
      out.push(0, 0, 0, 0);
      continue;
    }
    const code = char.charCodeAt(0) - 33;
    if (code < 0 || code > 84) continue;
    group.push(code);
    if (group.length === 5) flush();
  }
  flush(true);
  return Uint8Array.from(out);
}

function asciiHexDecode(value: string): Uint8Array {
  let clean = value.replace(/\s+/g, "");
  const eod = clean.indexOf(">");
  if (eod >= 0) clean = clean.slice(0, eod);
  if (clean.length % 2) clean += "0";
  const bytes = new Uint8Array(clean.length / 2);
  for (let i = 0; i < bytes.length; i += 1) bytes[i] = Number.parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  return bytes;
}

function runLengthDecode(bytes: Uint8Array): Uint8Array {
  const out: number[] = [];
  for (let i = 0; i < bytes.length; i += 1) {
    const length = bytes[i];
    if (length === 128) break;
    if (length <= 127) {
      out.push(...bytes.slice(i + 1, i + 2 + length));
      i += length + 1;
    } else {
      const repeat = 257 - length;
      const value = bytes[++i];
      for (let j = 0; j < repeat; j += 1) out.push(value);
    }
  }
  return Uint8Array.from(out);
}

function lzwDecode(bytes: Uint8Array): Uint8Array {
  const CLEAR = 256;
  const EOD = 257;
  let bit = 0;
  let codeSize = 9;
  const readCode = (): number | null => {
    if (bit + codeSize > bytes.length * 8) return null;
    let code = 0;
    for (let i = 0; i < codeSize; i += 1) {
      const byteIndex = (bit + i) >> 3;
      const bitIndex = 7 - ((bit + i) & 7);
      code = (code << 1) | ((bytes[byteIndex] >> bitIndex) & 1);
    }
    bit += codeSize;
    return code;
  };
  let table: number[][] = [];
  const reset = (): void => {
    table = Array.from({ length: 258 }, (_unused, index) => (index < 256 ? [index] : []));
    codeSize = 9;
  };
  reset();
  const out: number[] = [];
  let prev: number[] | null = null;
  while (true) {
    const code = readCode();
    if (code == null || code === EOD) break;
    if (code === CLEAR) {
      reset();
      prev = null;
      continue;
    }
    let entry = table[code];
    if ((!entry || entry.length === 0) && prev) entry = [...prev, prev[0]];
    if (!entry) break;
    out.push(...entry);
    if (prev) {
      table.push([...prev, entry[0]]);
      if (table.length === 1 << codeSize && codeSize < 12) codeSize += 1;
    }
    prev = entry;
  }
  return Uint8Array.from(out);
}

export function decodeStream(stream: PdfStream): Uint8Array {
  let bytes = stream.data;
  for (const filter of filterNames(stream.dict.get("Filter"))) {
    if (filter === "ASCII85Decode" || filter === "A85") bytes = ascii85Decode(latin1String(bytes));
    else if (filter === "ASCIIHexDecode" || filter === "AHx") bytes = asciiHexDecode(latin1String(bytes));
    else if (filter === "RunLengthDecode" || filter === "RL") bytes = runLengthDecode(bytes);
    else if (filter === "LZWDecode" || filter === "LZW") bytes = lzwDecode(bytes);
    else if (filter === "FlateDecode" || filter === "Fl") {
      try {
        bytes = decompressSync(bytes);
      } catch {
        return new Uint8Array();
      }
    }
  }
  return bytes;
}

export function decodeStreamToLatin1(stream: PdfStream): string {
  return latin1String(decodeStream(stream));
}

export function decodePdfStreamText(objectText: string | undefined): string {
  if (!objectText) return "";
  const object = parseObject(objectText);
  if (!object?.stream) return "";
  return decodeStreamToLatin1(object.stream);
}

export function streamDict(objectText: string | undefined): PdfDict | undefined {
  return objectText ? parseObject(objectText)?.stream?.dict : undefined;
}
