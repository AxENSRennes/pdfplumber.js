import { decodePdfStringLikePdfminer } from "../pdf-strings.js";
import { formatIntAlphaLikePdfminer, formatIntRomanLikePdfminer } from "../utils.js";
import { asArray, asDict, asName, asNumber, isRef, isStream, latin1Bytes } from "./primitives.js";
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

  getCatalog(): PdfDict | undefined {
    for (const objectNumber of this.rawObjects.keys()) {
      const value = this.getObject(objectNumber)?.value;
      const dict = value instanceof Map ? value : isStream(value) ? value.dict : undefined;
      if (asName(dict?.get("Type")) === "Catalog") return dict;
    }
    return undefined;
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

function resolvedDict(store: PdfObjectStore, value: PdfPrimitive | undefined): PdfDict | undefined {
  const resolved = store.resolve(value);
  return asDict(resolved) ?? (isStream(resolved) ? resolved.dict : undefined);
}

function resolvedArray(store: PdfObjectStore, value: PdfPrimitive | undefined): PdfArray | undefined {
  return asArray(store.resolve(value));
}

function resolvedNumber(store: PdfObjectStore, value: PdfPrimitive | undefined): number | undefined {
  return asNumber(store.resolve(value));
}

function labelPrefixLikePdfminer(value: PdfPrimitive | undefined): string {
  const resolved = storelessString(value);
  return resolved == null ? "" : decodePdfStringLikePdfminer(latin1Bytes(resolved));
}

function storelessString(value: PdfPrimitive | undefined): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function formatPageLabelLikePdfminer(value: number, style: string | undefined): string {
  if (style == null) return "";
  if (style === "D") return String(value);
  if (style === "R") return formatIntRomanLikePdfminer(value).toUpperCase();
  if (style === "r") return formatIntRomanLikePdfminer(value);
  if (style === "A") return formatIntAlphaLikePdfminer(value).toUpperCase();
  if (style === "a") return formatIntAlphaLikePdfminer(value);
  return "";
}

function pageLabelRanges(store: PdfObjectStore, tree: PdfDict | undefined): Array<[number, PdfDict]> {
  const out: Array<[number, PdfDict]> = [];
  const visit = (node: PdfDict | undefined): void => {
    if (!node) return;
    const nums = resolvedArray(store, node.get("Nums"));
    if (nums) {
      for (let index = 0; index + 1 < nums.length; index += 2) {
        const pageIndex = resolvedNumber(store, nums[index]);
        const dict = resolvedDict(store, nums[index + 1]);
        if (pageIndex != null && dict) out.push([pageIndex, dict]);
      }
    }
    for (const kid of resolvedArray(store, node.get("Kids")) ?? []) visit(resolvedDict(store, kid));
  };
  visit(tree);
  return out.sort((a, b) => a[0] - b[0]);
}

export function parsePageLabelsLikePdfminer(store: PdfObjectStore, pageCount: number): string[] | null {
  const labelsTree = resolvedDict(store, store.getCatalog()?.get("PageLabels"));
  if (!labelsTree) return null;
  const ranges = pageLabelRanges(store, labelsTree);
  if (!ranges.length) return null;
  if (ranges[0][0] !== 0) ranges.unshift([0, new Map()]);
  const out: string[] = [];
  for (let rangeIndex = 0; rangeIndex < ranges.length && out.length < pageCount; rangeIndex += 1) {
    const [start, dict] = ranges[rangeIndex];
    const end = Math.min(ranges[rangeIndex + 1]?.[0] ?? pageCount, pageCount);
    const style = asName(store.resolve(dict.get("S")));
    const prefix = labelPrefixLikePdfminer(store.resolve(dict.get("P")));
    const first = resolvedNumber(store, dict.get("St")) ?? 1;
    for (let pageIndex = start; pageIndex < end && pageIndex < pageCount; pageIndex += 1) {
      out[pageIndex] = `${prefix}${formatPageLabelLikePdfminer(first + pageIndex - start, style)}`;
    }
  }
  return Array.from({ length: pageCount }, (_unused, index) => out[index] ?? "");
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
