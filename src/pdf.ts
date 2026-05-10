import { inflateSync } from "node:zlib";

import * as pdfjs from "pdfjs-dist/legacy/build/pdf.mjs";

import { DEFAULT_FONT_ASCENT, DEFAULT_FONT_DESCENT, FONT_UNITS_PER_EM, METADATA_KEYS, STANDARD_FONT_METRICS } from "./constants.js";
import { glyphTextFromPdfJsGlyph, glyphWidthLikePdfminer } from "./font-decoding.js";
import { decodePdfLiteralBytesAsUtf8ThenUtf16, decodePdfStringLikePdfminer } from "./pdf-strings.js";
import type {
  BBox,
  ColorOp,
  FontRecord,
  GraphicsState,
  ImageResource,
  MappedFont,
  Matrix,
  MutableBBox,
  OpenOptions,
  PageBoxes,
  ParsedPath,
  PDFObject,
  PdfEncryption,
  PdfJsGlyph,
  Point
} from "./types.js";
import {
  applyMatrix,
  bboxFromPoints,
  cleanBBox,
  cleanMatrix,
  cleanNumber,
  cleanObject,
  cloneMatrix,
  colorSpaceName,
  colorValue,
  firstFinite,
  graphicColorValue,
  lineWidthScale,
  matrixToPageMatrix,
  multiplyMatrix,
  pointToPageCoords,
  pythonBytesName,
  rectFromPdfBBox,
  rgbColor,
  snapPdfOperand,
  snapPdfPathCoordinate,
  snapPdfPathExtent,
  snapPathOperandPoint,
  softenHalfMicro
} from "./utils.js";

const PDF_PASSWORD_PADDING = Uint8Array.from([
  0x28, 0xbf, 0x4e, 0x5e, 0x4e, 0x75, 0x8a, 0x41,
  0x64, 0x00, 0x4e, 0x56, 0xff, 0xfa, 0x01, 0x08,
  0x2e, 0x2e, 0x00, 0xb6, 0xd0, 0x68, 0x3e, 0x80,
  0x2f, 0x0c, 0xa9, 0xfe, 0x64, 0x53, 0x69, 0x7a
]);

function latin1Bytes(value: string): Uint8Array {
  const bytes = new Uint8Array(value.length);
  for (let i = 0; i < value.length; i += 1) bytes[i] = value.charCodeAt(i) & 0xff;
  return bytes;
}

function latin1String(bytes: Uint8Array): string {
  let out = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) out += String.fromCharCode(...bytes.subarray(i, i + chunk));
  return out;
}

function hexBytes(value: string): Uint8Array {
  const clean = value.replace(/\s+/g, "");
  const padded = clean.length % 2 ? `${clean}0` : clean;
  const bytes = new Uint8Array(padded.length / 2);
  for (let i = 0; i < bytes.length; i += 1) bytes[i] = parseInt(padded.slice(i * 2, i * 2 + 2), 16);
  return bytes;
}

function concatBytes(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

function int32LittleEndian(value: number): Uint8Array {
  const out = new Uint8Array(4);
  const unsigned = value >>> 0;
  out[0] = unsigned & 0xff;
  out[1] = (unsigned >>> 8) & 0xff;
  out[2] = (unsigned >>> 16) & 0xff;
  out[3] = (unsigned >>> 24) & 0xff;
  return out;
}

function pdfPasswordBytes(password: string): Uint8Array {
  const raw = latin1Bytes(password);
  const out = new Uint8Array(32);
  const used = Math.min(raw.length, 32);
  out.set(raw.subarray(0, used));
  if (used < 32) out.set(PDF_PASSWORD_PADDING.subarray(0, 32 - used), used);
  return out;
}

function md5Bytes(input: Uint8Array): Uint8Array {
  const s = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
    5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
    4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
    6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21
  ];
  const k = Array.from({ length: 64 }, (_, i) => Math.floor(Math.abs(Math.sin(i + 1)) * 0x100000000) >>> 0);
  const bitLength = input.length * 8;
  const paddedLength = (((input.length + 8) >>> 6) + 1) << 6;
  const padded = new Uint8Array(paddedLength);
  padded.set(input);
  padded[input.length] = 0x80;
  let low = bitLength >>> 0;
  let high = Math.floor(bitLength / 0x100000000) >>> 0;
  for (let i = 0; i < 4; i += 1) {
    padded[paddedLength - 8 + i] = low & 0xff;
    low >>>= 8;
    padded[paddedLength - 4 + i] = high & 0xff;
    high >>>= 8;
  }

  let a0 = 0x67452301;
  let b0 = 0xefcdab89;
  let c0 = 0x98badcfe;
  let d0 = 0x10325476;
  const m = new Uint32Array(16);
  const leftRotate = (value: number, amount: number): number => ((value << amount) | (value >>> (32 - amount))) >>> 0;

  for (let offset = 0; offset < padded.length; offset += 64) {
    for (let i = 0; i < 16; i += 1) {
      const j = offset + i * 4;
      m[i] = (padded[j] | (padded[j + 1] << 8) | (padded[j + 2] << 16) | (padded[j + 3] << 24)) >>> 0;
    }
    let a = a0;
    let b = b0;
    let c = c0;
    let d = d0;
    for (let i = 0; i < 64; i += 1) {
      let f: number;
      let g: number;
      if (i < 16) {
        f = (b & c) | (~b & d);
        g = i;
      } else if (i < 32) {
        f = (d & b) | (~d & c);
        g = (5 * i + 1) % 16;
      } else if (i < 48) {
        f = b ^ c ^ d;
        g = (3 * i + 5) % 16;
      } else {
        f = c ^ (b | ~d);
        g = (7 * i) % 16;
      }
      const temp = d;
      d = c;
      c = b;
      b = (b + leftRotate((a + f + k[i] + m[g]) >>> 0, s[i])) >>> 0;
      a = temp;
    }
    a0 = (a0 + a) >>> 0;
    b0 = (b0 + b) >>> 0;
    c0 = (c0 + c) >>> 0;
    d0 = (d0 + d) >>> 0;
  }

  const out = new Uint8Array(16);
  for (const [index, value] of [a0, b0, c0, d0].entries()) {
    out[index * 4] = value & 0xff;
    out[index * 4 + 1] = (value >>> 8) & 0xff;
    out[index * 4 + 2] = (value >>> 16) & 0xff;
    out[index * 4 + 3] = (value >>> 24) & 0xff;
  }
  return out;
}

function rc4Bytes(data: Uint8Array, key: Uint8Array): Uint8Array {
  const s = new Uint8Array(256);
  for (let i = 0; i < 256; i += 1) s[i] = i;
  let j = 0;
  for (let i = 0; i < 256; i += 1) {
    j = (j + s[i] + key[i % key.length]) & 0xff;
    [s[i], s[j]] = [s[j], s[i]];
  }
  const out = new Uint8Array(data.length);
  let i = 0;
  j = 0;
  for (let n = 0; n < data.length; n += 1) {
    i = (i + 1) & 0xff;
    j = (j + s[i]) & 0xff;
    [s[i], s[j]] = [s[j], s[i]];
    out[n] = data[n] ^ s[(s[i] + s[j]) & 0xff];
  }
  return out;
}

function parsePdfLiteralStringBytes(source: string, start: number): { bytes: Uint8Array; end: number } | null {
  if (source[start] !== "(") return null;
  const out: number[] = [];
  let depth = 1;
  let index = start + 1;
  while (index < source.length && depth > 0) {
    const char = source[index++];
    if (char === "\\") {
      if (index >= source.length) break;
      const escaped = source[index++];
      if (escaped === "n") out.push(0x0a);
      else if (escaped === "r") out.push(0x0d);
      else if (escaped === "t") out.push(0x09);
      else if (escaped === "b") out.push(0x08);
      else if (escaped === "f") out.push(0x0c);
      else if (escaped === "\r" || escaped === "\n") {
        if (escaped === "\r" && source[index] === "\n") index += 1;
      } else if (/[0-7]/.test(escaped)) {
        let octal = escaped;
        for (let i = 0; i < 2 && /[0-7]/.test(source[index] ?? ""); i += 1) octal += source[index++];
        out.push(parseInt(octal, 8) & 0xff);
      } else {
        out.push(escaped.charCodeAt(0) & 0xff);
      }
    } else if (char === "(") {
      depth += 1;
      out.push(0x28);
    } else if (char === ")") {
      depth -= 1;
      if (depth > 0) out.push(0x29);
    } else {
      out.push(char.charCodeAt(0) & 0xff);
    }
  }
  return depth === 0 ? { bytes: Uint8Array.from(out), end: index } : null;
}

function parsePdfDictBytes(source: string, key: string): Uint8Array | null {
  const match = new RegExp(`/${key}\\b`).exec(source);
  if (!match) return null;
  let index = match.index + match[0].length;
  while (/\s/.test(source[index] ?? "")) index += 1;
  if (source[index] === "(") return parsePdfLiteralStringBytes(source, index)?.bytes ?? null;
  if (source[index] === "<" && source[index + 1] !== "<") {
    const end = source.indexOf(">", index + 1);
    return end >= 0 ? hexBytes(source.slice(index + 1, end)) : null;
  }
  return null;
}

function parseFirstFileId(raw: string): Uint8Array | null {
  const hex = raw.match(/\/ID\s*\[\s*<([0-9A-Fa-f\s]+)>/)?.[1];
  if (hex) return hexBytes(hex);
  const literalStart = raw.search(/\/ID\s*\[\s*\(/);
  if (literalStart >= 0) {
    const open = raw.indexOf("(", literalStart);
    return open >= 0 ? parsePdfLiteralStringBytes(raw, open)?.bytes ?? null : null;
  }
  return null;
}

function parseCryptFilterMethod(encryptText: string, filterName: string | undefined, fallback: PdfEncryption["streamMethod"]): PdfEncryption["streamMethod"] {
  if (!filterName || filterName === "Identity") return filterName === "Identity" ? "Identity" : fallback;
  const match = new RegExp(`/${filterName}\\s*<<([\\s\\S]*?)>>`).exec(encryptText);
  const cfm = match?.[1].match(/\/CFM\s*\/([^\s/<>[\]()]+)/)?.[1];
  if (!cfm || cfm === "None" || cfm === "Identity") return cfm ? "Identity" : fallback;
  if (cfm === "V2") return "RC4";
  return "Unsupported";
}

function parsePdfEncryption(raw: string, objects: Map<number, string>, password = ""): PdfEncryption | null {
  const encryptRef = Number(raw.match(/\/Encrypt\s+(\d+)\s+\d+\s+R/)?.[1] ?? Number.NaN);
  if (!Number.isFinite(encryptRef)) return null;
  const encryptText = objects.get(encryptRef) ?? "";
  if (!/\/Filter\s*\/Standard\b/.test(encryptText)) return null;
  const owner = parsePdfDictBytes(encryptText, "O");
  const permissions = Number(encryptText.match(/\/P\s+(-?\d+)/)?.[1] ?? Number.NaN);
  const revision = Number(encryptText.match(/\/R\s+(\d+)/)?.[1] ?? 0);
  const version = Number(encryptText.match(/\/V\s+(\d+)/)?.[1] ?? 0);
  const lengthValues = Array.from(encryptText.matchAll(/\/Length\s+(\d+)/g), (match) => Number(match[1]));
  const lengthBits = lengthValues.length ? Math.max(...lengthValues) : (version === 1 ? 40 : 40);
  const fileId = parseFirstFileId(raw);
  if (!owner || !fileId || !Number.isFinite(permissions) || revision >= 5) return null;
  const keyLength = Math.max(5, Math.min(16, Math.floor(lengthBits / 8) || 5));
  const encryptMetadata = !/\/EncryptMetadata\s+false\b/.test(encryptText);
  let hashInput = concatBytes(pdfPasswordBytes(password), owner, int32LittleEndian(permissions), fileId);
  if (revision >= 4 && !encryptMetadata) hashInput = concatBytes(hashInput, Uint8Array.of(0xff, 0xff, 0xff, 0xff));
  let hash = md5Bytes(hashInput);
  if (revision >= 3) {
    for (let i = 0; i < 50; i += 1) hash = md5Bytes(hash.subarray(0, keyLength));
  }
  const defaultMethod: PdfEncryption["streamMethod"] = version === 1 || version === 2 ? "RC4" : "Unsupported";
  const stmf = encryptText.match(/\/StmF\s*\/([^\s/<>[\]()]+)/)?.[1];
  return {
    encryptRef,
    key: hash.subarray(0, keyLength),
    keyLength,
    streamMethod: version === 4 ? parseCryptFilterMethod(encryptText, stmf, defaultMethod) : defaultMethod,
    encryptMetadata
  };
}

function pdfObjectRefFromText(objectText: string): { num: number; gen: number } | null {
  const match = objectText.match(/^\s*(\d+)\s+(\d+)\s+obj\b/);
  return match ? { num: Number(match[1]), gen: Number(match[2]) } : null;
}

function pdfObjectKey(encryption: PdfEncryption, num: number, gen: number): Uint8Array {
  const extra = new Uint8Array(5);
  extra[0] = num & 0xff;
  extra[1] = (num >>> 8) & 0xff;
  extra[2] = (num >>> 16) & 0xff;
  extra[3] = gen & 0xff;
  extra[4] = (gen >>> 8) & 0xff;
  return md5Bytes(concatBytes(encryption.key, extra)).subarray(0, Math.min(encryption.keyLength + 5, 16));
}

function decryptPdfStreamObject(objectText: string, encryption: PdfEncryption | null): string {
  if (!encryption || encryption.streamMethod !== "RC4") return objectText;
  const ref = pdfObjectRefFromText(objectText);
  if (!ref || ref.num === encryption.encryptRef || /\/Type\s*\/XRef\b/.test(objectText)) return objectText;
  if (!encryption.encryptMetadata && /\/Type\s*\/Metadata\b/.test(objectText)) return objectText;
  const streamIndex = objectText.indexOf("stream");
  const endstreamIndex = objectText.lastIndexOf("endstream");
  if (streamIndex === -1 || endstreamIndex === -1 || endstreamIndex <= streamIndex) return objectText;

  let start = streamIndex + "stream".length;
  if (objectText[start] === "\r" && objectText[start + 1] === "\n") start += 2;
  else if (objectText[start] === "\r" || objectText[start] === "\n") start += 1;

  let end = endstreamIndex;
  if (objectText[end - 2] === "\r" && objectText[end - 1] === "\n") end -= 2;
  else if (objectText[end - 1] === "\r" || objectText[end - 1] === "\n") end -= 1;

  const decrypted = rc4Bytes(latin1Bytes(objectText.slice(start, end)), pdfObjectKey(encryption, ref.num, ref.gen));
  return `${objectText.slice(0, start)}${latin1String(decrypted)}${objectText.slice(end)}`;
}

export function parsePdfObjects(raw: string, password = ""): Map<number, string> {
  const objects = new Map<number, string>();
  const re = /(?:^|[\r\n])(\d+)\s+(\d+)\s+obj\b/g;
  let match: RegExpExecArray | null;
  while ((match = re.exec(raw))) {
    const start = match.index + (match[0].match(/^[\r\n]/) ? 1 : 0);
    const nextMatch = /(?:^|[\r\n])\d+\s+\d+\s+obj\b/g;
    nextMatch.lastIndex = re.lastIndex;
    const next = nextMatch.exec(raw);
    const boundary = next?.index ?? raw.search(/\n\s*(?:xref|trailer|startxref)\b/);
    const searchEnd = boundary >= 0 ? boundary : raw.length;
    const relativeEnd = raw.slice(re.lastIndex, searchEnd).lastIndexOf("endobj");
    if (relativeEnd === -1) continue;
    const end = re.lastIndex + relativeEnd;
    objects.set(Number(match[1]), raw.slice(start, end + "endobj".length));
    re.lastIndex = end + "endobj".length;
  }
  const encryption = parsePdfEncryption(raw, objects, password);
  if (encryption) {
    for (const [objectNumber, text] of objects) objects.set(objectNumber, decryptPdfStreamObject(text, encryption));
  }
  for (const text of [...objects.values()]) {
    if (!/\/Type\s*\/ObjStm\b/.test(text)) continue;
    const first = Number(text.match(/\/First\s+(\d+)/)?.[1] ?? Number.NaN);
    const count = Number(text.match(/\/N\s+(\d+)/)?.[1] ?? Number.NaN);
    if (!Number.isFinite(first) || !Number.isFinite(count)) continue;
    const decoded = decodePdfStream(text);
    if (!decoded || decoded.length <= first) continue;
    const header = decoded.slice(0, first);
    const values = parseNumbers(header);
    if (values.length < count * 2) continue;
    const body = decoded.slice(first);
    const entries = Array.from({ length: count }, (_, index) => ({
      objectNumber: values[index * 2],
      offset: values[index * 2 + 1]
    })).sort((a, b) => a.offset - b.offset);
    for (let i = 0; i < entries.length; i += 1) {
      const entry = entries[i];
      const end = entries[i + 1]?.offset ?? body.length;
      objects.set(entry.objectNumber, `${entry.objectNumber} 0 obj\n${body.slice(entry.offset, end).trim()}\nendobj`);
    }
  }
  return objects;
}

export function decodePdfStream(objectText: string | undefined): string {
  if (!objectText) return "";
  const streamIndex = objectText.indexOf("stream");
  const endstreamIndex = objectText.lastIndexOf("endstream");
  if (streamIndex === -1 || endstreamIndex === -1 || endstreamIndex <= streamIndex) return "";

  let start = streamIndex + "stream".length;
  if (objectText[start] === "\r" && objectText[start + 1] === "\n") start += 2;
  else if (objectText[start] === "\r" || objectText[start] === "\n") start += 1;

  let end = endstreamIndex;
  if (objectText[end - 2] === "\r" && objectText[end - 1] === "\n") end -= 2;
  else if (objectText[end - 1] === "\r" || objectText[end - 1] === "\n") end -= 1;

  let bytes: Buffer<ArrayBufferLike> = Buffer.from(objectText.slice(start, end), "latin1");
  const filters = Array.from(objectText.matchAll(/\/(ASCII85Decode|A85|FlateDecode|Fl|ASCIIHexDecode|AHx|RunLengthDecode|RL|LZWDecode|LZW)\b/g), (match) => match[1]);
  for (const filter of filters) {
    if (filter === "ASCII85Decode" || filter === "A85") bytes = ascii85Decode(bytes.toString("latin1"));
    else if (filter === "ASCIIHexDecode" || filter === "AHx") bytes = asciiHexDecode(bytes.toString("latin1"));
    else if (filter === "RunLengthDecode" || filter === "RL") bytes = runLengthDecode(bytes);
    else if (filter === "LZWDecode" || filter === "LZW") bytes = lzwDecode(bytes);
    else if (filter === "FlateDecode" || filter === "Fl") {
      try {
        bytes = inflateSync(bytes);
      } catch {
        return "";
      }
    }
  }
  return bytes.toString("latin1");
}

function stripPdfObject(text: string): string {
  return text.replace(/^\s*\d+\s+\d+\s+obj\s*/, "").replace(/\s*endobj\s*$/, "").trim();
}

function ascii85Decode(value: string): Buffer<ArrayBufferLike> {
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
  return Buffer.from(out);
}

function asciiHexDecode(value: string): Buffer<ArrayBufferLike> {
  let clean = value.replace(/\s+/g, "");
  const eod = clean.indexOf(">");
  if (eod >= 0) clean = clean.slice(0, eod);
  if (clean.length % 2) clean += "0";
  return Buffer.from(clean, "hex");
}

function runLengthDecode(bytes: Buffer<ArrayBufferLike>): Buffer<ArrayBufferLike> {
  const out: number[] = [];
  for (let i = 0; i < bytes.length; i += 1) {
    const length = bytes[i];
    if (length === 128) break;
    if (length <= 127) {
      for (let j = 0; j < length + 1 && i + 1 + j < bytes.length; j += 1) out.push(bytes[i + 1 + j]);
      i += length + 1;
    } else {
      const value = bytes[++i];
      for (let j = 0; j < 257 - length; j += 1) out.push(value);
    }
  }
  return Buffer.from(out);
}

function lzwDecode(bytes: Buffer<ArrayBufferLike>): Buffer<ArrayBufferLike> {
  let bitPos = 0;
  let nbits = 9;
  let table: Array<Buffer<ArrayBufferLike> | null> = [];
  let previous: Buffer<ArrayBufferLike> | null = null;
  const out: Buffer<ArrayBufferLike>[] = [];
  const reset = (): void => {
    table = Array.from({ length: 256 }, (_unused, index) => Buffer.from([index]));
    table[256] = null;
    table[257] = null;
    previous = Buffer.alloc(0);
    nbits = 9;
  };
  const readBits = (bits: number): number | null => {
    if (bitPos + bits > bytes.length * 8) return null;
    let value = 0;
    for (let i = 0; i < bits; i += 1) {
      const absolute = bitPos + i;
      value = (value << 1) | ((bytes[absolute >> 3] >> (7 - (absolute & 7))) & 1);
    }
    bitPos += bits;
    return value;
  };
  reset();
  while (true) {
    const code = readBits(nbits);
    if (code == null || code === 257) break;
    if (code === 256) {
      reset();
      continue;
    }
    let chunk: Buffer<ArrayBufferLike>;
    if (!previous?.length) {
      const entry = table[code];
      if (!entry) break;
      chunk = entry;
    } else if (code < table.length && table[code]) {
      chunk = table[code] as Buffer<ArrayBufferLike>;
      table.push(Buffer.concat([previous, chunk.subarray(0, 1)]));
    } else if (code === table.length) {
      chunk = Buffer.concat([previous, previous.subarray(0, 1)]);
      table.push(chunk);
    } else {
      break;
    }
    const tableLength = table.length;
    if (tableLength === 511) nbits = 10;
    else if (tableLength === 1023) nbits = 11;
    else if (tableLength === 2047) nbits = 12;
    out.push(chunk);
    previous = chunk;
  }
  return Buffer.concat(out);
}

function xObjectEntries(ownerText: string | undefined, objects: Map<number, string>): Array<{ name: string; objectNumber: number }> {
  if (!ownerText) return [];
  const sources: string[] = [ownerText];
  const resourceRef = ownerText.match(/\/Resources\s+(\d+)\s+\d+\s+R/)?.[1];
  const resourceText = resourceRef ? objects.get(Number(resourceRef)) : undefined;
  if (resourceText) sources.push(resourceText);

  const xObjectRefs: string[] = [];
  for (const source of sources) {
    const ref = source.match(/\/XObject\s+(\d+)\s+\d+\s+R/)?.[1];
    if (ref) xObjectRefs.push(ref);
  }
  for (const ref of xObjectRefs) {
    const objectText = objects.get(Number(ref));
    if (objectText) sources.push(stripPdfObject(objectText));
  }

  const entries: Array<{ name: string; objectNumber: number }> = [];
  const addEntries = (body: string): void => {
    const entryRe = /\/([^\s/<>[\]()]+)\s+(\d+)\s+\d+\s+R/g;
    let entry: RegExpExecArray | null;
    while ((entry = entryRe.exec(body))) entries.push({ name: entry[1], objectNumber: Number(entry[2]) });
  };
  for (const source of sources) {
    const direct = source.match(/\/XObject\s*<<([\s\S]*?)>>/);
    if (direct) addEntries(direct[1]);
    else if (!/\/XObject\b/.test(source) && /^\s*<</.test(source)) addEntries(source);
  }
  return entries;
}

function expandFormXObjects(content: string, ownerText: string | undefined, objects: Map<number, string>, depth = 0): string {
  if (!content || depth > 4) return content;
  const forms = new Map<string, string>();
  for (const entry of xObjectEntries(ownerText, objects)) {
    const objectText = objects.get(entry.objectNumber);
    if (objectText && /\/Subtype\s*\/Form\b/.test(objectText)) forms.set(entry.name, objectText);
  }
  if (!forms.size) return content;
  return content.replace(/\/([^\s/<>[\]()]+)\s+Do\b/g, (match, name: string) => {
    const form = forms.get(name);
    if (!form) return match;
    const formContent = expandFormXObjects(decodePdfStream(form), form, objects, depth + 1);
    const matrix = form.match(/\/Matrix\s*\[([^\]]+)\]/)?.[1];
    return matrix ? `q\n${matrix} cm\n${formContent}\nQ` : formContent;
  });
}

export function extractPageContent(pageObjectText: string | undefined, objects: Map<number, string>): string {
  if (!pageObjectText) return "";
  const contentRefs = (source: string | undefined, seen = new Set<number>()): number[] => {
    if (!source) return [];
    const arrayMatch = source.match(/\/Contents\s*\[([\s\S]*?)\]/);
    if (arrayMatch) return Array.from(arrayMatch[1].matchAll(/(\d+)\s+\d+\s+R/g), (match) => Number(match[1]));
    const refMatch = source.match(/\/Contents\s+(\d+)\s+\d+\s+R/);
    if (!refMatch) return [];
    const ref = Number(refMatch[1]);
    if (seen.has(ref)) return [];
    seen.add(ref);
    const objectText = objects.get(ref);
    const stripped = objectText ? stripPdfObject(objectText) : "";
    if (/^\s*\[/.test(stripped)) return Array.from(stripped.matchAll(/(\d+)\s+\d+\s+R/g), (match) => Number(match[1]));
    if (!/\bstream\b/.test(stripped)) return contentRefs(`/Contents ${stripped}`, seen);
    return [ref];
  };
  const content = contentRefs(pageObjectText)
    .map((ref) => decodePdfStream(objects.get(ref)))
    .join("\n");
  return expandFormXObjects(content, pageObjectText, objects);
}

export function parseImageResources(pageObjectText: string | undefined, objects: Map<number, string>, pageContent = ""): ImageResource[] {
  const numberAttr = (objectText: string, name: string): number | undefined => {
    const indirect = objectText.match(new RegExp(`/${name}\\s+(\\d+)\\s+\\d+\\s+R`));
    if (indirect) {
      const resolved = objects.get(Number(indirect[1]))?.match(/\bobj\s+([-+]?(?:\d+\.\d*|\.\d+|\d+))/);
      if (resolved) return Number(resolved[1]);
    }
    const direct = objectText.match(new RegExp(`/${name}\\s+([-+]?(?:\\d+\\.\\d+|\\d+|\\.\\d+))`));
    return direct ? Number(direct[1]) : undefined;
  };
  const resolvedLength = (objectText: string): number | undefined => {
    const direct = objectText.match(/\/Length\s+(\d+)\b(?!\s+\d+\s+R)/)?.[1];
    if (direct) return Number(direct);
    const ref = objectText.match(/\/Length\s+(\d+)\s+\d+\s+R/)?.[1];
    const resolved = ref ? objects.get(Number(ref))?.match(/\bobj\s+(\d+)/)?.[1] : undefined;
    return resolved ? Number(resolved) : undefined;
  };
  const pdfValueRepr = (objectText: string, name: string): string | number | undefined => {
    const ref = objectText.match(new RegExp(`/${name}\\s+(\\d+)\\s+\\d+\\s+R`))?.[1];
    if (ref) return `<PDFObjRef:${ref}>`;
    const array = objectText.match(new RegExp(`/${name}\\s*\\[([^\\]]*)\\]`))?.[1];
    if (array) return `[${parseNumbers(array).map((value) => value.toFixed(1)).join(", ")}]`;
    const number = objectText.match(new RegExp(`/${name}\\s+([-+]?(?:\\d+\\.\\d+|\\d+|\\.\\d+))`))?.[1];
    if (number) return Number(number);
    const named = objectText.match(new RegExp(`/${name}\\s*/([^\\s/<>[\\]()]+)`))?.[1];
    if (named) return `/'${named}'`;
    return undefined;
  };
  const pdfStreamRepr = (objectNumber: number, objectText: string): string => {
    const raw = resolvedLength(objectText) ?? decodePdfStream(objectText).length;
    const attrs: string[] = [];
    const dictText = objectText.slice(0, objectText.search(/\bstream\b/));
    const keys = Array.from(dictText.matchAll(/\/(Domain|Length|N|Alternate|Filter|FunctionType|Range)\b/g), (match) => match[1])
      .filter((key, index, all) => all.indexOf(key) === index);
    for (const key of keys) {
      const value = pdfValueRepr(objectText, key);
      if (value !== undefined) attrs.push(`'${key}': ${typeof value === "string" ? value : String(value)}`);
    }
    return `<PDFStream(${objectNumber}): raw=${raw}, {${attrs.join(", ")}}>`;
  };
  const parseColorSpaceArray = (body: string): unknown[] => {
    const deviceN = body.match(/^\s*\/DeviceN\s*\[([\s\S]*?)\]([\s\S]*)$/);
    if (deviceN) {
      return [`/'DeviceN'`, parseColorSpaceArray(deviceN[1]), ...parseColorSpaceArray(deviceN[2])];
    }
    const values: unknown[] = [];
    const tokenRe = /(\d+)\s+\d+\s+R|\/([^\s/<>[\]()]+)|<([0-9A-Fa-f\s]+)>|[-+]?(?:\d+\.\d*|\.\d+|\d+)/g;
    let token: RegExpExecArray | null;
    while ((token = tokenRe.exec(body))) {
      if (token[1]) {
        const objectNumber = Number(token[1]);
        const referenced = objects.get(objectNumber) ?? "";
        const body = referenced.replace(/^\d+\s+\d+\s+obj\s*/, "").replace(/\s*endobj$/, "").trim();
        const array = body.match(/^\[([\s\S]*)\]$/);
        const processRef = body.match(/\/Process\s+(\d+)\s+\d+\s+R/)?.[1];
        const processText = processRef ? objects.get(Number(processRef)) ?? "" : "";
        const components = processText.match(/\/Components\s*\[([\s\S]*?)\]/)?.[1];
        values.push(
          /\bstream\b/.test(referenced)
            ? pdfStreamRepr(objectNumber, referenced)
            : array
              ? parseColorSpaceArray(array[1])
              : components
                ? { Process: { Components: parseColorSpaceArray(components) } }
                : cleanObject({ value: body }).value
        );
      } else if (token[2]) {
        values.push(`/'${token[2]}'`);
      } else if (token[3]) {
        values.push(token[3].replace(/\s+/g, ""));
      } else {
        values.push(Number(token[0]));
      }
    }
    return values;
  };
  const colorSpaceAttr = (objectText: string): unknown[] | undefined => {
    const directArray = objectText.match(/\/ColorSpace\s*\[([\s\S]*?)\]/);
    if (directArray) return parseColorSpaceArray(directArray[1]);
    const directName = objectText.match(/\/ColorSpace\s*\/([^\s/<>[\]()]+)/)?.[1];
    if (directName) return [`/'${directName}'`];
    const colorspaceRef = objectText.match(/\/ColorSpace\s+(\d+)\s+\d+\s+R/)?.[1];
    if (!colorspaceRef) return undefined;
    const referenced = objects.get(Number(colorspaceRef)) ?? "";
    const referencedArray = referenced.match(/\bobj\s*\[([\s\S]*?)\]\s*endobj/);
    if (referencedArray) return [parseColorSpaceArray(referencedArray[1])];
    const referencedName = referenced.match(/\bobj\s*\/([^\s/<>[\]()]+)/)?.[1] ?? referenced.match(/\/(DeviceRGB|DeviceGray|DeviceCMYK)\b/)?.[1];
    return referencedName ? [`/'${referencedName}'`] : undefined;
  };
  const resourceByName = new Map<string, ImageResource>();
  const out: ImageResource[] = [];
  const imageResource = (name: string, objectText: string): ImageResource => ({
    name,
    width: numberAttr(objectText, "Width"),
    height: numberAttr(objectText, "Height"),
    bits: numberAttr(objectText, "BitsPerComponent"),
    colorspace: colorSpaceAttr(objectText) ?? [null]
  });
  const addResource = (resource: ImageResource): void => {
    if (resourceByName.has(resource.name)) return;
    resourceByName.set(resource.name, resource);
    out.push(resource);
  };
  const contentRefs = (source: string | undefined, seen = new Set<number>()): number[] => {
    if (!source) return [];
    const arrayMatch = source.match(/\/Contents\s*\[([\s\S]*?)\]/);
    if (arrayMatch) return Array.from(arrayMatch[1].matchAll(/(\d+)\s+\d+\s+R/g), (match) => Number(match[1]));
    const refMatch = source.match(/\/Contents\s+(\d+)\s+\d+\s+R/);
    if (!refMatch) return [];
    const ref = Number(refMatch[1]);
    if (seen.has(ref)) return [];
    seen.add(ref);
    const objectText = objects.get(ref);
    const stripped = objectText ? stripPdfObject(objectText) : "";
    if (/^\s*\[/.test(stripped)) return Array.from(stripped.matchAll(/(\d+)\s+\d+\s+R/g), (match) => Number(match[1]));
    if (!/\bstream\b/.test(stripped)) return contentRefs(`/Contents ${stripped}`, seen);
    return [ref];
  };
  const collectDrawnImages = (ownerText: string | undefined, content: string, depth = 0): ImageResource[] => {
    if (!ownerText || depth > 4) return [];
    const entries = new Map(xObjectEntries(ownerText, objects).map((entry) => [entry.name, entry.objectNumber]));
    const images: ImageResource[] = [];
    for (const match of content.matchAll(/\/([^\s/<>[\]()]+)\s+Do\b/g)) {
      const name = match[1];
      const objectNumber = entries.get(name);
      const objectText = objectNumber == null ? undefined : objects.get(objectNumber);
      if (!objectText) {
        images.push({ name });
      } else if (/\/Subtype\s*\/Form\b/.test(objectText)) {
        images.push(...collectDrawnImages(objectText, decodePdfStream(objectText), depth + 1));
      } else if (/\/Subtype\s*\/Image\b/.test(objectText)) {
        images.push(imageResource(name, objectText));
      }
    }
    return images;
  };
  const unexpandedContent = contentRefs(pageObjectText)
    .map((ref) => decodePdfStream(objects.get(ref)))
    .join("\n");
  const drawnImages = collectDrawnImages(pageObjectText, unexpandedContent);
  if (drawnImages.length) return drawnImages;
  const resourceRef = pageObjectText?.match(/\/Resources\s+(\d+)\s+\d+\s+R/)?.[1];
  const resourceObject = resourceRef ? objects.get(Number(resourceRef)) : undefined;
  const xObjectRef = (pageObjectText ?? resourceObject)?.match(/\/XObject\s+(\d+)\s+\d+\s+R/)?.[1];
  const xObjectObject = xObjectRef ? objects.get(Number(xObjectRef)) : undefined;
  const sources = [
    pageObjectText ?? "",
    resourceObject ?? "",
    xObjectObject ? `/XObject ${xObjectObject.replace(/^\d+\s+\d+\s+obj\s*/, "").replace(/\s*endobj$/, "")}` : "",
    ...objects.values()
  ];
  for (const source of sources) {
    const match = source.match(/\/XObject\s*<<([\s\S]*?)>>/);
    if (!match) continue;
    const entryRe = /\/([^\s/<>[\]()]+)\s+(\d+)\s+\d+\s+R/g;
    let entry: RegExpExecArray | null;
    while ((entry = entryRe.exec(match[1]))) {
      const name = entry[1];
      if (out.some((image) => image.name === name)) continue;
      const objectText = objects.get(Number(entry[2])) ?? "";
      if (!/\/Subtype\s*\/Image\b/.test(objectText) && objectText) continue;
      addResource(imageResource(name, objectText));
    }
    if (out.length) break;
  }
  const drawNames = Array.from(pageContent.matchAll(/\/([^\s/<>[\]()]+)\s+Do\b/g), (match) => match[1]);
  return drawNames.length ? drawNames.map((name) => resourceByName.get(name) ?? { name }) : out;
}

export function parseColorSpaceResources(pageObjectText: string | undefined, objects: Map<number, string>): Record<string, string> {
  const out: Record<string, string> = {};
  const collectSources = (ownerText: string | undefined, seen = new Set<number>(), depth = 0): string[] => {
    if (!ownerText || depth > 4) return [];
    const sources = [ownerText];
    const resourceRef = ownerText.match(/\/Resources\s+(\d+)\s+\d+\s+R/)?.[1];
    const resourceText = resourceRef ? objects.get(Number(resourceRef)) : undefined;
    if (resourceText) sources.push(resourceText);
    for (const entry of xObjectEntries(ownerText, objects)) {
      if (seen.has(entry.objectNumber)) continue;
      seen.add(entry.objectNumber);
      const objectText = objects.get(entry.objectNumber);
      if (!objectText || !/\/Subtype\s*\/Form\b/.test(objectText)) continue;
      sources.push(objectText, ...collectSources(objectText, seen, depth + 1));
    }
    return sources;
  };
  const sources = collectSources(pageObjectText);
  for (const source of sources) {
    const match = source.match(/\/ColorSpace\s*<<([\s\S]*?)>>/);
    if (!match) continue;
    const body = match[1];
    const entryRe = /\/([^\s/<>[\]()]+)\s+(?:(\d+)\s+\d+\s+R|\[\s*\/([^\s/<>[\]()]+))/g;
    let entry: RegExpExecArray | null;
    while ((entry = entryRe.exec(body))) {
      const name = entry[1];
      const direct = entry[3];
      const referenced = entry[2] ? objects.get(Number(entry[2])) : undefined;
      const indexedBaseRef = referenced?.match(/\[\s*\/Indexed\s+(\d+)\s+\d+\s+R/)?.[1];
      const indexedBase = indexedBaseRef ? objects.get(Number(indexedBaseRef)) : undefined;
      out[name] = colorSpaceName(
        direct ??
          indexedBase?.match(/\/Separation\b/)?.[0].slice(1) ??
          referenced?.match(/\/Separation\b/)?.[0].slice(1) ??
          referenced?.match(/\[\s*\/([^\s/<>[\]()]+)/)?.[1] ??
          referenced?.match(/\/(DeviceRGB|DeviceGray|DeviceCMYK|ICCBased)\b/)?.[1] ??
          name
      );
    }
  }
  return out;
}

export function parseColorOps(content: string, colorSpaces: Record<string, string> = {}): ColorOp[] {
  const ops: ColorOp[] = [];
  const operands: Array<number | { name: string }> = [];
  let fillColorSpace = "DeviceGray";
  let strokeColorSpace = "DeviceGray";
  let index = 0;

  const isWhite = (char: string) => /\s/.test(char);
  const isDelimiter = (char: string) => "()<>[]{}/%".includes(char);
  const skipString = (): void => {
    let depth = 1;
    index += 1;
    while (index < content.length && depth > 0) {
      const char = content[index++];
      if (char === "\\") {
        index += 1;
      } else if (char === "(") {
        depth += 1;
      } else if (char === ")") {
        depth -= 1;
      }
    }
  };
  const skipHexOrDict = (): boolean => {
    if (content[index + 1] === "<") {
      index += 2;
      return false;
    }
    index += 1;
    while (index < content.length && content[index] !== ">") index += 1;
    index += 1;
    return true;
  };
  const skipArray = (): void => {
    let depth = 1;
    index += 1;
    while (index < content.length && depth > 0) {
      const char = content[index];
      if (char === "(") skipString();
      else if (char === "<") skipHexOrDict();
      else {
        if (char === "[") depth += 1;
        else if (char === "]") depth -= 1;
        index += 1;
      }
    }
  };
  const readToken = (): string => {
    const start = index;
    while (index < content.length && !isWhite(content[index]) && !isDelimiter(content[index])) index += 1;
    return content.slice(start, index);
  };
  const resetOperands = (): void => {
    operands.length = 0;
  };

  while (index < content.length) {
    const char = content[index];
    if (isWhite(char)) {
      index += 1;
      continue;
    }
    if (char === "%") {
      while (index < content.length && content[index] !== "\n" && content[index] !== "\r") index += 1;
      continue;
    }
    if (char === "(") {
      skipString();
      continue;
    }
    if (char === "[") {
      skipArray();
      continue;
    }
    if (char === "<") {
      const skippedHex = skipHexOrDict();
      if (skippedHex) continue;
      resetOperands();
      continue;
    }
    if (char === ">") {
      index += content[index + 1] === ">" ? 2 : 1;
      resetOperands();
      continue;
    }
    if (char === "/") {
      index += 1;
      operands.push({ name: readToken() });
      continue;
    }

    const token = readToken();
    if (!token) {
      index += 1;
      continue;
    }
    if (/^[-+]?(?:\d+\.\d*|\.\d+|\d+)$/.test(token)) {
      operands.push(Number(token));
      continue;
    }

    const op = token;
    if (op === "cs" || op === "CS") {
      const name = [...operands].reverse().find((operand): operand is { name: string } => typeof operand === "object")?.name ?? "";
      const resolved = colorSpaceName(colorSpaces[name] ?? name);
      if (op === "cs") fillColorSpace = resolved;
      else strokeColorSpace = resolved;
      resetOperands();
      continue;
    }
    const components = operands.filter((operand): operand is number => typeof operand === "number");
    const isStroke = op === op.toUpperCase();
    let colorSpace = isStroke ? strokeColorSpace : fillColorSpace;
    if (op === "g" || op === "G") colorSpace = "DeviceGray";
    else if (op === "rg" || op === "RG") colorSpace = "DeviceRGB";
    else if (op === "k" || op === "K") colorSpace = "DeviceCMYK";
    if (["g", "G", "rg", "RG", "k", "K", "sc", "SC", "scn", "SCN"].includes(op)) {
      const pattern = op.endsWith("cn") ? [...operands].reverse().find((operand): operand is { name: string } => typeof operand === "object")?.name : undefined;
      ops.push({
        target: isStroke ? "stroke" : "fill",
        components,
        colorSpace,
        pattern
      });
      if (isStroke) strokeColorSpace = colorSpace;
      else fillColorSpace = colorSpace;
    }
    resetOperands();
  }
  return ops;
}

export function parseTextMatrixOps(content: string): Matrix[] {
  const matrices: Matrix[] = [];
  const operands: number[] = [];
  let index = 0;

  const isWhite = (char: string): boolean => /\s/.test(char);
  const isDelimiter = (char: string): boolean => "()<>[]{}/%".includes(char);
  const skipString = (): void => {
    let depth = 1;
    index += 1;
    while (index < content.length && depth > 0) {
      const char = content[index++];
      if (char === "\\") index += 1;
      else if (char === "(") depth += 1;
      else if (char === ")") depth -= 1;
    }
  };
  const skipHexOrDict = (): void => {
    if (content[index + 1] === "<") {
      index += 2;
      return;
    }
    index += 1;
    while (index < content.length && content[index] !== ">") index += 1;
    index += 1;
  };
  const skipArray = (): void => {
    let depth = 1;
    index += 1;
    while (index < content.length && depth > 0) {
      const char = content[index];
      if (char === "(") skipString();
      else if (char === "<") skipHexOrDict();
      else {
        if (char === "[") depth += 1;
        else if (char === "]") depth -= 1;
        index += 1;
      }
    }
  };
  const readToken = (): string => {
    const start = index;
    while (index < content.length && !isWhite(content[index]) && !isDelimiter(content[index])) index += 1;
    return content.slice(start, index);
  };

  while (index < content.length) {
    const char = content[index];
    if (isWhite(char)) {
      index += 1;
      continue;
    }
    if (char === "%") {
      while (index < content.length && content[index] !== "\n" && content[index] !== "\r") index += 1;
      continue;
    }
    if (char === "(") {
      skipString();
      continue;
    }
    if (char === "[") {
      skipArray();
      operands.length = 0;
      continue;
    }
    if (char === "<") {
      skipHexOrDict();
      operands.length = 0;
      continue;
    }
    if (char === ">") {
      index += content[index + 1] === ">" ? 2 : 1;
      operands.length = 0;
      continue;
    }
    if (char === "/") {
      index += 1;
      readToken();
      operands.length = 0;
      continue;
    }

    const token = readToken();
    if (!token) {
      index += 1;
      continue;
    }
    if (/^[-+]?(?:\d+\.\d*|\.\d+|\d+)$/.test(token)) {
      operands.push(Number(token));
      continue;
    }
    if (token === "Tm" && operands.length >= 6) matrices.push(operands.slice(-6) as Matrix);
    operands.length = 0;
  }

  return matrices;
}

export function parseTextMoveOps(content: string): { move: Point[]; leadingMove: Point[] } {
  const move: Point[] = [];
  const leadingMove: Point[] = [];
  const operands: number[] = [];
  let index = 0;

  const isWhite = (char: string): boolean => /\s/.test(char);
  const isDelimiter = (char: string): boolean => "()<>[]{}/%".includes(char);
  const skipString = (): void => {
    let depth = 1;
    index += 1;
    while (index < content.length && depth > 0) {
      const char = content[index++];
      if (char === "\\") index += 1;
      else if (char === "(") depth += 1;
      else if (char === ")") depth -= 1;
    }
  };
  const skipHexOrDict = (): void => {
    if (content[index + 1] === "<") {
      index += 2;
      return;
    }
    index += 1;
    while (index < content.length && content[index] !== ">") index += 1;
    index += 1;
  };
  const skipArray = (): void => {
    let depth = 1;
    index += 1;
    while (index < content.length && depth > 0) {
      const char = content[index];
      if (char === "(") skipString();
      else if (char === "<") skipHexOrDict();
      else {
        if (char === "[") depth += 1;
        else if (char === "]") depth -= 1;
        index += 1;
      }
    }
  };
  const readToken = (): string => {
    const start = index;
    while (index < content.length && !isWhite(content[index]) && !isDelimiter(content[index])) index += 1;
    return content.slice(start, index);
  };

  while (index < content.length) {
    const char = content[index];
    if (isWhite(char)) {
      index += 1;
      continue;
    }
    if (char === "%") {
      while (index < content.length && content[index] !== "\n" && content[index] !== "\r") index += 1;
      continue;
    }
    if (char === "(") {
      skipString();
      continue;
    }
    if (char === "[") {
      skipArray();
      operands.length = 0;
      continue;
    }
    if (char === "<") {
      skipHexOrDict();
      operands.length = 0;
      continue;
    }
    if (char === ">") {
      index += content[index + 1] === ">" ? 2 : 1;
      operands.length = 0;
      continue;
    }
    if (char === "/") {
      index += 1;
      readToken();
      operands.length = 0;
      continue;
    }

    const token = readToken();
    if (!token) {
      index += 1;
      continue;
    }
    if (/^[-+]?(?:\d+\.\d*|\.\d+|\d+)$/.test(token)) {
      operands.push(Number(token));
      continue;
    }
    if ((token === "Td" || token === "TD") && operands.length >= 2) {
      const point = operands.slice(-2) as Point;
      if (token === "TD") leadingMove.push(point);
      else move.push(point);
    }
    operands.length = 0;
  }

  return { move, leadingMove };
}

export function parseTransformOps(content: string): Matrix[] {
  const matrices: Matrix[] = [];
  const operands: number[] = [];
  let index = 0;

  const isWhite = (char: string): boolean => /\s/.test(char);
  const isDelimiter = (char: string): boolean => "()<>[]{}/%".includes(char);
  const skipString = (): void => {
    let depth = 1;
    index += 1;
    while (index < content.length && depth > 0) {
      const char = content[index++];
      if (char === "\\") index += 1;
      else if (char === "(") depth += 1;
      else if (char === ")") depth -= 1;
    }
  };
  const skipHexOrDict = (): void => {
    if (content[index + 1] === "<") {
      index += 2;
      return;
    }
    index += 1;
    while (index < content.length && content[index] !== ">") index += 1;
    index += 1;
  };
  const skipArray = (): void => {
    let depth = 1;
    index += 1;
    while (index < content.length && depth > 0) {
      const char = content[index];
      if (char === "(") skipString();
      else if (char === "<") skipHexOrDict();
      else {
        if (char === "[") depth += 1;
        else if (char === "]") depth -= 1;
        index += 1;
      }
    }
  };
  const readToken = (): string => {
    const start = index;
    while (index < content.length && !isWhite(content[index]) && !isDelimiter(content[index])) index += 1;
    return content.slice(start, index);
  };

  while (index < content.length) {
    const char = content[index];
    if (isWhite(char)) {
      index += 1;
      continue;
    }
    if (char === "%") {
      while (index < content.length && content[index] !== "\n" && content[index] !== "\r") index += 1;
      continue;
    }
    if (char === "(") {
      skipString();
      continue;
    }
    if (char === "[") {
      skipArray();
      operands.length = 0;
      continue;
    }
    if (char === "<") {
      skipHexOrDict();
      operands.length = 0;
      continue;
    }
    if (char === ">") {
      index += content[index + 1] === ">" ? 2 : 1;
      operands.length = 0;
      continue;
    }
    if (char === "/") {
      index += 1;
      readToken();
      operands.length = 0;
      continue;
    }

    const token = readToken();
    if (!token) {
      index += 1;
      continue;
    }
    if (/^[-+]?(?:\d+\.\d*|\.\d+|\d+)$/.test(token)) {
      operands.push(Number(token));
      continue;
    }
    if (token === "cm" && operands.length >= 6) matrices.push(operands.slice(-6) as Matrix);
    operands.length = 0;
  }

  return matrices;
}

export function parsePathOps(content: string): number[][] {
  const paths: number[][] = [];
  let current: number[] = [];
  const operands: number[] = [];
  let point: Point = [0, 0];
  let index = 0;

  const isWhite = (char: string): boolean => /\s/.test(char);
  const isDelimiter = (char: string): boolean => "()<>[]{}/%".includes(char);
  const skipString = (): void => {
    let depth = 1;
    index += 1;
    while (index < content.length && depth > 0) {
      const char = content[index++];
      if (char === "\\") index += 1;
      else if (char === "(") depth += 1;
      else if (char === ")") depth -= 1;
    }
  };
  const skipHexOrDict = (): void => {
    if (content[index + 1] === "<") {
      index += 2;
      return;
    }
    index += 1;
    while (index < content.length && content[index] !== ">") index += 1;
    index += 1;
  };
  const skipArray = (): void => {
    let depth = 1;
    index += 1;
    while (index < content.length && depth > 0) {
      const char = content[index];
      if (char === "(") skipString();
      else if (char === "<") skipHexOrDict();
      else {
        if (char === "[") depth += 1;
        else if (char === "]") depth -= 1;
        index += 1;
      }
    }
  };
  const readToken = (): string => {
    const start = index;
    while (index < content.length && !isWhite(content[index]) && !isDelimiter(content[index])) index += 1;
    return content.slice(start, index);
  };
  const flush = (close = false, includeEmpty = false): void => {
    if (close) current.push(4);
    if (current.length || includeEmpty) paths.push(current);
    current = [];
  };

  while (index < content.length) {
    const char = content[index];
    if (isWhite(char)) {
      index += 1;
      continue;
    }
    if (char === "%") {
      while (index < content.length && content[index] !== "\n" && content[index] !== "\r") index += 1;
      continue;
    }
    if (char === "(") {
      skipString();
      continue;
    }
    if (char === "[") {
      skipArray();
      operands.length = 0;
      continue;
    }
    if (char === "<") {
      skipHexOrDict();
      operands.length = 0;
      continue;
    }
    if (char === ">") {
      index += content[index + 1] === ">" ? 2 : 1;
      operands.length = 0;
      continue;
    }
    if (char === "/") {
      index += 1;
      readToken();
      operands.length = 0;
      continue;
    }

    const token = readToken();
    if (!token) {
      index += 1;
      continue;
    }
    if (/^[-+]?(?:\d+\.\d*|\.\d+|\d+)$/.test(token)) {
      operands.push(Number(token));
      continue;
    }

    if (token === "m" && operands.length >= 2) {
      const [x, y] = operands.slice(-2);
      current.push(0, x, y);
      point = [x, y];
    } else if (token === "l" && operands.length >= 2) {
      const [x, y] = operands.slice(-2);
      current.push(1, x, y);
      point = [x, y];
    } else if (token === "c" && operands.length >= 6) {
      const values = operands.slice(-6);
      current.push(2, ...values);
      point = [values[4], values[5]];
    } else if (token === "v" && operands.length >= 4) {
      const values = operands.slice(-4);
      current.push(2, point[0], point[1], ...values);
      point = [values[2], values[3]];
    } else if (token === "y" && operands.length >= 4) {
      const values = operands.slice(-4);
      current.push(3, ...values);
      point = [values[2], values[3]];
    } else if (token === "h") {
      current.push(4);
    } else if (token === "re" && operands.length >= 4) {
      const [x, y, width, height] = operands.slice(-4);
      current.push(5, x, y, width, height);
      point = [x, y];
    } else if (["S", "f", "F", "f*", "B", "B*", "n"].includes(token)) {
      flush(false, token === "n");
    } else if (["s", "b", "b*"].includes(token)) {
      flush(true);
    }
    operands.length = 0;
  }

  return paths;
}

export function parseNumbers(value: string): number[] {
  return Array.from(value.matchAll(/[-+]?(?:\d+\.\d*|\.\d+|\d+)/g)).map((m) => Number(m[0]));
}

export function decodePdfName(value: string): string {
  return value.replace(/#([0-9A-Fa-f]{2})/g, (_match, hex: string) => String.fromCharCode(Number.parseInt(hex, 16)));
}

export function decodePdfNameUtf8(value: string): string {
  const decoded = decodePdfName(value);
  if (!/[^\x00-\x7f]/.test(decoded)) return decoded;
  try {
    const bytes = Uint8Array.from([...decoded].map((char) => char.charCodeAt(0) & 0xff));
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return decoded;
  }
}

function extractRawBox(objectText: string | undefined, name: string): number[] | null {
  if (!objectText) return null;
  const match = objectText.match(new RegExp(`/${name}\\s*\\[([^\\]]+)\\]`));
  if (!match) return null;
  const numbers = parseNumbers(match[1]);
  return numbers.length >= 4 ? numbers.slice(0, 4) : null;
}

function boxToPdfplumber(box: readonly number[], mediaHeight: number): BBox {
  return cleanBBox([box[0], mediaHeight - box[3], box[2], mediaHeight - box[1]]);
}

function normalizeRawBox(box: readonly number[]): BBox {
  return [Math.min(box[0], box[2]), Math.min(box[1], box[3]), Math.max(box[0], box[2]), Math.max(box[1], box[3])];
}

export function resolvePageBoxes(pdfPage: any, pageObjectText: string | undefined): PageBoxes {
  const rawMediaOriginal = extractRawBox(pageObjectText, "MediaBox") ?? [...pdfPage.view];
  const rawMedia = normalizeRawBox(rawMediaOriginal);
  const rotate = ((pdfPage.rotate ?? 0) % 360 + 360) % 360;
  const contentYOffset = rawMediaOriginal[3] < rawMediaOriginal[1] ? -(rawMedia[3] - rawMedia[1]) : 0;
  const rawCrop = normalizeRawBox(extractRawBox(pageObjectText, "CropBox") ?? rawMedia);
  if (rotate === 90 || rotate === 270) {
    const rawWidth = rawMedia[2] - rawMedia[0];
    const rawHeight = rawMedia[3] - rawMedia[1];
    const width = rawHeight;
    const height = rawWidth;
    const box = cleanBBox([0, 0, width, height]);
    const rotatedBox = (rawBox: BBox): BBox => {
      if (rotate === 90) {
        return cleanBBox([rawBox[1] - rawMedia[1], rawBox[0] - rawMedia[0], rawBox[3] - rawMedia[1], rawBox[2] - rawMedia[0]]);
      }
      return cleanBBox([rawBox[1] - rawMedia[1], rawMedia[2] - rawBox[2], rawBox[3] - rawMedia[1], rawMedia[2] - rawBox[0]]);
    };
    return { bbox: box, mediabox: box, cropbox: rotatedBox(rawCrop), width: cleanNumber(width), height: cleanNumber(height), rotate, contentYOffset };
  }
  const mediaHeight = rawMedia[3] - rawMedia[1];
  const mediabox = boxToPdfplumber(rawMedia, mediaHeight);
  const cropbox = boxToPdfplumber(rawCrop, mediaHeight);
  const art = extractRawBox(pageObjectText, "ArtBox");
  const bleed = extractRawBox(pageObjectText, "BleedBox");
  const trim = extractRawBox(pageObjectText, "TrimBox");
  return {
    bbox: mediabox,
    mediabox,
    cropbox,
    artbox: art ? boxToPdfplumber(normalizeRawBox(art), mediaHeight) : undefined,
    bleedbox: bleed ? boxToPdfplumber(normalizeRawBox(bleed), mediaHeight) : undefined,
    trimbox: trim ? boxToPdfplumber(normalizeRawBox(trim), mediaHeight) : undefined,
    width: cleanNumber(mediabox[2] - mediabox[0]),
    height: cleanNumber(mediabox[3] - mediabox[1]),
    rotate,
    contentYOffset
  };
}

export function parseFontRecords(objects: Map<number, string>): FontRecord[] {
  const fonts: FontRecord[] = [];
  for (const [objectNumber, text] of objects) {
    if (!/\/Type\s*\/Font\b/.test(text) || !/\/BaseFont\s*(?:\/|\d+\s+\d+\s+R)/.test(text)) continue;
    const baseFontRef = text.match(/\/BaseFont\s+(\d+)\s+\d+\s+R/)?.[1];
    const baseFontRaw =
      text.match(/\/BaseFont\s*\/([^\s/>]+)/)?.[1] ??
      (baseFontRef ? objects.get(Number(baseFontRef))?.match(/\bobj\s*\/([^\s/>]+)/)?.[1] : undefined);
    if (!baseFontRaw) continue;
    const baseFont = decodePdfName(baseFontRaw);
    const subtype = text.match(/\/Subtype\s*\/([^\s/>]+)/)?.[1];
    const firstChar = Number(text.match(/\/FirstChar\s+(\d+)/)?.[1] ?? 0);
    const encodingRef = text.match(/\/Encoding\s+(\d+)\s+\d+\s+R/)?.[1];
    const encodingText = encodingRef ? objects.get(Number(encodingRef)) : text;
    const encodingDifferences = parseEncodingDifferences(encodingText);
    const widthsRef = text.match(/\/Widths\s+(\d+)\s+\d+\s+R/)?.[1];
    const widthsText = widthsRef ? objects.get(Number(widthsRef)) : text;
    const widthsMatch = widthsText?.match(/\[([\s\S]*?)\]/);
    const widths = widthsMatch ? parseNumbers(widthsMatch[1]) : [];
    const descendantRef = text.match(/\/DescendantFonts\s*\[\s*(\d+)\s+\d+\s+R/)?.[1];
    const descendantContainer = descendantRef ? objects.get(Number(descendantRef)) : undefined;
    const nestedDescendantRef = descendantContainer?.match(/^\s*\[\s*(\d+)\s+\d+\s+R/)?.[1];
    const descendant = nestedDescendantRef ? objects.get(Number(nestedDescendantRef)) : descendantContainer;
    const descriptorRef = text.match(/\/FontDescriptor\s+(\d+)\s+\d+\s+R/)?.[1] ?? descendant?.match(/\/FontDescriptor\s+(\d+)\s+\d+\s+R/)?.[1];
    const descriptor = descriptorRef ? objects.get(Number(descriptorRef)) : undefined;
    const ascent = descriptor?.match(/\/Ascent\s+([-+]?(?:\d+\.\d*|\.\d+|\d+))/)?.[1];
    const descent = descriptor?.match(/\/Descent\s+([-+]?(?:\d+\.\d*|\.\d+|\d+))/)?.[1];
    const flags = descriptor?.match(/\/Flags\s+(\d+)/)?.[1];
    const charSet = descriptor?.match(/\/CharSet\s*\(([^)]*)\)/)?.[1].match(/\/([^/\s()]+)/g)?.map((name) => name.slice(1));
    fonts.push({
      objectNumber,
      baseFont,
      subtype: subtype ? decodePdfName(subtype) : undefined,
      hasToUnicode: /\/ToUnicode\s+\d+\s+\d+\s+R\b/.test(text),
      symbolic: flags == null ? undefined : (Number(flags) & 4) !== 0,
      charSet,
      encodingDifferences,
      firstChar,
      widths,
      ascent: ascent == null ? undefined : Number(ascent) / FONT_UNITS_PER_EM,
      descent: descent == null ? undefined : -Math.abs(Number(descent) / FONT_UNITS_PER_EM),
      vertical: /\/Encoding\s*\/(?:Identity-V|DLIdent-V)\b/.test(text)
    });
  }
  return fonts;
}

export function parsePageFontObjectNumbers(pageObjectText: string | undefined): number[] {
  const fontDict = pageObjectText?.match(/\/Font\s*<<([\s\S]*?)>>/)?.[1];
  if (!fontDict) return [];
  return Array.from(fontDict.matchAll(/\/[^\s/<>[\]()]+\s+(\d+)\s+\d+\s+R/g), (match) => Number(match[1]));
}

function parseEncodingDifferences(encodingText: string | undefined): Record<number, string> | undefined {
  const body = encodingText?.match(/\/Differences\s*\[([\s\S]*?)\]/)?.[1];
  if (!body) return undefined;
  const differences: Record<number, string> = {};
  let code: number | null = null;
  for (const match of body.matchAll(/\/([^\s/[\]()<>]+)|(-?\d+)/g)) {
    if (match[2] != null) {
      code = Number(match[2]);
      continue;
    }
    if (code == null) continue;
    differences[code] = decodePdfName(match[1]);
    code += 1;
  }
  return differences;
}

export function parseInfoMetadata(raw: string, objects: Map<number, string>): Record<string, unknown> {
  const infoRefs = Array.from(raw.matchAll(/\/Info\s+(\d+)\s+\d+\s+R/g));
  const infoRef = infoRefs.at(-1)?.[1];
  const text = infoRef ? objects.get(Number(infoRef)) : undefined;
  if (!text) return {};
  const metadata: Record<string, unknown> = {};
  for (const key of METADATA_KEYS) {
    const indirectRef = text.match(new RegExp(`/${key}\\s+(\\d+)\\s+\\d+\\s+R\\b`))?.[1];
    const indirectText = indirectRef ? objects.get(Number(indirectRef)) : undefined;
    const indirectString = indirectText ? readFirstPdfString(indirectText) : null;
    if (indirectString != null) {
      metadata[key] = indirectString;
      continue;
    }
    const string = readPdfLiteralString(text, key);
    const hexString = readPdfHexString(text, key);
    if (string != null) metadata[key] = string;
    else if (hexString != null) metadata[key] = hexString;
    else {
      const number = text.match(new RegExp(`/${key}\\s+([-+]?(?:\\d+\\.\\d+|\\d+|\\.\\d+))\\b`))?.[1];
      if (number != null) metadata[key] = Number(number);
    }
  }
  return metadata;
}

function decodePdfStringBytes(bytes: number[]): string {
  if (bytes[0] === 0xff && bytes[1] === 0xfe) {
    let out = "";
    for (let i = 2; i < bytes.length - 1; i += 2) out += String.fromCharCode(bytes[i] | (bytes[i + 1] << 8));
    return out;
  }
  return decodePdfStringLikePdfminer(bytes);
}

function decodePdfLiteralValue(value: string): string {
  return decodePdfStringBytes([...value].map((char) => char.charCodeAt(0) & 0xff));
}

function readPdfLiteralString(text: string, key: string): string | null {
  const start = text.search(new RegExp(`/${key}\\s*\\(`));
  if (start < 0) return null;
  return readPdfLiteralStringAt(text, text.indexOf("(", start));
}

function readPdfLiteralStringAt(text: string, start: number): string | null {
  let i = start;
  if (i < 0) return null;
  i += 1;
  let depth = 1;
  let out = "";
  for (; i < text.length; i += 1) {
    const char = text[i];
    if (char === "\\") {
      const next = text[++i];
      if (next == null) break;
      if (/[0-7]/.test(next)) {
        let octal = next;
        for (let count = 0; count < 2 && /[0-7]/.test(text[i + 1] ?? ""); count += 1) octal += text[++i];
        out += String.fromCharCode(Number.parseInt(octal, 8));
      } else {
        out += ({ n: "\n", r: "\r", t: "\t", b: "\b", f: "\f" } as Record<string, string>)[next] ?? next;
      }
    } else if (char === "(") {
      depth += 1;
      out += char;
    } else if (char === ")") {
      depth -= 1;
      if (depth === 0) return decodePdfLiteralValue(out);
      out += char;
    } else {
      out += char;
    }
  }
  return null;
}

function readPdfHexString(text: string, key: string): string | null {
  const match = text.match(new RegExp(`/${key}\\s*<([0-9A-Fa-f\\s]*)>`));
  return match ? decodePdfHexValue(match[1]) : null;
}

function decodePdfHexValue(value: string): string {
  const hex = value.replace(/\s+/g, "");
  const bytes: number[] = [];
  for (let i = 0; i < hex.length - 1; i += 2) bytes.push(Number.parseInt(hex.slice(i, i + 2), 16));
  return decodePdfStringBytes(bytes);
}

function readFirstPdfString(text: string): string | null {
  const literal = readPdfLiteralStringAt(text, text.indexOf("("));
  if (literal != null) return literal;
  const hex = text.match(/<([0-9A-Fa-f\s]+)>/)?.[1];
  return hex == null ? null : decodePdfHexValue(hex);
}

function stripSubsetPrefix(name: string): string {
  return name.replace(/^[A-Z]{6}\+/, "");
}

function standardFontMetrics(name: string | undefined): typeof STANDARD_FONT_METRICS[string] | undefined {
  if (!name) return undefined;
  return STANDARD_FONT_METRICS[name];
}

function canonicalFontName(name: string | undefined): string | undefined {
  if (!name) return undefined;
  const stripped = stripSubsetPrefix(name);
  return standardFontMetrics(stripped)?.fontName ?? stripped;
}

function pdfminerCjkFontName(name: string | undefined): string | undefined {
  if (!name || !/[^\x20-\x7e]/.test(name)) return undefined;
  if (/^[A-Z]{6}\+/.test(name)) return undefined;
  try {
    const bytes = Uint8Array.from([...name].map((char) => char.charCodeAt(0) & 0xff));
    const decoded = new TextDecoder("gb18030", { fatal: false }).decode(bytes);
    if (decoded === "宋体") return "SimSun,Regular";
  } catch {
    // Fall back to pdfminer's byte-string representation below.
  }
  return pythonBytesName(name).slice(2, -1);
}

function fontFamilyName(name: string | undefined): string | undefined {
  return canonicalFontName(name)?.replace(/(?:[-,](?:Bold|Italic|Oblique|Roman))*$/i, "");
}

function fontBBoxMetric(fontObj: any, index: 1 | 3): number | undefined {
  return Array.isArray(fontObj?.bbox) && typeof fontObj.bbox[index] === "number" ? Number(fontObj.bbox[index]) / FONT_UNITS_PER_EM : undefined;
}

function widthComparison(record: FontRecord, fontObj: any): { averageDelta: number; matchedWidths: number } | null {
  if (!fontObj?.widths) return null;
  let totalDelta = 0;
  let matchedWidths = 0;
  for (let code = 0; code < 256; code += 1) {
    const raw = record.widths[code - record.firstChar];
    const actual = fontObj.widths[code];
    if (raw != null && actual != null && (raw !== 0 || actual !== 0)) {
      totalDelta += Math.abs(raw - actual);
      matchedWidths += 1;
    }
  }
  return matchedWidths ? { averageDelta: totalDelta / matchedWidths, matchedWidths } : null;
}

function chooseFontRecord(fontRecords: FontRecord[], used: Set<number>, fontObjName: string | undefined, fontObj: any): FontRecord | undefined {
  const matchingName = (record: FontRecord): boolean => {
    const recordStandard = standardFontMetrics(record.baseFont);
    const objectStandard = standardFontMetrics(fontObjName);
    return record.baseFont === fontObjName || (recordStandard != null && objectStandard != null && recordStandard.fontName === objectStandard.fontName);
  };
  const exactMatches = fontRecords.filter(matchingName);
  exactMatches.sort(
    (a, b) =>
      Number(Boolean(b.hasToUnicode)) - Number(Boolean(a.hasToUnicode)) ||
      Number(Boolean(b.ascent != null || b.descent != null)) - Number(Boolean(a.ascent != null || a.descent != null)) ||
      Number(b.subtype === "Type0") - Number(a.subtype === "Type0")
  );
  const exact = exactMatches[0];
  if (exact) {
    const parentType0 = fontRecords.find(
      (record) =>
        record.subtype === "Type0" &&
        (record.baseFont === exact.baseFont || record.baseFont.startsWith(`${exact.baseFont}-`)) &&
        (record.hasToUnicode || !exact.hasToUnicode)
    );
    return parentType0
      ? { ...parentType0, widths: exact.widths, ascent: exact.ascent, descent: exact.descent, symbolic: exact.symbolic }
      : exact;
  }

  const available = fontRecords.filter((record) => !used.has(record.objectNumber));
  const scored = available.flatMap((record, index) => {
    const widths = widthComparison(record, fontObj);
    const recordFamily = fontFamilyName(record.baseFont);
    const objectFamily = fontFamilyName(fontObjName);
    const familyRank = !recordFamily || !objectFamily ? 1 : recordFamily === objectFamily ? 0 : 2;
    return widths ? [{ record, familyRank, averageDelta: widths.averageDelta, matchedWidths: widths.matchedWidths, index }] : [];
  });
  scored.sort((a, b) => a.familyRank - b.familyRank || a.averageDelta - b.averageDelta || b.matchedWidths - a.matchedWidths || a.index - b.index);
  return scored[0]?.record;
}

function resolveFontName(best: FontRecord | undefined, fontObjName: string | undefined, fallbackId: string): string {
  if (best?.subtype === "Type3" || fontObjName === "Type3") return "unknown";
  const standard = standardFontMetrics(best?.baseFont) ?? standardFontMetrics(fontObjName);
  const rawName = fontObjName ?? best?.baseFont;
  const cjkName = pdfminerCjkFontName(rawName);
  if (cjkName) return cjkName;
  return pythonBytesName(standard?.fontName ?? rawName ?? fallbackId);
}

function resolveFontMetrics(best: FontRecord | undefined, fontObj: any, style: Record<string, any>, fontname: string): Pick<MappedFont, "ascent" | "descent"> {
  const standard = standardFontMetrics(best?.baseFont ? stripSubsetPrefix(best.baseFont) : undefined) ?? standardFontMetrics(stripSubsetPrefix(fontname));
  const preferEmbeddedMetrics = Boolean(best?.subtype === "Type0" && !best.hasToUnicode && /TimesNewRoman/i.test(best.baseFont));
  const ascent = firstFinite(
    DEFAULT_FONT_ASCENT,
    ...(preferEmbeddedMetrics ? [best?.ascent, standard?.ascent] : [standard?.ascent, best?.ascent]),
    fontObj?.ascent,
    style.ascent,
    fontBBoxMetric(fontObj, 3)
  );
  const descent = firstFinite(
    DEFAULT_FONT_DESCENT,
    ...(preferEmbeddedMetrics ? [best?.descent, standard?.descent] : [standard?.descent, best?.descent]),
    fontObj?.descent,
    style.descent,
    fontBBoxMetric(fontObj, 1)
  );
  return { ascent: cleanNumber(ascent), descent: cleanNumber(descent > 0 ? -descent : descent) };
}

function shouldUseCidFallback(record: FontRecord | undefined): boolean {
  if (!record?.symbolic || record.hasToUnicode) return false;
  if (record.subtype !== "Type1" && record.subtype !== "TrueType" && record.subtype !== "CIDFontType2") return false;
  if (standardFontMetrics(stripSubsetPrefix(record.baseFont)) != null) return false;
  if (!record.charSet?.length) return false;
  const cidOnlyGlyphNames = new Set(["check", "checkmark", "summationtext"]);
  return record.charSet.every((name) => cidOnlyGlyphNames.has(name));
}

function mapFonts(pdfPage: any, styles: Record<string, any>, fontRecords: FontRecord[], fontIds: string[]): Map<string, MappedFont> {
  const mapped = new Map<string, MappedFont>();
  const used = new Set<number>();
  for (const id of fontIds) {
    let fontObj: any;
    try {
      fontObj = pdfPage.commonObjs.get(id);
    } catch {
      fontObj = null;
    }
    const fontObjName = typeof fontObj?.name === "string" ? fontObj.name : undefined;
    const best = chooseFontRecord(fontRecords, used, fontObjName, fontObj);
    if (best) used.add(best.objectNumber);
    const style = styles[id] ?? {};
    const fontname = resolveFontName(best, fontObjName, id);
    const metrics = resolveFontMetrics(best, fontObj, style, fontname);
    const scopedRecords = fontRecords.filter((record) => record.pageScoped);
    mapped.set(id, {
      fontname,
      ascent: metrics.ascent,
      descent: metrics.descent,
      fontMatrix0: Number(fontObj?.fontMatrix?.[0] ?? 0.001),
      vertical: Boolean(style.vertical ?? fontObj?.vertical ?? best?.vertical),
      cidFallback:
        shouldUseCidFallback(best) ||
        Boolean(
          fontObj?.composite &&
            !fontObj?.missingFile &&
            /TimesNewRoman/i.test(fontObjName ?? "") &&
            scopedRecords.length > 0 &&
            scopedRecords.every((record) => !record.hasToUnicode)
        ),
      fontRecord: best,
      hasToUnicode: Boolean(fontObj?.toUnicode) || Boolean(best?.hasToUnicode),
      missingFile: Boolean(fontObj?.missingFile)
    });
  }
  return mapped;
}

function cloneState(state: GraphicsState): GraphicsState {
  return { ...state, ctm: cloneMatrix(state.ctm), textMatrix: cloneMatrix(state.textMatrix) };
}

function initialState(): GraphicsState {
  return {
    ctm: [1, 0, 0, 1, 0, 0],
    fillColor: "#000000",
    strokeColor: "#000000",
    fillColorSpace: "DeviceGray",
    strokeColorSpace: "DeviceGray",
    lineWidth: 1,
    lineWidthSet: false,
    dash: null,
    charSpacing: 0,
    wordSpacing: 0,
    textHScale: 1,
    leading: 0,
    textRise: 0,
    fontId: null,
    fontSize: 1,
    fontDirection: 1,
    textMatrix: [1, 0, 0, 1, 0, 0],
    x: 0,
    y: 0,
    lineX: 0,
    lineY: 0
  };
}

function isGlyph(value: unknown): value is PdfJsGlyph {
  return typeof value === "object" && value !== null && "width" in value;
}

function parseDrawPath(data: ArrayLike<number>): ParsedPath[] {
  const paths: ParsedPath[] = [];
  let current: ParsedPath | null = null;
  let start: Point | null = null;
  for (let i = 0; i < data.length;) {
    const op = data[i++];
    if (op === 0) {
      const point: Point = [Number(data[i++]), Number(data[i++])];
      current = { points: [point], closed: false, hasCurve: false, lastOp: op };
      start = point;
      paths.push(current);
    } else if (op === 1) {
      current?.points.push([Number(data[i++]), Number(data[i++])]);
      if (current) current.lastOp = op;
    } else if (op === 2) {
      i += 4;
      current?.points.push([Number(data[i++]), Number(data[i++])]);
      if (current) {
        current.hasCurve = true;
        current.lastOp = op;
      }
    } else if (op === 3) {
      i += 2;
      current?.points.push([Number(data[i++]), Number(data[i++])]);
      if (current) {
        current.hasCurve = true;
        current.lastOp = op;
      }
    } else if (op === 4) {
      if (current) {
        current.closed = true;
        if (current.fromRect) current.forceCurve = true;
        const last = current.points[current.points.length - 1];
        if (start && last) {
          if (last[0] !== start[0] || last[1] !== start[1]) {
            current.points.push(start);
          } else if (current.lastOp !== 1) {
            current.points.push(start);
          }
        }
        current.lastOp = op;
      }
    } else if (op === 5) {
      const x = Number(data[i++]);
      const y = Number(data[i++]);
      const width = Number(data[i++]);
      const height = Number(data[i++]);
      const point: Point = [x, y];
      current = {
        points: [point, [x + width, y], [x + width, y + height], [x, y + height], point],
        closed: true,
        hasCurve: false,
        lastOp: op,
        fromRect: true
      };
      start = point;
      paths.push(current);
    } else {
      break;
    }
  }
  return paths;
}

function isAxisAlignedRect(path: ParsedPath, inferFromGeometry = false): boolean {
  if (path.forceCurve) return false;
  if (path.fromRect) return true;
  void inferFromGeometry;
  const pts = [...path.points];
  if (path.closed) {
    while (pts.length > 1) {
      const first = pts[0];
      const last = pts[pts.length - 1];
      if (first[0] !== last[0] || first[1] !== last[1]) break;
      pts.pop();
    }
  }
  if (!path.closed || path.hasCurve || pts.length !== 4) return false;
  const xs = new Set(pts.map((p) => cleanNumber(p[0])));
  const ys = new Set(pts.map((p) => cleanNumber(p[1])));
  return xs.size === 2 && ys.size === 2;
}

function pathSignature(data: ArrayLike<number>): string {
  const ops: number[] = [];
  for (let i = 0; i < data.length;) {
    const op = Number(data[i++]);
    ops.push(op);
    if (op === 0 || op === 1) i += 2;
    else if (op === 2) i += 6;
    else if (op === 3 || op === 5) i += 4;
    else if (op !== 4) break;
  }
  return ops.join(",");
}

function pathOpsCompatible(candidate: number[], fallback: ArrayLike<number>): boolean {
  return candidate.length === fallback.length && pathSignature(candidate) === pathSignature(fallback);
}

function pathHasCurveOps(data: ArrayLike<number>): boolean {
  for (let i = 0; i < data.length;) {
    const op = Number(data[i++]);
    if (op === 2 || op === 3) return true;
    if (op === 0 || op === 1) i += 2;
    else if (op === 2) i += 6;
    else if (op === 3 || op === 5) i += 4;
    else if (op !== 4) break;
  }
  return false;
}

function collinearPathEndpoints(points: Point[]): [Point, Point] | null {
  const unique: Point[] = [];
  for (const point of points) {
    if (!unique.some((other) => cleanNumber(other[0]) === cleanNumber(point[0]) && cleanNumber(other[1]) === cleanNumber(point[1]))) unique.push(point);
  }
  if (unique.length !== 2) return null;
  return [unique[0], unique[1]];
}

export function extractPageObjects(
  pdfPage: any,
  operatorList: { fnArray: number[]; argsArray: unknown[] },
  textStyles: Record<string, any>,
  fontRecords: FontRecord[],
  pageNumber: number,
  pageWidth: number,
  pageHeight: number,
  pageRotate: number,
  doctopOffset: number,
  contentYOffset = 0,
  pageX0 = 0,
  pageTop = 0,
  colorOps: ColorOp[] = [],
  transformOps: Matrix[] = [],
  textMatrixOps: Matrix[] = [],
  textMoveOps: Point[] = [],
  textLeadingMoveOps: Point[] = [],
  pathOps: number[][] = [],
  imageResources: ImageResource[] = [],
  unicodeNorm?: OpenOptions["unicode_norm"],
  preferRgbFallback = false
): { chars: PDFObject[]; rects: PDFObject[]; lines: PDFObject[]; curves: PDFObject[]; images: PDFObject[] } {
  const fontIds = Array.from(new Set(operatorList.fnArray.flatMap((fn, i) => (fn === pdfjs.OPS.setFont ? [String((operatorList.argsArray[i] as unknown[])[0])] : []))));
  const fonts = mapFonts(pdfPage, textStyles, fontRecords, fontIds);
  const state = initialState();
  const stack: GraphicsState[] = [];
  const chars: PDFObject[] = [];
  const rects: PDFObject[] = [];
  const lines: PDFObject[] = [];
  const curves: PDFObject[] = [];
  const images: PDFObject[] = [];
  const contentPoint = (point: Point): Point => (contentYOffset ? [point[0], point[1] + contentYOffset] : point);
  const coordOffset: Point = [0, pageTop];
  const markedContent: Array<{ tag: string | null; mcid: number | null; suppressed?: boolean }> = [];
  const fillColorOps = colorOps.filter((op) => op.target === "fill");
  const strokeColorOps = colorOps.filter((op) => op.target === "stroke");
  let transformIndex = 0;
  let textMatrixIndex = 0;
  let textMoveIndex = 0;
  let textLeadingMoveIndex = 0;
  let pathIndex = 0;
  let imageIndex = 0;
  let sawDeviceCMYK = false;
  const constructPathTotal = operatorList.fnArray.filter((fn) => fn === pdfjs.OPS.constructPath).length;
  const partialRawPaths = pathOps.length > 0 && pathOps.length !== constructPathTotal;
  let usedPartialRawCurve = false;
  const markedExtras = () => {
    const current = [...markedContent].reverse().find((item) => !item.suppressed && item.tag !== "OC");
    return {
      mcid: current?.mcid ?? null,
      tag: current?.tag ?? null
    };
  };
  const colorFromHint = (fallback: unknown, queue: ColorOp[], currentColorSpace: string): { color: unknown; colorSpace: string | null; fromHint: boolean } => {
    const hint = queue.shift();
    if (hint?.pattern) return { color: hint.pattern, colorSpace: "Pattern", fromHint: true };
    if (hint?.components.length === 1) return { color: [cleanNumber(hint.components[0])], colorSpace: hint.colorSpace, fromHint: true };
    if (hint) return { color: hint.components.map(cleanNumber), colorSpace: hint.colorSpace, fromHint: true };
    if (!preferRgbFallback && currentColorSpace === "DeviceGray" && typeof fallback === "string" && /^#([0-9a-f]{2})\1\1$/i.test(fallback)) {
      const component = parseInt(fallback.slice(1, 3), 16);
      return { color: [component === 128 ? 0.5 : cleanNumber(component / 255)], colorSpace: "DeviceGray", fromHint: false };
    }
    if (typeof fallback === "string" && /^#[0-9a-f]{6}$/i.test(fallback)) return { color: rgbColor(fallback), colorSpace: "DeviceRGB", fromHint: false };
    return { color: rgbColor(fallback), colorSpace: null, fromHint: false };
  };
  const setColorFromGeneric = (target: "fill" | "stroke", fallback: unknown): void => {
    const queue = target === "fill" ? fillColorOps : strokeColorOps;
    const currentColorSpace = target === "fill" ? state.fillColorSpace : state.strokeColorSpace;
    if (Array.isArray(fallback) && fallback[0] === "TilingPattern") {
      const patternIndex = queue.findIndex((hint) => hint.pattern);
      if (patternIndex >= 0) {
        const [hint] = queue.splice(patternIndex, 1);
        if (target === "fill") {
          state.fillColor = hint.pattern;
          state.fillColorSpace = "Pattern";
        } else {
          state.strokeColor = hint.pattern;
          state.strokeColorSpace = "Pattern";
        }
        return;
      }
    }
    const hinted = colorFromHint(fallback, queue, currentColorSpace);
    if (target === "fill") {
      state.fillColor = hinted.color;
      if (hinted.colorSpace) state.fillColorSpace = hinted.colorSpace;
    } else {
      state.strokeColor = hinted.color;
      if (hinted.colorSpace) state.strokeColorSpace = hinted.colorSpace;
    }
  };

  for (let i = 0; i < operatorList.fnArray.length; i += 1) {
    const fn = operatorList.fnArray[i];
    const args = operatorList.argsArray[i] as any[] | null;
    switch (fn) {
      case pdfjs.OPS.save:
        stack.push(cloneState(state));
        break;
      case pdfjs.OPS.restore: {
        const restored = stack.pop();
        if (restored) Object.assign(state, restored);
        break;
      }
      case pdfjs.OPS.transform:
        state.ctm = multiplyMatrix(state.ctm, transformOps[transformIndex++] ?? (args as Matrix));
        break;
      case pdfjs.OPS.setLineWidth:
        state.lineWidth = Number(args?.[0] ?? state.lineWidth) * lineWidthScale(state.ctm);
        state.lineWidthSet = true;
        break;
      case pdfjs.OPS.setDash:
        state.dash = args ? [args[0] ?? [], args[1] ?? 0] : null;
        break;
      case pdfjs.OPS.beginMarkedContent:
        markedContent.push({ tag: decodePdfNameUtf8(args?.[0]?.name ?? String(args?.[0] ?? "")), mcid: null });
        break;
      case pdfjs.OPS.beginMarkedContentProps:
        markedContent.push({ tag: decodePdfNameUtf8(args?.[0]?.name ?? String(args?.[0] ?? "")), mcid: typeof args?.[1] === "number" ? args[1] : null });
        break;
      case pdfjs.OPS.endMarkedContent:
        {
          const ended = markedContent.pop();
          if (ended?.mcid === null && markedContent.length) markedContent[markedContent.length - 1].suppressed = true;
        }
        break;
      case pdfjs.OPS.setFillColorSpace:
        state.fillColorSpace = colorSpaceName(args?.[0]);
        break;
      case pdfjs.OPS.setStrokeColorSpace:
        state.strokeColorSpace = colorSpaceName(args?.[0]);
        break;
      case pdfjs.OPS.setFillRGBColor: {
        const hinted = colorFromHint(args?.[0], fillColorOps, state.fillColorSpace);
        if (
          !hinted.fromHint &&
          (sawDeviceCMYK || state.fillColorSpace === "DeviceCMYK" || state.strokeColorSpace === "DeviceCMYK") &&
          typeof args?.[0] === "string" &&
          args[0].toLowerCase() === "#ffffff"
        ) {
          state.fillColor = [0, 0, 0, 0];
          state.fillColorSpace = "DeviceCMYK";
        } else {
          state.fillColor = hinted.color;
          if (hinted.colorSpace) state.fillColorSpace = hinted.colorSpace;
        }
        if (state.fillColorSpace === "DeviceCMYK") sawDeviceCMYK = true;
        break;
      }
      case pdfjs.OPS.setFillGray:
        state.fillColor = args?.length === 1 ? args[0] : args ?? state.fillColor;
        state.fillColorSpace = "DeviceGray";
        break;
      case pdfjs.OPS.setFillCMYKColor:
        state.fillColorSpace = "DeviceCMYK";
        state.fillColor = args ?? state.fillColor;
        sawDeviceCMYK = true;
        break;
      case pdfjs.OPS.setFillColor:
      case pdfjs.OPS.setFillColorN:
        setColorFromGeneric("fill", args ?? state.fillColor);
        break;
      case pdfjs.OPS.setStrokeRGBColor: {
        const hinted = colorFromHint(args?.[0], strokeColorOps, state.strokeColorSpace);
        if (
          !hinted.fromHint &&
          (sawDeviceCMYK || state.fillColorSpace === "DeviceCMYK" || state.strokeColorSpace === "DeviceCMYK") &&
          typeof args?.[0] === "string" &&
          args[0].toLowerCase() === "#ffffff"
        ) {
          state.strokeColor = [0, 0, 0, 0];
          state.strokeColorSpace = "DeviceCMYK";
        } else {
          state.strokeColor = hinted.color;
          if (hinted.colorSpace) state.strokeColorSpace = hinted.colorSpace;
        }
        if (state.strokeColorSpace === "DeviceCMYK") sawDeviceCMYK = true;
        break;
      }
      case pdfjs.OPS.setStrokeGray:
        state.strokeColor = args?.length === 1 ? args[0] : args ?? state.strokeColor;
        state.strokeColorSpace = "DeviceGray";
        break;
      case pdfjs.OPS.setStrokeCMYKColor:
        state.strokeColorSpace = "DeviceCMYK";
        state.strokeColor = args ?? state.strokeColor;
        sawDeviceCMYK = true;
        break;
      case pdfjs.OPS.setStrokeColor:
      case pdfjs.OPS.setStrokeColorN:
        setColorFromGeneric("stroke", args ?? state.strokeColor);
        break;
      case pdfjs.OPS.setCharSpacing:
        state.charSpacing = Number(args?.[0] ?? 0);
        break;
      case pdfjs.OPS.setWordSpacing:
        state.wordSpacing = Number(args?.[0] ?? 0);
        break;
      case pdfjs.OPS.setHScale:
        state.textHScale = Number(args?.[0] ?? 100) / 100;
        break;
      case pdfjs.OPS.setLeading:
        state.leading = -Number(args?.[0] ?? 0);
        break;
      case pdfjs.OPS.setTextRise:
        state.textRise = Number(args?.[0] ?? 0);
        break;
      case pdfjs.OPS.beginText:
        state.textMatrix = [1, 0, 0, 1, 0, 0];
        state.x = state.lineX = 0;
        state.y = state.lineY = 0;
        break;
      case pdfjs.OPS.setFont: {
        state.fontId = String(args?.[0]);
        const size = Number(args?.[1] ?? 1);
        state.fontSize = Math.abs(size);
        state.fontDirection = size < 0 ? -1 : 1;
        break;
      }
      case pdfjs.OPS.moveText:
        {
          const rawMove = textMoveOps[textMoveIndex++];
          const tx = rawMove?.[0] ?? Number(args?.[0] ?? 0);
          const ty = rawMove?.[1] ?? Number(args?.[1] ?? 0);
          state.x = state.lineX += tx;
          state.y = state.lineY += ty;
        }
        break;
      case pdfjs.OPS.setLeadingMoveText:
        {
          const rawMove = textLeadingMoveOps[textLeadingMoveIndex++];
          const tx = rawMove?.[0] ?? Number(args?.[0] ?? 0);
          const ty = rawMove?.[1] ?? Number(args?.[1] ?? 0);
          state.leading = ty;
          state.x = state.lineX += tx;
          state.y = state.lineY += ty;
        }
        break;
      case pdfjs.OPS.nextLine:
        state.x = state.lineX;
        state.y = state.lineY += state.leading;
        break;
      case pdfjs.OPS.setTextMatrix:
        {
          const rawMatrix = textMatrixOps[textMatrixIndex++];
          state.textMatrix = rawMatrix
            ? cloneMatrix(rawMatrix)
            : (Array.from(args?.[0] ?? args ?? [1, 0, 0, 1, 0, 0]).map((value) => snapPdfOperand(Number(value))) as Matrix);
        }
        state.x = state.lineX = 0;
        state.y = state.lineY = 0;
        break;
      case pdfjs.OPS.showText:
      case pdfjs.OPS.showSpacedText: {
        const glyphs = (args?.[0] ?? []) as unknown[];
        const font = state.fontId ? fonts.get(state.fontId) : undefined;
        if (!font) break;
        const matrix = multiplyMatrix(state.ctm, state.textMatrix);
        const textHScale = state.textHScale * state.fontDirection;
        let x = 0;
        let needCharSpacing = false;
        const spacingDelta = (value: number): number => font.vertical ? -value * state.fontDirection : value * state.fontDirection;
        for (let glyphIndex = 0; glyphIndex < glyphs.length; glyphIndex += 1) {
          const glyph = glyphs[glyphIndex];
          if (typeof glyph === "number") {
            x += (font.vertical ? 1 : -1) * glyph * state.fontSize / 1000;
            needCharSpacing = true;
            continue;
          }
          if (!isGlyph(glyph)) continue;
          if (needCharSpacing) x += spacingDelta(state.charSpacing);
          const rawText = glyphTextFromPdfJsGlyph(font, glyph);
          const text = unicodeNorm ? rawText.normalize(unicodeNorm) : rawText;
          const glyphWidth = glyphWidthLikePdfminer(font, glyph);
          const verticalMetric = Array.isArray(glyph.vmetric) ? (glyph.vmetric as number[]) : null;
          const advance =
            text === "\n" && Number(glyph.originalCharCode ?? 0) <= 2
              ? 0
              : font.vertical && !verticalMetric
                ? state.fontSize
                : glyphWidth * state.fontSize * font.fontMatrix0;
          const verticalX = Number(verticalMetric?.[1] ?? 500) * state.fontSize * font.fontMatrix0;
          const verticalY = Number(verticalMetric?.[2] ?? 880) * state.fontSize * font.fontMatrix0;
          const originalCharCode = Number(glyph.originalCharCode ?? 0);
          const splitChars =
            !font.vertical && font.missingFile && originalCharCode > 0xff && !text.startsWith("(cid:") ? Array.from(text) : [text];
          const metricParts =
            !font.vertical &&
            font.missingFile &&
            ((text === " " && glyphWidth >= 900) ||
              (originalCharCode > 0xff && splitChars.length === 1 && font.fontRecord?.subtype !== "Type0" && font.fontRecord?.subtype !== "CIDFontType2"))
              ? 2
              : splitChars.length;
          for (let partIndex = 0; partIndex < splitChars.length; partIndex += 1) {
            const partText = splitChars[partIndex];
            const partStart = x + (advance * partIndex) / metricParts;
            const partEnd = x + (advance * (partIndex + 1)) / metricParts;
            const xStart = font.vertical ? state.x - verticalX : state.x + partStart * textHScale;
            const xEnd = font.vertical ? xStart + state.fontSize : state.x + partEnd * textHScale;
            const yStart = font.vertical ? state.y + state.textRise - verticalY - partStart : state.y + state.textRise + font.descent * state.fontSize;
            const yEnd = font.vertical ? yStart + state.fontSize : yStart + state.fontSize;
            const rawBBox = bboxFromPoints([
              contentPoint(applyMatrix([xStart, yStart], matrix)),
              contentPoint(applyMatrix([xStart, yEnd], matrix)),
              contentPoint(applyMatrix([xEnd, yStart], matrix)),
              contentPoint(applyMatrix([xEnd, yEnd], matrix))
            ]);
            const originX = font.vertical ? state.x + x : xStart;
            const originY = state.y + state.textRise;
            const [matrixE, matrixF] = contentPoint(applyMatrix([originX, originY], matrix));
            const glyphMatrix = cleanMatrix(matrixToPageMatrix([matrix[0], matrix[1], matrix[2], matrix[3], matrixE, matrixF], pageWidth, pageHeight, pageRotate));
            const upright =
              pageRotate === 90 || pageRotate === 270
                ? Math.abs(matrix[0]) < 1e-6 && Math.abs(matrix[3]) < 1e-6
                : matrix[0] * matrix[3] * textHScale > 0 && matrix[1] * matrix[2] <= 0;
            glyphMatrix[4] = cleanNumber(glyphMatrix[4] - pageX0);
            glyphMatrix[5] = cleanNumber(glyphMatrix[5] + pageTop);
            const obj = rectFromPdfBBox(rawBBox, pageWidth, pageHeight, pageRotate, pageNumber, "char", doctopOffset, {
              text: partText,
              fontname: font.fontname,
              adv:
                font.vertical && verticalMetric
                  ? cleanNumber(Number(verticalMetric[0]) * state.fontSize * font.fontMatrix0)
                  : cleanNumber(Math.abs((advance * textHScale) / metricParts)),
              upright,
              matrix: glyphMatrix,
              ncs: state.fillColorSpace,
              non_stroking_color: colorValue(state.fillColor),
              stroking_color: colorValue(state.strokeColor),
              ...markedExtras()
            }, coordOffset);
            obj.size = font.vertical ? obj.width : obj.height;
            chars.push(obj);
          }
          x += advance;
          if (glyph.isSpace) x += spacingDelta(state.wordSpacing);
          needCharSpacing = true;
        }
        if (font.vertical) state.y -= x;
        else state.x += x * textHScale;
        break;
      }
      case pdfjs.OPS.constructPath: {
        const paintOp = Number(args?.[0]);
        const pdfJsPath = (args?.[1]?.[0] ?? []) as ArrayLike<number>;
        const candidatePath = pathOps[pathIndex];
        const compatibleRawPath = candidatePath && (!partialRawPaths || pathOpsCompatible(candidatePath, pdfJsPath)) ? candidatePath : undefined;
        const compatibleHasCurve = compatibleRawPath ? pathHasCurveOps(compatibleRawPath) : false;
        const rawPath = compatibleRawPath && (!partialRawPaths || (compatibleHasCurve && !usedPartialRawCurve)) ? compatibleRawPath : undefined;
        if (compatibleRawPath) pathIndex += 1;
        if (rawPath && partialRawPaths && compatibleHasCurve) {
          usedPartialRawCurve = true;
          pathIndex = pathOps.length;
        }
        const raw = (rawPath ?? pdfJsPath) as ArrayLike<number>;
        const pathBBox = (points: Point[]): MutableBBox => bboxFromPoints(points);
        const paths = parseDrawPath(raw);
        const isStroke =
          paintOp === pdfjs.OPS.stroke ||
          paintOp === pdfjs.OPS.closeStroke ||
          paintOp === pdfjs.OPS.fillStroke ||
          paintOp === pdfjs.OPS.eoFillStroke ||
          paintOp === pdfjs.OPS.closeFillStroke ||
          paintOp === pdfjs.OPS.closeEOFillStroke;
        const isFill =
          paintOp === pdfjs.OPS.fill ||
          paintOp === pdfjs.OPS.eoFill ||
          paintOp === pdfjs.OPS.fillStroke ||
          paintOp === pdfjs.OPS.eoFillStroke ||
          paintOp === pdfjs.OPS.closeFillStroke ||
          paintOp === pdfjs.OPS.closeEOFillStroke;
        const linewidth = state.lineWidthSet ? state.lineWidth : 0;
        const vectorExtras = {
          linewidth,
          dash: state.dash,
          fill: isFill,
          stroke: isStroke,
          evenodd: paintOp === pdfjs.OPS.eoFill || paintOp === pdfjs.OPS.eoFillStroke || paintOp === pdfjs.OPS.closeEOFillStroke,
          stroking_color: graphicColorValue(state.strokeColor, state.strokeColorSpace),
          non_stroking_color: graphicColorValue(state.fillColor, state.fillColorSpace),
          ...markedExtras()
        };
        const lineExtras = vectorExtras;
        for (const path of paths) {
          const operandPoints = rawPath ? path.points : path.points.map(snapPathOperandPoint);
          const transformed = operandPoints.map((point) => contentPoint(applyMatrix(point, state.ctm)));
          const inferRectFromGeometry = rawPath === undefined;
          if (isStroke && !isFill) {
            const rawBBox = pathBBox(transformed);
            if (path.closed || path.hasCurve || transformed.length > 2) {
              const lineEndpoints = !path.hasCurve ? collinearPathEndpoints(transformed) : null;
              if (lineEndpoints) {
                const rawLineBBox = pathBBox(lineEndpoints);
                lines.push(rectFromPdfBBox(rawLineBBox, pageWidth, pageHeight, pageRotate, pageNumber, "line", doctopOffset, path.closed ? vectorExtras : lineExtras, coordOffset));
              } else if (isAxisAlignedRect({ ...path, points: transformed }, inferRectFromGeometry)) {
                rects.push(
                  rectFromPdfBBox(rawBBox, pageWidth, pageHeight, pageRotate, pageNumber, "rect", doctopOffset, {
                    ...vectorExtras,
                    linewidth: isStroke || state.lineWidthSet ? linewidth : 0
                  }, coordOffset)
                );
              } else {
                curves.push(
                  rectFromPdfBBox(rawBBox, pageWidth, pageHeight, pageRotate, pageNumber, "curve", doctopOffset, {
                    pts: transformed.map((point) => pointToPageCoords(point, pageWidth, pageHeight, pageRotate).map(cleanNumber)),
                    ...vectorExtras
                  }, coordOffset)
                );
              }
            } else {
              if (transformed.length === 1 && (state.dash != null || (transformed[0][0] === 0 && transformed[0][1] === 0))) {
                const rawPointBBox = pathBBox(transformed);
                curves.push(
                  rectFromPdfBBox(rawPointBBox, pageWidth, pageHeight, pageRotate, pageNumber, "curve", doctopOffset, {
                    pts: transformed.map((point) => pointToPageCoords(point, pageWidth, pageHeight, pageRotate).map(cleanNumber)),
                    ...vectorExtras
                  }, coordOffset)
                );
              }
              for (let p = 0; p < transformed.length - 1; p += 1) {
                const rawLineBBox = pathBBox([transformed[p], transformed[p + 1]]);
                lines.push(rectFromPdfBBox(rawLineBBox, pageWidth, pageHeight, pageRotate, pageNumber, "line", doctopOffset, lineExtras, coordOffset));
              }
            }
          } else if (isFill) {
            const rawBBox = pathBBox(transformed);
            const lineEndpoints = !path.hasCurve ? collinearPathEndpoints(transformed) : null;
            if (lineEndpoints && (rawBBox[0] === rawBBox[2] || rawBBox[1] === rawBBox[3])) {
              const rawLineBBox = pathBBox(lineEndpoints);
              lines.push(rectFromPdfBBox(rawLineBBox, pageWidth, pageHeight, pageRotate, pageNumber, "line", doctopOffset, vectorExtras, coordOffset));
            } else if (isAxisAlignedRect({ ...path, points: transformed }, inferRectFromGeometry)) {
              rects.push(
                rectFromPdfBBox(rawBBox, pageWidth, pageHeight, pageRotate, pageNumber, "rect", doctopOffset, {
                  ...vectorExtras,
                  linewidth: isStroke || state.lineWidthSet ? linewidth : 0
                }, coordOffset)
              );
            } else if (transformed.length > 1 && !(isStroke && (rawBBox[0] === rawBBox[2] || rawBBox[1] === rawBBox[3]))) {
              const curve = rectFromPdfBBox(rawBBox, pageWidth, pageHeight, pageRotate, pageNumber, "curve", doctopOffset, {
                pts: transformed.map((point) => pointToPageCoords(point, pageWidth, pageHeight, pageRotate).map(cleanNumber)),
                ...vectorExtras,
                linewidth: isStroke || state.lineWidthSet ? linewidth : 0
              }, coordOffset);
              curves.push(curve);
            }
          }
        }
        break;
      }
      case pdfjs.OPS.paintImageXObject:
      case pdfjs.OPS.paintInlineImageXObject:
      case pdfjs.OPS.paintImageMaskXObject: {
        const resource = imageResources[imageIndex++];
        const srcWidth = Number(resource?.width ?? args?.[1] ?? args?.[0]?.width ?? 0);
        const srcHeight = Number(resource?.height ?? args?.[2] ?? args?.[0]?.height ?? 0);
        const rawBBox = bboxFromPoints([
          contentPoint(applyMatrix([0, 0], state.ctm)),
          contentPoint(applyMatrix([1, 0], state.ctm)),
          contentPoint(applyMatrix([0, 1], state.ctm)),
          contentPoint(applyMatrix([1, 1], state.ctm))
        ]);
        const image = rectFromPdfBBox(rawBBox, pageWidth, pageHeight, pageRotate, pageNumber, "image", doctopOffset, {
            name: resource?.name ?? (typeof args?.[0] === "string" ? args[0] : undefined),
            srcsize: srcWidth && srcHeight ? [srcWidth, srcHeight] : undefined,
            colorspace: resource?.colorspace,
            bits: resource?.bits,
            ...markedExtras()
          }, coordOffset);
        for (const key of ["width", "height"] as const) {
          image[key] = softenHalfMicro(Number(image[key]));
        }
        images.push(image);
        break;
      }
    }
  }

  return { chars, rects, lines, curves, images };
}

export function annotationToObject(
  annotation: any,
  pageNumber: number,
  pageWidth: number,
  pageHeight: number,
  pageRotate: number,
  doctopOffset: number,
  objects?: Map<number, string>
): PDFObject {
  const optionalString = (value: unknown): string | null => (typeof value === "string" && value.length > 0 ? value : null);
  const pdfplumberContents = (value: unknown): string | null => (typeof value === "string" ? value : null);
  const objectNumber = Number.parseInt(String(annotation.id ?? ""), 10);
  const annotationObject = Number.isFinite(objectNumber) ? objects?.get(objectNumber) : undefined;
  const rawRect = annotation.subtype === "Text" ? extractRawBox(annotationObject, "Rect") : null;
  const rect = rawRect ?? (annotation.rect as number[]);
  const rawContents = annotation.contentsObj?.str ?? annotation.contents ?? null;
  const rawContentsFromObject = annotationObject ? readPdfDictStringLikePdfplumber(annotationObject, "Contents") : null;
  const rawTitle = annotationObject ? readPdfDictStringLikePdfplumber(annotationObject, "T") : null;
  const isTinyPopup = annotation.subtype === "Popup" && Math.abs(Number(rect[2]) - Number(rect[0])) <= 2 && Math.abs(Number(rect[3]) - Number(rect[1])) <= 2;
  return rectFromPdfBBox([rect[0], rect[1], rect[2], rect[3]], pageWidth, pageHeight, pageRotate, pageNumber, "annot", doctopOffset, {
    uri: optionalString(annotation.unsafeUrl ?? annotation.url ?? null),
    title: annotation.subtype === "Popup" ? null : optionalString(annotation.titleObj?.str ?? annotation.title ?? rawTitle ?? null),
    contents:
      (annotation.subtype === "Popup" && !isTinyPopup) ||
      ((annotation.subtype === "Link" || annotation.subtype === "Widget" || (annotation.subtype === "Stamp" && pageRotate !== 0)) && (rawContentsFromObject ?? rawContents) === "")
        ? null
        : pdfplumberContents(rawContentsFromObject ?? rawContents)
  });
}

function readPdfDictStringLikePdfplumber(objectText: string, key: string): string | null {
  const bytes = parsePdfDictBytes(objectText, key);
  return bytes ? decodePdfLiteralBytesAsUtf8ThenUtf16(bytes) : null;
}
