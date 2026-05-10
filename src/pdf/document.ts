import { asArray, asName, asNumber, isRef, isStream } from "./primitives.js";
import type { PdfArray, PdfDict, PdfIndirectObject, PdfPrimitive, PdfRef, PdfStream } from "./primitives.js";
import { findKeywordOutsideSyntax, parseObject } from "./parser.js";
import { pdfNumbersFromString } from "./tokenizer.js";
import { decodePdfStreamText, decodeStreamToLatin1 } from "./streams.js";
import { decryptPdfStreamObject, parsePdfEncryption } from "./security.js";

export interface ParsePdfDocumentOptions {
  password?: string;
}

export interface PdfPageModel {
  objectNumber: number;
  raw: string;
  dict: PdfDict | undefined;
  getDecodedContent(): string;
}

export class PdfObjectStore {
  private parsedObjects = new Map<number, PdfIndirectObject | null>();

  constructor(readonly raw: string, readonly rawObjects: Map<number, string>) {}

  getObject(objectNumber: number): PdfIndirectObject | undefined {
    if (!this.parsedObjects.has(objectNumber)) {
      const raw = this.rawObjects.get(objectNumber);
      this.parsedObjects.set(objectNumber, raw ? parseObject(raw) : null);
    }
    return this.parsedObjects.get(objectNumber) ?? undefined;
  }

  getRawObjectText(objectNumber: number): string | undefined {
    return this.rawObjects.get(objectNumber);
  }

  resolve(value: PdfPrimitive | undefined): PdfPrimitive | undefined {
    if (isRef(value)) return this.getObject(value.objectNumber)?.value;
    return value;
  }

  getStream(objectNumber: number): PdfStream | undefined {
    return this.getObject(objectNumber)?.stream;
  }

  getPageModel(objectNumber: number | undefined): PdfPageModel | undefined {
    if (objectNumber == null) return undefined;
    const object = this.getObject(objectNumber);
    if (!object) return undefined;
    const dict = object.value instanceof Map ? object.value : object.stream?.dict;
    return {
      objectNumber,
      raw: object.raw,
      dict,
      getDecodedContent: () => this.decodedPageContent(dict)
    };
  }

  private decodedPageContent(dict: PdfDict | undefined): string {
    const contents = this.resolve(dict?.get("Contents"));
    const decodeOne = (value: PdfPrimitive | undefined): string => {
      const resolved = this.resolve(value);
      if (isStream(resolved)) return decodeStreamToLatin1(resolved);
      if (isRef(value)) return decodePdfStreamText(this.getRawObjectText(value.objectNumber));
      return "";
    };
    if (Array.isArray(contents)) return contents.map((item) => decodeOne(item)).join("\n");
    return decodeOne(contents);
  }
}

function findObjectEnd(raw: string, start: number): number {
  let index = start;
  while (index < raw.length) {
    const stream = findKeywordOutsideSyntax(raw, "stream", index);
    const endobj = findKeywordOutsideSyntax(raw, "endobj", index);
    if (endobj < 0) return -1;
    if (stream >= 0 && stream < endobj) {
      const endstream = raw.indexOf("endstream", stream + "stream".length);
      if (endstream < 0) return endobj;
      index = endstream + "endstream".length;
      continue;
    }
    return endobj;
  }
  return -1;
}

function collectIndirectObjects(raw: string): Map<number, string> {
  const objects = new Map<number, string>();
  const re = /(?:^|[\r\n])(\d+)\s+(\d+)\s+obj\b/g;
  let match: RegExpExecArray | null;
  while ((match = re.exec(raw))) {
    const start = match.index + (/^[\r\n]/.test(match[0]) ? 1 : 0);
    const end = findObjectEnd(raw, re.lastIndex);
    if (end < 0) continue;
    objects.set(Number(match[1]), raw.slice(start, end + "endobj".length));
    re.lastIndex = end + "endobj".length;
  }
  return objects;
}

function expandObjectStreams(objects: Map<number, string>): void {
  for (const text of [...objects.values()]) {
    const object = parseObject(text);
    const stream = object?.stream;
    if (!stream || asName(stream.dict.get("Type")) !== "ObjStm") continue;
    const first = asNumber(stream.dict.get("First"));
    const count = asNumber(stream.dict.get("N"));
    if (first == null || count == null) continue;
    const decoded = decodeStreamToLatin1(stream);
    if (!decoded || decoded.length <= first) continue;
    const values = pdfNumbersFromString(decoded.slice(0, first));
    if (values.length < count * 2) continue;
    const body = decoded.slice(first);
    const entries = Array.from({ length: count }, (_unused, index) => ({
      objectNumber: values[index * 2],
      offset: values[index * 2 + 1]
    })).sort((a, b) => a.offset - b.offset);
    for (let i = 0; i < entries.length; i += 1) {
      const entry = entries[i];
      const end = entries[i + 1]?.offset ?? body.length;
      objects.set(entry.objectNumber, `${entry.objectNumber} 0 obj\n${body.slice(entry.offset, end).trim()}\nendobj`);
    }
  }
}

function parsePdfNumbersCompat(value: string): number[] {
  return Array.from(value.matchAll(/[-+]?(?:\d+\.\d*|\.\d+|\d+)/g)).map((match) => Number(match[0]));
}

function expandObjectStreamsCompat(objects: Map<number, string>): void {
  for (const text of [...objects.values()]) {
    if (!/\/Type\s*\/ObjStm\b/.test(text)) continue;
    const first = Number(text.match(/\/First\s+(\d+)/)?.[1] ?? Number.NaN);
    const count = Number(text.match(/\/N\s+(\d+)/)?.[1] ?? Number.NaN);
    if (!Number.isFinite(first) || !Number.isFinite(count)) continue;
    const decoded = decodePdfStreamText(text);
    if (!decoded || decoded.length <= first) continue;
    const header = decoded.slice(0, first);
    const values = parsePdfNumbersCompat(header);
    if (values.length < count * 2) continue;
    const body = decoded.slice(first);
    const entries = Array.from({ length: count }, (_unused, index) => ({
      objectNumber: values[index * 2],
      offset: values[index * 2 + 1]
    })).sort((a, b) => a.offset - b.offset);
    for (let i = 0; i < entries.length; i += 1) {
      const entry = entries[i];
      const end = entries[i + 1]?.offset ?? body.length;
      objects.set(entry.objectNumber, `${entry.objectNumber} 0 obj\n${body.slice(entry.offset, end).trim()}\nendobj`);
    }
  }
}

export function parsePdfObjectsCompat(raw: string, password = ""): Map<number, string> {
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
  expandObjectStreamsCompat(objects);
  return objects;
}

export function parsePdfDocument(raw: string, options: ParsePdfDocumentOptions = {}): PdfObjectStore {
  const objects = parsePdfObjectsCompat(raw, options.password ?? "");
  return new PdfObjectStore(raw, objects);
}
