const PDF_DOC_ENCODING_OVERRIDES: Record<number, string> = {
  0x16: "\u0017",
  0x18: "\u02d8",
  0x19: "\u02c7",
  0x1a: "\u02c6",
  0x1b: "\u02d9",
  0x1c: "\u02dd",
  0x1d: "\u02db",
  0x1e: "\u02da",
  0x1f: "\u02dc",
  0x7f: "\u0000",
  0x80: "\u2022",
  0x81: "\u2020",
  0x82: "\u2021",
  0x83: "\u2026",
  0x84: "\u2014",
  0x85: "\u2013",
  0x86: "\u0192",
  0x87: "\u2044",
  0x88: "\u2039",
  0x89: "\u203a",
  0x8a: "\u2212",
  0x8b: "\u2030",
  0x8c: "\u201e",
  0x8d: "\u201c",
  0x8e: "\u201d",
  0x8f: "\u2018",
  0x90: "\u2019",
  0x91: "\u201a",
  0x92: "\u2122",
  0x93: "\ufb01",
  0x94: "\ufb02",
  0x95: "\u0141",
  0x96: "\u0152",
  0x97: "\u0160",
  0x98: "\u0178",
  0x99: "\u017d",
  0x9a: "\u0131",
  0x9b: "\u0142",
  0x9c: "\u0153",
  0x9d: "\u0161",
  0x9e: "\u017e",
  0x9f: "\u0000",
  0xa0: "\u20ac",
  0xad: "\u0000"
};

function hexBytes(value: string): Uint8Array {
  const clean = value.replace(/\s+/g, "");
  const padded = clean.length % 2 ? `${clean}0` : clean;
  const bytes = new Uint8Array(padded.length / 2);
  for (let i = 0; i < bytes.length; i += 1) bytes[i] = Number.parseInt(padded.slice(i * 2, i * 2 + 2), 16);
  return bytes;
}

export function parsePdfLiteralStringBytes(source: string, start: number): { bytes: Uint8Array; end: number } | null {
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
  return depth === 0 ? { bytes: Uint8Array.from(out), end: index } : null;
}

export function parsePdfDictBytes(source: string, key: string): Uint8Array | null {
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

export function decodePdfStringLikePdfminer(bytes: Uint8Array | number[]): string {
  if (bytes[0] === 0xfe && bytes[1] === 0xff) {
    let out = "";
    for (let i = 2; i < bytes.length - 1; i += 2) out += String.fromCharCode((bytes[i] << 8) | bytes[i + 1]);
    return out;
  }
  return Array.from(bytes, (byte) => PDF_DOC_ENCODING_OVERRIDES[byte] ?? String.fromCharCode(byte)).join("");
}

export function decodePdfLiteralBytesAsUtf8ThenUtf16(bytes: Uint8Array | number[]): string | null {
  const buffer = Uint8Array.from(bytes);
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(buffer);
  } catch {
    try {
      if (buffer[0] === 0xfe && buffer[1] === 0xff) return new TextDecoder("utf-16be", { fatal: true }).decode(buffer);
      if (buffer[0] === 0xff && buffer[1] === 0xfe) return new TextDecoder("utf-16le", { fatal: true }).decode(buffer);
      return new TextDecoder("utf-16le", { fatal: true }).decode(buffer);
    } catch {
      return null;
    }
  }
}
