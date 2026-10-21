// Typed JSON (oracle/typed_json.py, docs/verification.md) to JavaScript
// values, and the equality the tests use. Runtime-agnostic.
//
// As an output (what the core returns): ints are numbers (BigInt beyond
// ±2^53), floats numbers, tuples and lists arrays, dicts objects. As an
// input (what the tests send to the core): floats become PyFloat and
// tuples PyTuple, so that the core receives the Python types the oracle
// used.

const hexBytes = (hex) => {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(hex.substr(2 * i, 2), 16);
  return out;
};

function floatOf(hex) {
  const dv = new DataView(new ArrayBuffer(8));
  dv.setBigUint64(0, BigInt(`0x${hex}`));
  return dv.getFloat64(0);
}

function setOwn(obj, key, value) {
  Object.defineProperty(obj, key, { value, writable: true, enumerable: true, configurable: true });
}

/** Decodes typed JSON. `lib` gives the classes (PyFloat, PyTuple,
 * PSLiteral...); `input` selects the input mapping. */
export function fromTyped(j, lib, input = false) {
  const go = (v) => fromTyped(v, lib, input);
  if (v_is_plain(j)) return j;
  if (Array.isArray(j)) return j.map(go);
  const keys = Object.keys(j);
  if (keys.length === 1) {
    const [k] = keys;
    const x = j[k];
    switch (k) {
      case "s":
        return String.fromCodePoint(...x);
      case "i": {
        const b = BigInt(x);
        return b >= -(2n ** 53n) && b <= 2n ** 53n ? Number(b) : b;
      }
      case "f":
        return input ? new lib.PyFloat(floatOf(x)) : floatOf(x);
      case "b":
        return hexBytes(x);
      case "t":
        return input ? new lib.PyTuple(x.map(go)) : x.map(go);
      case "d": {
        const o = {};
        for (const [kk, vv] of x) setOwn(o, go(kk), go(vv));
        return o;
      }
      case "lit":
        return new lib.PSLiteral(go(x));
      case "kw":
        return new lib.PSKeyword(hexBytes(x));
      case "stream":
        return new lib.PDFStream(go(x.objid), go(x.genno), go(x.attrs), go(x.rawdata), go(x.data));
      case "ref":
        return new lib.PDFObjRef(typeof x === "number" ? x : go(x));
    }
  }
  if (keys.length === 2 && "exc" in j) return { exception: j.exc, message: j.msg };
  // a pdfplumber Page, as oracle/unit/js_pages_run.py writes it: its handle
  // label ("#k") and page_number
  if (keys.length === 2 && j.named === "Page") {
    const fields = Object.fromEntries(j.d.map(([k, v]) => [go(k), go(v)]));
    return new lib.PageRef(fields.handle, fields.page_number);
  }
  throw new Error(`unknown typed JSON ${JSON.stringify(j).slice(0, 80)}`);
}

function v_is_plain(j) {
  return j === null || typeof j !== "object";
}

/** Deep equality: numbers by Object.is (NaN equals NaN, -0 is not 0),
 * key order of objects included, typed arrays by content, class instances
 * by class and fields. Returns a path to the first difference, or null. */
export function difference(a, b, path = "$") {
  const stack = [[a, b, path]];
  while (stack.length) {
    const [x, y, p] = stack.pop();
    if (typeof x === "number" && typeof y === "number") {
      if (!Object.is(x, y)) return `${p}: ${x} != ${y}`;
      continue;
    }
    if (x === y) continue;
    if (typeof x !== typeof y || x === null || y === null || typeof x !== "object")
      return `${p}: ${String(x).slice(0, 60)} != ${String(y).slice(0, 60)}`;
    if (x.constructor !== y.constructor) return `${p}: ${x.constructor?.name} != ${y.constructor?.name}`;
    if (x instanceof Uint8Array) {
      if (x.length !== y.length || x.some((v, i) => v !== y[i])) return `${p}: bytes differ`;
      continue;
    }
    const kx = Object.keys(x);
    const ky = Object.keys(y);
    if (kx.length !== ky.length || kx.some((k, i) => k !== ky[i]))
      return `${p}: keys [${kx.join(",")}] != [${ky.join(",")}]`;
    for (const k of kx) stack.push([x[k], y[k], `${p}.${k}`]);
  }
  return null;
}
