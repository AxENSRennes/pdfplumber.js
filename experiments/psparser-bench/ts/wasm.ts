// Loader for the Rust/wasm parser and decoder for its binary tape.
// Decoded values are the same objects the TS port produces (interned through the
// same LIT/KWD tables), so both paths can be checked with one serializer.
import { InlineStream, KWD, LIT, PSLiteral, type PSDict, type PSKeyword, type PSValue } from "./psparser.ts";

const ERRORS = ["eof", "error:AssertionError", "error:PSSyntaxError", "error:IndexError", "error:TypeError", "error:KeyError"];

interface Exports {
  memory: WebAssembly.Memory;
  input_buffer(len: number): number;
  tape_ptr(): number;
  tape_len(): number;
  parse_tape(): number;
  parse_count(): number;
}

const utf8 = new TextDecoder("utf-8");
const bytesLiterals = new Map<string, PSLiteral>();

export async function loadWasmParser(wasmBytes: BufferSource) {
  const { instance } = await WebAssembly.instantiate(wasmBytes, {});
  const ex = instance.exports as unknown as Exports;
  const lits: PSLiteral[] = [];
  const kws: PSKeyword[] = [];

  /** Copy one job (u32 nstreams, then u32 len + bytes per stream) into wasm memory. */
  function writeJob(streams: Uint8Array[]): void {
    let size = 4;
    for (const s of streams) size += 4 + s.length;
    const ptr = ex.input_buffer(size);
    const mem = new Uint8Array(ex.memory.buffer, ptr, size);
    const view = new DataView(ex.memory.buffer, ptr, size);
    view.setUint32(0, streams.length, true);
    let off = 4;
    for (const s of streams) {
      view.setUint32(off, s.length, true);
      mem.set(s, off + 4);
      off += 4 + s.length;
    }
  }

  /** Decode the tape into [pos, value] pairs. */
  function decodeTape(onItem: (pos: number, obj: PSValue) => void): void {
    const ptr = ex.tape_ptr();
    const len = ex.tape_len();
    const buf = new Uint8Array(ex.memory.buffer, ptr, len);
    const view = new DataView(ex.memory.buffer, ptr, len);
    let off = 0;
    const u32 = () => {
      const v = view.getUint32(off, true);
      off += 4;
      return v;
    };
    const bytes = () => {
      const n = u32();
      const b = buf.slice(off, off + n);
      off += n;
      return b;
    };
    const key = () => {
      const n = u32();
      let ascii = true;
      for (let i = off; i < off + n; i += 1) if (buf[i] >= 0x80) ascii = false;
      let s = "";
      if (ascii) for (let i = off; i < off + n; i += 1) s += String.fromCharCode(buf[i]);
      else s = utf8.decode(buf.subarray(off, off + n));
      off += n;
      return s;
    };
    const dict = (): PSDict => {
      const n = u32();
      const d: PSDict = new Map();
      for (let i = 0; i < n; i += 1) {
        const k = key();
        d.set(k, value());
      }
      return d;
    };
    const value = (): PSValue => {
      const tag = buf[off++];
      switch (tag) {
        case 0: {
          const v = view.getFloat64(off, true);
          off += 8;
          return v;
        }
        case 1: {
          const s = utf8.decode(bytes());
          const big = BigInt(s);
          return big >= BigInt(Number.MIN_SAFE_INTEGER) && big <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(big) : big;
        }
        case 2:
          return true;
        case 3:
          return false;
        case 4:
          return bytes();
        case 5:
          return lits[u32()];
        case 6: {
          const id = u32();
          const isStr = buf[off++] === 1;
          const name = bytes();
          let lit: PSLiteral;
          if (isStr) lit = LIT(utf8.decode(name));
          else {
            const k = String.fromCharCode(...name);
            lit = bytesLiterals.get(k) ?? new PSLiteral(name);
            bytesLiterals.set(k, lit);
          }
          lits[id] = lit;
          return lit;
        }
        case 7:
          return kws[u32()];
        case 8: {
          const id = u32();
          const kw = KWD(bytes());
          kws[id] = kw;
          return kw;
        }
        case 9: {
          const n = u32();
          const items: PSValue[] = new Array(n);
          for (let i = 0; i < n; i += 1) items[i] = value();
          return items;
        }
        case 10:
          return dict();
        case 11: {
          const attrs = dict();
          return new InlineStream(attrs, bytes());
        }
        default:
          throw new Error(`bad tape tag ${tag} at ${off - 1}`);
      }
    };
    while (off < len) {
      const pos = u32();
      onItem(pos, value());
    }
  }

  return {
    wasmBytes: (wasmBytes as ArrayBuffer).byteLength,
    /** Full path: copy in, parse in Rust, decode every object into JS. */
    parseJobDecoded(streams: Uint8Array[]): { error: string; items: Array<[number, PSValue]> } {
      writeJob(streams);
      const code = ex.parse_tape();
      const items: Array<[number, PSValue]> = [];
      decodeTape((pos, obj) => items.push([pos, obj]));
      return { error: ERRORS[code], items };
    },
    /** Same, but only counts decoded objects (what a consumer pays to get JS objects). */
    parseJobDecodedCount(streams: Uint8Array[], consume: (obj: PSValue) => void): number {
      writeJob(streams);
      ex.parse_tape();
      let n = 0;
      decodeTape((_pos, obj) => {
        consume(obj);
        n += 1;
      });
      return n;
    },
    /** Lower bound: copy in and parse in Rust, nothing comes back but a count. */
    parseJobCountOnly(streams: Uint8Array[]): number {
      writeJob(streams);
      return ex.parse_count() & 0x0fffffff;
    }
  };
}
