// js/trace/canon.mjs (the trace format written by the harness of the public
// API) against oracle/canon.py: the ~6 300 cases of oracle/unit/canon.py
// (edge values of every type: ints past 2^64, float repr thresholds, NaN,
// -0.0, lone surrogates, bytes of 256/257 bytes, streams, exceptions with
// addresses, Named values), decoded into typed values.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import * as api from "../../src/index.js";
import * as internal from "../../src/internal.js";
import { oracleDir } from "../oracle-data.mjs";
import { canon } from "../../trace/canon.mjs";
import { fromTypedJson } from "../../trace/typed-json.mjs";

const lib = { ...api, ...internal };

/** The lines of a unit file, integers beyond 2^53 as BigInt (typed JSON
 * writes a `PDFObjRef`'s objid as a JSON integer). */
function lines(module: string, name: string): { in: [unknown]; out: string }[] {
  const exact = (_k: string, v: unknown, ctx?: { source?: string }) =>
    typeof v === "number" && Number.isInteger(v) && !Number.isSafeInteger(v) && ctx?.source && /^-?\d+$/.test(ctx.source)
      ? BigInt(ctx.source)
      : v;
  return readFileSync(join(oracleDir(module), `${name}.jsonl`), "utf8")
    .split("\n")
    .filter(Boolean)
    .map((l) => JSON.parse(l, exact as never));
}

describe("the canon of js/trace", () => {
  it("writes every case of oracle/unit/canon.py as Python does", () => {
    let n = 0;
    for (const { in: input, out } of lines("canon", "cases")) {
      const value = fromTypedJson(input[0], lib);
      const where = JSON.stringify(input[0], (_k, v) => (typeof v === "bigint" ? `${v}n` : v)).slice(0, 200);
      expect(canon(value), where).toBe(out);
      n++;
    }
    expect(n).toBeGreaterThan(6000);
  });
});
