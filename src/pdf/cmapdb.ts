import { gunzipSync, strFromU8 } from "fflate";

export type Code2CidMap = Map<number, number | Code2CidMap>;

export interface PdfminerCMapJson {
  IS_VERTICAL?: boolean;
  CODE2CID?: unknown;
  CID2UNICHR_H?: Record<string, string>;
  CID2UNICHR_V?: Record<string, string>;
}

function numericEntries(value: Record<string, unknown>): Array<[number, unknown]> {
  return Object.entries(value).map(([key, item]) => [Number(key), item]);
}

export function convertCode2CidKeysLikePdfminer(value: unknown): number | Code2CidMap {
  if (typeof value === "number") return value;
  const out: Code2CidMap = new Map();
  if (!value || typeof value !== "object" || Array.isArray(value)) return out;
  for (const [key, item] of numericEntries(value as Record<string, unknown>)) {
    if (!Number.isFinite(key)) continue;
    out.set(key, typeof item === "number" ? item : convertCode2CidKeysLikePdfminer(item));
  }
  return out;
}

export function convertCidToUnicodeKeysLikePdfminer(value: Record<string, string> | undefined): Map<number, string> {
  const out = new Map<number, string>();
  if (!value) return out;
  for (const [key, item] of numericEntries(value)) {
    if (Number.isFinite(key) && typeof item === "string") out.set(key, item);
  }
  return out;
}

export function parseCompressedCMapJsonLikePdfminer(bytes: Uint8Array): PdfminerCMapJson {
  return JSON.parse(strFromU8(gunzipSync(bytes))) as PdfminerCMapJson;
}

export class CMapLikePdfminer {
  readonly attrs: Map<string, unknown>;
  readonly code2cid: Code2CidMap;
  readonly vertical: boolean;

  constructor(readonly name: string, data: PdfminerCMapJson) {
    this.attrs = new Map([["CMapName", name]]);
    const converted = convertCode2CidKeysLikePdfminer(data.CODE2CID ?? {});
    this.code2cid = converted instanceof Map ? converted : new Map();
    this.vertical = data.IS_VERTICAL === true;
  }

  toString(): string {
    return `<CMap: ${this.name}>`;
  }

  decode(bytes: Uint8Array | number[]): number[] {
    const out: number[] = [];
    let node = this.code2cid;
    for (const byte of Array.from(bytes)) {
      const value = node.get(byte);
      if (typeof value === "number") {
        out.push(value);
        node = this.code2cid;
      } else if (value instanceof Map) {
        node = value;
      } else {
        node = this.code2cid;
      }
    }
    return out;
  }
}

export class UnicodeMapLikePdfminer {
  readonly cid2unichr: Map<number, string>;
  readonly vertical: boolean;

  constructor(readonly name: string, data: PdfminerCMapJson, vertical = false) {
    this.cid2unichr = convertCidToUnicodeKeysLikePdfminer(vertical ? data.CID2UNICHR_V : data.CID2UNICHR_H);
    this.vertical = vertical;
  }

  toString(): string {
    return `<UnicodeMap: ${this.name}>`;
  }

  getUnicode(cid: number): string | undefined {
    return this.cid2unichr.get(cid);
  }
}

const ADOBE_JAPAN1_KANA_ROWS: Record<number, string> = {
  0x24: "ぁあぃいぅうぇえぉおかがきぎくぐけげこごさざしじすずせぜそぞただちぢっつづてでとどなにぬねのはばぱひびぴふぶぷへべぺほぼぽまみむめもゃやゅゆょよらりるれろゎわゐゑをん",
  0x25: "ァアィイゥウェエォオカガキギクグケゲコゴサザシジスズセゼソゾタダチヂッツヅテデトドナニヌネノハバパヒビピフブプヘベペホボポマミムメモャヤュユョヨラリルレロヮワヰヱヲンヴヵヶ"
};

export function decodePredefinedCMapUnicodeLikePdfminer(cmapName: string | undefined, cidCoding: string | undefined, bytes: Uint8Array | number[]): string[] {
  if (cmapName !== "V" || cidCoding !== "Adobe-Japan1") return [];
  const values = Array.from(bytes);
  const out: string[] = [];
  for (let index = 0; index + 1 < values.length; index += 2) {
    const lead = values[index];
    const trail = values[index + 1];
    const row = ADOBE_JAPAN1_KANA_ROWS[lead];
    const offset = trail - 0x21;
    const text = row == null || offset < 0 ? undefined : Array.from(row)[offset];
    if (text != null) out.push(text);
  }
  return out;
}
