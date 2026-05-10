import { latin1Bytes, pdfRef } from "./primitives.js";
import type { PdfArray, PdfDict, PdfIndirectObject, PdfOperation, PdfPrimitive, PdfStream } from "./primitives.js";
import { tokenizePdf } from "./tokenizer.js";
import type { PdfToken } from "./tokenizer.js";

class PrimitiveParser {
  constructor(
    readonly tokens: PdfToken[],
    public index = 0
  ) {}

  peek(offset = 0): PdfToken | undefined {
    return this.tokens[this.index + offset];
  }

  consume(): PdfToken | undefined {
    return this.tokens[this.index++];
  }

  parseValue(): PdfPrimitive | undefined {
    const token = this.consume();
    if (!token) return undefined;
    if (token.type === "number") {
      const generation = this.peek(0);
      const marker = this.peek(1);
      if (generation?.type === "number" && marker?.type === "keyword" && marker.value === "R") {
        this.index += 2;
        return pdfRef(token.value, generation.value);
      }
      return token.value;
    }
    if (token.type === "name") return token.value;
    if (token.type === "string" || token.type === "hexString") return token.value;
    if (token.type === "arrayStart") return this.parseArray();
    if (token.type === "dictStart") return this.parseDict();
    if (token.type === "keyword") {
      if (token.value === "true") return true;
      if (token.value === "false") return false;
      if (token.value === "null") return null;
      return token.value;
    }
    return undefined;
  }

  parseArray(): PdfArray {
    const values: PdfArray = [];
    while (this.index < this.tokens.length && this.peek()?.type !== "arrayEnd") {
      const value = this.parseValue();
      if (value !== undefined) values.push(value);
    }
    if (this.peek()?.type === "arrayEnd") this.index += 1;
    return values;
  }

  parseDict(): PdfDict {
    const dict: PdfDict = new Map();
    while (this.index < this.tokens.length && this.peek()?.type !== "dictEnd") {
      const key = this.consume();
      if (key?.type !== "name") continue;
      const value = this.parseValue();
      if (value !== undefined) dict.set(key.value.name, value);
    }
    if (this.peek()?.type === "dictEnd") this.index += 1;
    return dict;
  }
}

function isKeywordBoundary(source: string, start: number, end: number): boolean {
  const before = source[start - 1];
  const after = source[end];
  const regular = (char: string | undefined): boolean => char != null && !/[\s()[\]<>/%{}]/.test(char);
  return !regular(before) && !regular(after);
}

export function findKeywordOutsideSyntax(source: string, keyword: string, start = 0, end = source.length): number {
  let index = start;
  while (index < end) {
    const char = source[index];
    if (/\s/.test(char)) {
      index += 1;
      continue;
    }
    if (char === "%") {
      while (index < end && source[index] !== "\n" && source[index] !== "\r") index += 1;
      continue;
    }
    if (char === "(") {
      let depth = 1;
      index += 1;
      while (index < end && depth > 0) {
        const current = source[index++];
        if (current === "\\") index += 1;
        else if (current === "(") depth += 1;
        else if (current === ")") depth -= 1;
      }
      continue;
    }
    if (char === "<" && source[index + 1] !== "<") {
      index += 1;
      while (index < end && source[index] !== ">") index += 1;
      index += 1;
      continue;
    }
    if (source.startsWith(keyword, index) && isKeywordBoundary(source, index, index + keyword.length)) return index;
    index += 1;
  }
  return -1;
}

function streamDataBounds(raw: string, streamKeywordIndex: number): { start: number; end: number } | null {
  let start = streamKeywordIndex + "stream".length;
  if (raw[start] === "\r" && raw[start + 1] === "\n") start += 2;
  else if (raw[start] === "\r" || raw[start] === "\n") start += 1;
  const endstream = raw.indexOf("endstream", start);
  if (endstream < 0) return null;
  let end = endstream;
  if (raw[end - 2] === "\r" && raw[end - 1] === "\n") end -= 2;
  else if (raw[end - 1] === "\r" || raw[end - 1] === "\n") end -= 1;
  return { start, end };
}

export function parsePrimitive(source: string): PdfPrimitive | undefined {
  return new PrimitiveParser(tokenizePdf(source)).parseValue();
}

export function parseDict(source: string): PdfDict {
  const parsed = parsePrimitive(source);
  return parsed instanceof Map ? parsed : new Map();
}

export function parseContentStream(source: string): PdfToken[] {
  return tokenizePdf(source);
}

export function parseOperatorStream(source: string): PdfOperation[] {
  const parser = new PrimitiveParser(tokenizePdf(source));
  const operations: PdfOperation[] = [];
  let args: PdfPrimitive[] = [];
  while (parser.index < parser.tokens.length) {
    const token = parser.peek();
    if (!token) break;
    if (token.type === "keyword") {
      parser.consume();
      operations.push({ operator: token.value, args });
      args = [];
      continue;
    }
    const value = parser.parseValue();
    if (value === undefined) continue;
    if (value instanceof Map && parser.peek()?.type !== "keyword") {
      args = [];
      continue;
    }
    args.push(value);
  }
  return operations;
}

export function parseObject(rawObjectText: string): PdfIndirectObject | null {
  const header = rawObjectText.match(/^\s*(\d+)\s+(\d+)\s+obj\b/);
  if (!header) return null;
  const objectNumber = Number(header[1]);
  const generation = Number(header[2]);
  const bodyStart = header.index! + header[0].length;
  const endobj = findKeywordOutsideSyntax(rawObjectText, "endobj", bodyStart);
  const objectEnd = endobj >= 0 ? endobj : rawObjectText.length;
  const streamKeyword = findKeywordOutsideSyntax(rawObjectText, "stream", bodyStart, objectEnd);
  const valueText = rawObjectText.slice(bodyStart, streamKeyword >= 0 ? streamKeyword : objectEnd).trim();
  const value = parsePrimitive(valueText) ?? null;
  let stream: PdfStream | undefined;
  if (streamKeyword >= 0 && value instanceof Map) {
    const bounds = streamDataBounds(rawObjectText, streamKeyword);
    if (bounds) {
      const rawData = rawObjectText.slice(bounds.start, bounds.end);
      stream = { kind: "stream", dict: value, rawData, data: latin1Bytes(rawData) };
      return { objectNumber, generation, value: stream, stream, raw: rawObjectText };
    }
  }
  return { objectNumber, generation, value, raw: rawObjectText };
}
