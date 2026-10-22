// Typed JSON (oracle/typed_json.py; docs/verification.md, "Format
// d'échange typé") to typed values (js/src/core/typed.ts): an int is a
// number (a bigint beyond ±2^53), a float a PyFloat, a tuple a PyTuple, a
// dict a PyDict, an exception `{ exception, message }`, Named a canon.mjs
// `Named`. `lib` gives the classes (the package and pdfplumber-js/internal).
import { Named } from "./canon.mjs";

const hexBytes = (hex) => Uint8Array.from(Buffer.from(hex, "hex"));

function floatOf(hex) {
  const dv = new DataView(new ArrayBuffer(8));
  dv.setBigUint64(0, BigInt(`0x${hex}`));
  return dv.getFloat64(0);
}

/** An exception of typed JSON, written by canon as the exception. */
export class ExcValue extends Error {
  constructor(name, message) {
    super(message);
    this.name = name;
  }
}

/** Decodes typed JSON (iterative over lists; dict and tuple items recurse
 * once per level, which the tables and canon cases keep shallow). */
export function fromTypedJson(j, lib) {
  const go = (v) => fromTypedJson(v, lib);
  if (j === null || typeof j !== "object") return j;
  if (Array.isArray(j)) return j.map(go);
  const keys = Object.keys(j);
  if (keys.length === 2 && "exc" in j) return new ExcValue(j.exc, go(j.msg));
  if (keys.length === 2 && "named" in j) return new Named(j.named, j.d.map(([k, v]) => [go(k), go(v)]));
  const [k] = keys;
  const x = j[k];
  switch (k) {
    case "s":
      return String.fromCharCode(...x.flatMap((c) => (c > 0xffff ? [0xd800 + ((c - 0x10000) >> 10), 0xdc00 + ((c - 0x10000) & 0x3ff)] : [c])));
    case "i": {
      const b = BigInt(x);
      return b >= -(2n ** 53n) && b <= 2n ** 53n ? Number(b) : b;
    }
    case "f":
      return new lib.PyFloat(floatOf(x));
    case "b":
      return hexBytes(x);
    case "t":
      return new lib.PyTuple(x.map(go));
    case "d":
      return new lib.PyDict(x.map(([kk, vv]) => [go(kk), go(vv)]));
    case "lit":
      return new lib.PSLiteral(go(x));
    case "kw":
      return new lib.PSKeyword(hexBytes(x));
    case "ref":
      return new lib.PDFObjRef(typeof x === "number" ? x : go(x));
    case "stream":
      return new lib.PDFStream(go(x.objid), go(x.genno), go(x.attrs), go(x.rawdata), go(x.data));
  }
  throw new Error(`unknown typed JSON ${JSON.stringify(j).slice(0, 80)}`);
}
