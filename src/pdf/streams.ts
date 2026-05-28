import { decompressSync } from "fflate";

import { asArray, asDict, asName, asNumber, isName, latin1String } from "./primitives.js";
import type { PdfDict, PdfPrimitive, PdfStream } from "./primitives.js";
import { parseObject } from "./parser.js";

function filterNames(filterValue: PdfPrimitive | undefined): string[] {
  if (isName(filterValue)) return [filterValue.name];
  const array = asArray(filterValue);
  if (!array) return [];
  return array.map((item) => asName(item)).filter((name): name is string => name != null);
}

function decodeParams(stream: PdfStream, index: number): PdfDict | undefined {
  const params = stream.dict.get("DecodeParms") ?? stream.dict.get("DP");
  const array = asArray(params);
  if (array) return asDict(array[index]);
  return asDict(params);
}

function dictNumber(dict: PdfDict, key: string, fallback: number): number {
  return asNumber(dict.get(key)) ?? fallback;
}

function paethPredictor(left: number, above: number, upperLeft: number): number {
  const estimate = left + above - upperLeft;
  const leftDistance = Math.abs(estimate - left);
  const aboveDistance = Math.abs(estimate - above);
  const upperLeftDistance = Math.abs(estimate - upperLeft);
  if (leftDistance <= aboveDistance && leftDistance <= upperLeftDistance) return left;
  return aboveDistance <= upperLeftDistance ? above : upperLeft;
}

function applyTiffPredictor(bytes: Uint8Array, colors: number, columns: number, bitsPerComponent: number): Uint8Array {
  if (bitsPerComponent !== 8) return bytes;
  const bytesPerPixel = Math.floor((colors * bitsPerComponent) / 8);
  const rowLength = columns * bytesPerPixel;
  if (!Number.isInteger(rowLength) || rowLength <= 0) return bytes;
  const out: number[] = [];
  for (let rowOffset = 0; rowOffset < bytes.length; rowOffset += rowLength) {
    const raw: number[] = [];
    const row = bytes.subarray(rowOffset, rowOffset + rowLength);
    for (let i = 0; i < row.length; i += 1) {
      const prior = i >= bytesPerPixel ? raw[i - bytesPerPixel] : 0;
      raw.push((row[i] + prior) & 0xff);
    }
    out.push(...raw);
  }
  return Uint8Array.from(out);
}

function applyPngPredictor(bytes: Uint8Array, colors: number, columns: number, bitsPerComponent: number): Uint8Array {
  if (bitsPerComponent !== 8 && bitsPerComponent !== 1) return bytes;
  const rowLength = Math.floor((colors * columns * bitsPerComponent) / 8);
  const bytesPerPixel = Math.floor((colors * bitsPerComponent) / 8);
  if (rowLength <= 0) return bytes;

  const out: number[] = [];
  let priorRow = new Uint8Array(rowLength);
  for (let rowOffset = 0; rowOffset < bytes.length; rowOffset += rowLength + 1) {
    const filterType = bytes[rowOffset];
    const encoded = bytes.subarray(rowOffset + 1, rowOffset + 1 + rowLength);
    const raw = new Uint8Array(encoded.length);
    for (let i = 0; i < encoded.length; i += 1) {
      const left = i >= bytesPerPixel && bytesPerPixel > 0 ? raw[i - bytesPerPixel] : 0;
      const above = priorRow[i] ?? 0;
      const upperLeft = i >= bytesPerPixel && bytesPerPixel > 0 ? (priorRow[i - bytesPerPixel] ?? 0) : 0;
      const predictor =
        filterType === 0 ? 0 :
        filterType === 1 ? left :
        filterType === 2 ? above :
        filterType === 3 ? Math.floor((left + above) / 2) :
        filterType === 4 ? paethPredictor(left, above, upperLeft) :
        null;
      if (predictor == null) return bytes;
      raw[i] = (encoded[i] + predictor) & 0xff;
    }
    out.push(...raw);
    priorRow = raw;
  }
  return Uint8Array.from(out);
}

function applyPredictor(bytes: Uint8Array, params: PdfDict | undefined): Uint8Array {
  if (!params) return bytes;
  const predictor = dictNumber(params, "Predictor", 1);
  if (predictor === 1) return bytes;
  const colors = dictNumber(params, "Colors", 1);
  const columns = dictNumber(params, "Columns", 1);
  const bitsPerComponent = dictNumber(params, "BitsPerComponent", 8);
  if (predictor === 2) return applyTiffPredictor(bytes, colors, columns, bitsPerComponent);
  if (predictor >= 10) return applyPngPredictor(bytes, colors, columns, bitsPerComponent);
  return bytes;
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
    if (char === "<" && value[i + 1] === "~") {
      i += 1;
      continue;
    }
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
  const filters = filterNames(stream.dict.get("Filter"));
  for (let index = 0; index < filters.length; index += 1) {
    const filter = filters[index];
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
    bytes = applyPredictor(bytes, decodeParams(stream, index));
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
