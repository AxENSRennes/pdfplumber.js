import type { PdfEncryption } from "../types.js";
import { parsePdfDictBytes, parsePdfLiteralStringBytes } from "../pdf-strings.js";
import { latin1Bytes, latin1String } from "./primitives.js";

const PDF_PASSWORD_PADDING = Uint8Array.from([
  0x28, 0xbf, 0x4e, 0x5e, 0x4e, 0x75, 0x8a, 0x41,
  0x64, 0x00, 0x4e, 0x56, 0xff, 0xfa, 0x01, 0x08,
  0x2e, 0x2e, 0x00, 0xb6, 0xd0, 0x68, 0x3e, 0x80,
  0x2f, 0x0c, 0xa9, 0xfe, 0x64, 0x53, 0x69, 0x7a
]);

function hexBytes(value: string): Uint8Array {
  const clean = value.replace(/\s+/g, "");
  const padded = clean.length % 2 ? `${clean}0` : clean;
  const bytes = new Uint8Array(padded.length / 2);
  for (let i = 0; i < bytes.length; i += 1) bytes[i] = Number.parseInt(padded.slice(i * 2, i * 2 + 2), 16);
  return bytes;
}

function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
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
  const k = Array.from({ length: 64 }, (_unused, i) => Math.floor(Math.abs(Math.sin(i + 1)) * 0x100000000) >>> 0);
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

export function rc4BytesLikePdfminer(key: Uint8Array, data: Uint8Array): Uint8Array {
  return rc4Bytes(data, key);
}

export function unpadAesLikePdfminer(data: Uint8Array): Uint8Array {
  const size = data[data.length - 1];
  if (!size || size > 16 || size > data.length) return data;
  for (let i = data.length - size; i < data.length; i += 1) {
    if (data[i] !== size) return data;
  }
  return data.subarray(0, data.length - size);
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
  if (cfm === "AESV2") return "AESV2";
  if (cfm === "AESV3") return "AESV3";
  return "Unsupported";
}

export function parsePdfEncryption(raw: string, objects: Map<number, string>, password = ""): PdfEncryption | null {
  const encryptRef = Number(raw.match(/\/Encrypt\s+(\d+)\s+\d+\s+R/)?.[1] ?? Number.NaN);
  if (!Number.isFinite(encryptRef)) return null;
  const encryptText = objects.get(encryptRef) ?? "";
  if (!/\/Filter\s*\/Standard\b/.test(encryptText)) return null;
  const owner = parsePdfDictBytes(encryptText, "O");
  const permissions = Number(encryptText.match(/\/P\s+(-?\d+)/)?.[1] ?? Number.NaN);
  const revision = Number(encryptText.match(/\/R\s+(\d+)/)?.[1] ?? 0);
  const version = Number(encryptText.match(/\/V\s+(\d+)/)?.[1] ?? 0);
  const lengthValues = Array.from(encryptText.matchAll(/\/Length\s+(\d+)/g), (match) => Number(match[1]));
  const lengthBits = lengthValues.length ? Math.max(...lengthValues) : version === 1 ? 40 : 40;
  const fileId = parseFirstFileId(raw) ?? new Uint8Array();
  if (!owner || !Number.isFinite(permissions) || revision >= 5) return null;
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

function decryptPdfObjectStrings(text: string, key: Uint8Array): string {
  let out = "";
  let index = 0;
  while (index < text.length) {
    const char = text[index];
    if (char === "(") {
      const parsed = parsePdfLiteralStringBytes(text, index);
      if (parsed) {
        out += `<${bytesToHex(rc4Bytes(parsed.bytes, key))}>`;
        index = parsed.end;
        continue;
      }
    }
    if (char === "<" && text[index - 1] !== "<" && text[index + 1] !== "<") {
      const end = text.indexOf(">", index + 1);
      if (end >= 0) {
        out += `<${bytesToHex(rc4Bytes(hexBytes(text.slice(index + 1, end)), key))}>`;
        index = end + 1;
        continue;
      }
    }
    out += char;
    index += 1;
  }
  return out;
}

export function decryptPdfObject(objectText: string, encryption: PdfEncryption | null): string {
  if (!encryption || encryption.streamMethod !== "RC4") return objectText;
  const ref = pdfObjectRefFromText(objectText);
  if (!ref || ref.num === encryption.encryptRef || /\/Type\s*\/XRef\b/.test(objectText)) return objectText;
  const key = pdfObjectKey(encryption, ref.num, ref.gen);
  const streamIndex = objectText.indexOf("stream");
  const endstreamIndex = objectText.lastIndexOf("endstream");
  if (streamIndex === -1 || endstreamIndex === -1 || endstreamIndex <= streamIndex) return decryptPdfObjectStrings(objectText, key);
  let start = streamIndex + "stream".length;
  if (objectText[start] === "\r" && objectText[start + 1] === "\n") start += 2;
  else if (objectText[start] === "\r" || objectText[start] === "\n") start += 1;
  let end = endstreamIndex;
  if (objectText[end - 2] === "\r" && objectText[end - 1] === "\n") end -= 2;
  else if (objectText[end - 1] === "\r" || objectText[end - 1] === "\n") end -= 1;
  const prefix = decryptPdfObjectStrings(objectText.slice(0, start), key);
  if (!encryption.encryptMetadata && /\/Type\s*\/Metadata\b/.test(objectText)) return `${prefix}${objectText.slice(start)}`;
  const decrypted = rc4Bytes(latin1Bytes(objectText.slice(start, end)), key);
  return `${prefix}${latin1String(decrypted)}${objectText.slice(end)}`;
}

export const decryptPdfStreamObject = decryptPdfObject;
