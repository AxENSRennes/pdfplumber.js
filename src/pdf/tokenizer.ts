import { latin1String, pdfName } from "./primitives.js";
import type { PdfName } from "./primitives.js";

export type PdfToken =
  | { type: "number"; value: number; raw: string; start: number; end: number }
  | { type: "name"; value: PdfName; raw: string; start: number; end: number }
  | { type: "string"; value: string; bytes: Uint8Array; raw: string; start: number; end: number }
  | { type: "hexString"; value: string; bytes: Uint8Array; raw: string; start: number; end: number }
  | { type: "keyword"; value: string; raw: string; start: number; end: number }
  | { type: "arrayStart" | "arrayEnd" | "dictStart" | "dictEnd"; raw: string; start: number; end: number };

const NUMBER_RE = /^[-+]?(?:(?:\d+\.\d*)|(?:\.\d+)|(?:\d+))$/;

export function isPdfWhitespace(char: string | undefined): boolean {
  return char == null || char === "\x00" || char === "\t" || char === "\n" || char === "\f" || char === "\r" || char === " ";
}

export function isPdfDelimiter(char: string | undefined): boolean {
  return char != null && "()<>[]{}/%".includes(char);
}

function isRegularChar(char: string | undefined): boolean {
  return char != null && !isPdfWhitespace(char) && !isPdfDelimiter(char);
}

function bytesFromHex(hex: string): Uint8Array {
  const clean = hex.replace(/\s+/g, "");
  const padded = clean.length % 2 ? `${clean}0` : clean;
  const bytes = new Uint8Array(padded.length / 2);
  for (let i = 0; i < bytes.length; i += 1) bytes[i] = Number.parseInt(padded.slice(i * 2, i * 2 + 2), 16);
  return bytes;
}

function readBare(source: string, index: number): { raw: string; end: number } {
  const start = index;
  while (isRegularChar(source[index])) index += 1;
  return { raw: source.slice(start, index), end: index };
}

function readName(source: string, index: number): PdfToken {
  const start = index;
  index += 1;
  const bare = readBare(source, index);
  return { type: "name", value: pdfName(bare.raw), raw: source.slice(start, bare.end), start, end: bare.end };
}

function readLiteralString(source: string, index: number): PdfToken {
  const start = index;
  const out: number[] = [];
  let depth = 1;
  index += 1;
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
        out.push(Number.parseInt(octal, 8) & 0xff);
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
  const bytes = Uint8Array.from(out);
  return { type: "string", value: latin1String(bytes), bytes, raw: source.slice(start, index), start, end: index };
}

function readHexString(source: string, index: number): PdfToken {
  const start = index;
  index += 1;
  let body = "";
  while (index < source.length && source[index] !== ">") {
    body += source[index++];
  }
  if (source[index] === ">") index += 1;
  const bytes = bytesFromHex(body);
  return { type: "hexString", value: latin1String(bytes), bytes, raw: source.slice(start, index), start, end: index };
}

export function tokenizePdf(source: string): PdfToken[] {
  const tokens: PdfToken[] = [];
  let index = 0;
  while (index < source.length) {
    const char = source[index];
    if (isPdfWhitespace(char)) {
      index += 1;
      continue;
    }
    if (char === "%") {
      while (index < source.length && source[index] !== "\n" && source[index] !== "\r") index += 1;
      continue;
    }
    if (char === "[") {
      tokens.push({ type: "arrayStart", raw: char, start: index, end: index + 1 });
      index += 1;
      continue;
    }
    if (char === "]") {
      tokens.push({ type: "arrayEnd", raw: char, start: index, end: index + 1 });
      index += 1;
      continue;
    }
    if (char === "<" && source[index + 1] === "<") {
      tokens.push({ type: "dictStart", raw: "<<", start: index, end: index + 2 });
      index += 2;
      continue;
    }
    if (char === ">" && source[index + 1] === ">") {
      tokens.push({ type: "dictEnd", raw: ">>", start: index, end: index + 2 });
      index += 2;
      continue;
    }
    if (char === "<") {
      const token = readHexString(source, index);
      tokens.push(token);
      index = token.end;
      continue;
    }
    if (char === "(") {
      const token = readLiteralString(source, index);
      tokens.push(token);
      index = token.end;
      continue;
    }
    if (char === "/") {
      const token = readName(source, index);
      tokens.push(token);
      index = token.end;
      continue;
    }

    const bare = readBare(source, index);
    if (!bare.raw) {
      index += 1;
      continue;
    }
    if (NUMBER_RE.test(bare.raw)) tokens.push({ type: "number", value: Number(bare.raw), raw: bare.raw, start: index, end: bare.end });
    else tokens.push({ type: "keyword", value: bare.raw, raw: bare.raw, start: index, end: bare.end });
    index = bare.end;
  }
  return tokens;
}

export function pdfNumbersFromString(source: string): number[] {
  return tokenizePdf(source)
    .filter((token): token is Extract<PdfToken, { type: "number" }> => token.type === "number")
    .map((token) => token.value);
}
