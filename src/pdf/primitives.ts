export interface PdfName {
  kind: "name";
  name: string;
  raw: string;
}

export interface PdfRef {
  kind: "ref";
  objectNumber: number;
  generation: number;
}

export type PdfArray = PdfPrimitive[];
export type PdfDict = Map<string, PdfPrimitive>;

export interface PdfStream {
  kind: "stream";
  dict: PdfDict;
  data: Uint8Array;
  rawData: string;
}

export type PdfPrimitive = null | boolean | number | string | PdfName | PdfRef | PdfArray | PdfDict | PdfStream;

export interface PdfIndirectObject {
  objectNumber: number;
  generation: number;
  value: PdfPrimitive;
  stream?: PdfStream;
  raw: string;
}

export interface PdfOperation {
  operator: string;
  args: PdfPrimitive[];
}

export function pdfName(raw: string): PdfName {
  return { kind: "name", raw, name: decodePdfName(raw) };
}

export function pdfRef(objectNumber: number, generation: number): PdfRef {
  return { kind: "ref", objectNumber, generation };
}

export function isName(value: PdfPrimitive | undefined): value is PdfName {
  return typeof value === "object" && value !== null && !Array.isArray(value) && !(value instanceof Map) && (value as PdfName).kind === "name";
}

export function isRef(value: PdfPrimitive | undefined): value is PdfRef {
  return typeof value === "object" && value !== null && !Array.isArray(value) && !(value instanceof Map) && (value as PdfRef).kind === "ref";
}

export function isDict(value: PdfPrimitive | undefined): value is PdfDict {
  return value instanceof Map;
}

export function isArray(value: PdfPrimitive | undefined): value is PdfArray {
  return Array.isArray(value);
}

export function isStream(value: PdfPrimitive | undefined): value is PdfStream {
  return typeof value === "object" && value !== null && !Array.isArray(value) && !(value instanceof Map) && (value as PdfStream).kind === "stream";
}

export function asNumber(value: PdfPrimitive | undefined): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

export function asName(value: PdfPrimitive | undefined): string | undefined {
  return isName(value) ? value.name : undefined;
}

export function asArray(value: PdfPrimitive | undefined): PdfArray | undefined {
  return isArray(value) ? value : undefined;
}

export function asDict(value: PdfPrimitive | undefined): PdfDict | undefined {
  return isDict(value) ? value : undefined;
}

export function decodePdfName(value: string): string {
  return value.replace(/#([0-9A-Fa-f]{2})/g, (_match, hex: string) => String.fromCharCode(Number.parseInt(hex, 16)));
}

export function latin1String(bytes: Uint8Array): string {
  let out = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) out += String.fromCharCode(...bytes.subarray(i, i + chunk));
  return out;
}

export function latin1Bytes(value: string): Uint8Array {
  const bytes = new Uint8Array(value.length);
  for (let i = 0; i < value.length; i += 1) bytes[i] = value.charCodeAt(i) & 0xff;
  return bytes;
}
