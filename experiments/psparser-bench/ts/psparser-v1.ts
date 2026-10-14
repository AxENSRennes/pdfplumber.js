// Faithful port of pdfminer.six psparser.py (PSBaseParser + PSStackParser) and
// pdfinterp.py PDFContentParser, optimized for V8 without changing behavior.
//
// Behavior that depends on pdfminer's 4096-byte read chunks (escapes, `\r\n`
// detection, token positions) is preserved by keeping the same chunked buffer
// model; only the per-byte work (regex scans, bytes concatenation) is replaced
// by table-driven loops over Uint8Array.

export class PSLiteral {
  constructor(readonly name: string | Uint8Array) {}
}

export class PSKeyword {
  constructor(
    readonly name: Uint8Array,
    readonly key: string
  ) {}
}

/** Inline image produced by `BI ... ID ... EI` (pdfminer: PDFStream(d, data)). */
export class InlineStream {
  constructor(
    readonly attrs: PSDict,
    readonly rawdata: Uint8Array
  ) {}
}

export type PSDict = Map<string, PSValue>;
export type PSValue = number | bigint | boolean | Uint8Array | PSLiteral | PSKeyword | PSValue[] | PSDict | InlineStream;

/** Python exception raised by pdfminer that escapes the parser (name kept for parity). */
export class PyError extends Error {
  constructor(readonly pyName: string, message = "") {
    super(`${pyName}: ${message}`);
  }
}
export class PSEOF extends PyError {
  constructor() {
    super("PSEOF", "Unexpected EOF");
  }
}

// ---------------------------------------------------------------------------
// Interning (PSLiteralTable / PSKeywordTable)

const literalsByStr = new Map<string, PSLiteral>();
const literalsByBytes = new Map<string, PSLiteral>();
const keywords = new Map<string, PSKeyword>();

function latin1(bytes: Uint8Array, start: number, end: number): string {
  let s = "";
  for (let i = start; i < end; i += 1) s += String.fromCharCode(bytes[i]);
  return s;
}

export function KWD(name: Uint8Array): PSKeyword {
  const key = latin1(name, 0, name.length);
  let kw = keywords.get(key);
  if (!kw) {
    kw = new PSKeyword(name.slice(), key);
    keywords.set(key, kw);
  }
  return kw;
}

function kwdFrom(bytes: Uint8Array, len: number): PSKeyword {
  const key = latin1(bytes, 0, len);
  let kw = keywords.get(key);
  if (!kw) {
    kw = new PSKeyword(bytes.slice(0, len), key);
    keywords.set(key, kw);
  }
  return kw;
}

export function LIT(name: string): PSLiteral {
  let lit = literalsByStr.get(name);
  if (!lit) {
    lit = new PSLiteral(name);
    literalsByStr.set(name, lit);
  }
  return lit;
}

const utf8 = new TextDecoder("utf-8", { fatal: true });

/** `str(curtoken, "utf-8")`, falling back to the raw bytes like pdfminer. */
function litFrom(bytes: Uint8Array, len: number): PSLiteral {
  let ascii = true;
  for (let i = 0; i < len; i += 1) {
    if (bytes[i] >= 0x80) {
      ascii = false;
      break;
    }
  }
  if (ascii) return LIT(latin1(bytes, 0, len));
  try {
    return LIT(utf8.decode(bytes.subarray(0, len)));
  } catch {
    const key = latin1(bytes, 0, len);
    let lit = literalsByBytes.get(key);
    if (!lit) {
      lit = new PSLiteral(bytes.slice(0, len));
      literalsByBytes.set(key, lit);
    }
    return lit;
  }
}

const ascii = (s: string) => Uint8Array.from(s, (c) => c.charCodeAt(0));
export const KEYWORD_PROC_BEGIN = KWD(ascii("{"));
export const KEYWORD_PROC_END = KWD(ascii("}"));
export const KEYWORD_ARRAY_BEGIN = KWD(ascii("["));
export const KEYWORD_ARRAY_END = KWD(ascii("]"));
export const KEYWORD_DICT_BEGIN = KWD(ascii("<<"));
export const KEYWORD_DICT_END = KWD(ascii(">>"));
const KEYWORD_BI = KWD(ascii("BI"));
const KEYWORD_ID = KWD(ascii("ID"));
const KEYWORD_EI = KWD(ascii("EI"));
const LITERALS_ASCII85_DECODE = [LIT("ASCII85Decode"), LIT("A85")];

// ---------------------------------------------------------------------------
// Python str()/repr() for dictionary keys that are not literals
// (literal_name() falls back to str(x) when settings.STRICT is off).

function pyBytesRepr(b: Uint8Array): string {
  let hasSingle = false;
  let hasDouble = false;
  for (const c of b) {
    if (c === 0x27) hasSingle = true;
    else if (c === 0x22) hasDouble = true;
  }
  const quote = hasSingle && !hasDouble ? '"' : "'";
  let s = "b" + quote;
  for (const c of b) {
    if (c === 0x5c) s += "\\\\";
    else if (c === 0x09) s += "\\t";
    else if (c === 0x0a) s += "\\n";
    else if (c === 0x0d) s += "\\r";
    else if (String.fromCharCode(c) === quote) s += "\\" + quote;
    else if (c < 0x20 || c >= 0x7f) s += "\\x" + c.toString(16).padStart(2, "0");
    else s += String.fromCharCode(c);
  }
  return s + quote;
}

function pyStrRepr(v: string): string {
  const quote = v.includes("'") && !v.includes('"') ? '"' : "'";
  let s = quote;
  for (const ch of v) {
    const c = ch.codePointAt(0)!;
    if (ch === "\\") s += "\\\\";
    else if (ch === "\t") s += "\\t";
    else if (ch === "\n") s += "\\n";
    else if (ch === "\r") s += "\\r";
    else if (ch === quote) s += "\\" + quote;
    else if (c < 0x20 || c === 0x7f || (c >= 0x80 && c < 0xa0)) s += "\\x" + c.toString(16).padStart(2, "0");
    else s += ch;
  }
  return s + quote;
}

/** Python float repr (shortest round-trip, exponent outside [1e-4, 1e16)). */
export function pyFloatRepr(x: number): string {
  if (Number.isNaN(x)) return "nan";
  if (!Number.isFinite(x)) return x > 0 ? "inf" : "-inf";
  if (x === 0) return Object.is(x, -0) ? "-0.0" : "0.0";
  const [mant, expStr] = x.toExponential().split("e");
  const exp = Number(expStr);
  const neg = mant.startsWith("-");
  const digits = mant.replace("-", "").replace(".", "");
  let body: string;
  if (exp >= -5 && exp < 16) {
    if (exp >= digits.length - 1) body = digits + "0".repeat(exp - digits.length + 1) + ".0";
    else if (exp >= 0) body = digits.slice(0, exp + 1) + "." + digits.slice(exp + 1);
    else body = "0." + "0".repeat(-exp - 1) + digits;
  } else {
    const m = digits.length > 1 ? digits[0] + "." + digits.slice(1) : digits;
    body = m + "e" + (exp < 0 ? "-" : "+") + String(Math.abs(exp)).padStart(2, "0");
  }
  return (neg ? "-" : "") + body;
}

function pyRepr(v: PSValue, isFloat = false): string {
  if (typeof v === "boolean") return v ? "True" : "False";
  if (typeof v === "bigint") return v.toString();
  if (typeof v === "number") return isFloat || !Number.isInteger(v) ? pyFloatRepr(v) : String(v);
  if (v instanceof Uint8Array) return pyBytesRepr(v);
  if (v instanceof PSLiteral) return "/" + (typeof v.name === "string" ? pyStrRepr(v.name) : pyBytesRepr(v.name));
  if (v instanceof PSKeyword) return "/" + pyBytesRepr(v.name);
  if (Array.isArray(v)) return "[" + v.map((x) => pyRepr(x)).join(", ") + "]";
  if (v instanceof Map) return "{" + [...v].map(([k, x]) => pyStrRepr(k) + ": " + pyRepr(x)).join(", ") + "}";
  return "<PDFStream>";
}

/** pdfminer.psparser.literal_name with settings.STRICT == False. */
function literalName(x: PSValue): string {
  if (x instanceof PSLiteral) return typeof x.name === "string" ? x.name : pyBytesRepr(x.name);
  return pyRepr(x);
}

// ---------------------------------------------------------------------------
// Byte classes (Python bytes regex semantics: \s == [\t\n\v\f\r ]).

const SPACE = 1;
const DIGIT = 2;
const HEXD = 4;
const ALPHA = 8;
const END_LITERAL = 16; // [#/%\[\]()<>{}\s]  (also END_KEYWORD)
const END_STRING = 32; // [()\\]
const EOL = 64; // [\r\n]
const OCT = 128; // [0-7]
const CLS = new Uint8Array(256);
for (const c of [0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x20]) CLS[c] |= SPACE | END_LITERAL;
for (let c = 0x30; c <= 0x39; c += 1) CLS[c] |= DIGIT | HEXD;
for (let c = 0x30; c <= 0x37; c += 1) CLS[c] |= OCT;
for (let c = 0x41; c <= 0x46; c += 1) CLS[c] |= HEXD;
for (let c = 0x61; c <= 0x66; c += 1) CLS[c] |= HEXD;
for (let c = 0x41; c <= 0x5a; c += 1) CLS[c] |= ALPHA;
for (let c = 0x61; c <= 0x7a; c += 1) CLS[c] |= ALPHA;
for (const c of "#/%[]()<>{}") CLS[c.charCodeAt(0)] |= END_LITERAL;
for (const c of "()\\") CLS[c.charCodeAt(0)] |= END_STRING;
CLS[0x0a] |= EOL;
CLS[0x0d] |= EOL;

const ESC_STRING = new Int16Array(256).fill(-1);
ESC_STRING[0x62] = 8; // b
ESC_STRING[0x74] = 9; // t
ESC_STRING[0x6e] = 10; // n
ESC_STRING[0x66] = 12; // f
ESC_STRING[0x72] = 13; // r
ESC_STRING[0x28] = 40; // (
ESC_STRING[0x29] = 41; // )
ESC_STRING[0x5c] = 92; // backslash

const NEWLINE = ascii("\n");
const EMPTY = new Uint8Array(0);
const BUFSIZ = 4096;
const POW10 = [1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16, 1e17, 1e18, 1e19, 1e20, 1e21, 1e22];

const enum State {
  Main,
  Comment,
  Literal,
  LiteralHex,
  Number,
  Float,
  Keyword,
  String,
  String1,
  WOpen,
  WClose,
  HexString
}

type Token = number | bigint | boolean | Uint8Array | PSLiteral | PSKeyword;

/** pdfminer.pdfinterp.PDFContentParser (with its PSStackParser/PSBaseParser bases). */
export class PDFContentParser {
  private istream = 0;
  private fp: Uint8Array | null = null;
  private fpPos = 0;
  private buf: Uint8Array = EMPTY;
  private bufpos = 0;
  private charpos = 0;
  private eof = false;

  private state = State.Main;
  private tok = new Uint8Array(256); // _curtoken
  private toklen = 0;
  private curtokenpos = 0;
  private paren = 0;
  private hex = 0; // pending hex digits of a #xx escape in a literal (value)
  private hexlen = 0;
  private oct = 0; // pending octal escape (value)
  private octlen = 0;
  private tokPos: number[] = [];
  private tokVal: Token[] = [];
  private tokHead = 0;

  private context: Array<[number, string | null, number[], PSValue[]]> = [];
  private curtype: string | null = null;
  private stackPos: number[] = [];
  private stackObj: PSValue[] = [];
  private resPos: number[] = [];
  private resObj: PSValue[] = [];
  private resHead = 0;

  /** Position of the object last returned by nextobject(). */
  objpos = 0;

  constructor(private readonly streams: Uint8Array[]) {
    this.seek(0);
    this.reset();
  }

  // -- PDFContentParser stream plumbing ------------------------------------

  private fillfp(): boolean {
    if (!this.fp) {
      if (this.istream < this.streams.length) {
        this.fp = this.streams[this.istream];
        this.istream += 1;
        this.fpPos = 0;
      } else {
        throw new PSEOF();
      }
      return true;
    }
    return false;
  }

  private seek(pos: number): void {
    this.fillfp();
    // PSBaseParser.seek
    this.fpPos = pos;
    this.bufpos = pos;
    this.buf = EMPTY;
    this.charpos = 0;
    this.state = State.Main;
    this.toklen = 0;
    this.curtokenpos = 0;
    this.tokPos.length = 0;
    this.tokVal.length = 0;
    this.tokHead = 0;
    this.eof = false;
    // PSStackParser.seek
    this.reset();
  }

  private reset(): void {
    this.context = [];
    this.curtype = null;
    this.stackPos = [];
    this.stackObj = [];
    this.resPos = [];
    this.resObj = [];
    this.resHead = 0;
  }

  private fillbuf(): boolean {
    if (this.charpos < this.buf.length) return false;
    let newStream = false;
    for (;;) {
      newStream = this.fillfp();
      const fp = this.fp!;
      this.bufpos = this.fpPos;
      const end = Math.min(fp.length, this.fpPos + BUFSIZ);
      this.buf = this.fpPos < end ? fp.subarray(this.fpPos, end) : EMPTY;
      this.fpPos = Math.max(this.fpPos, end);
      if (this.buf.length) break;
      this.fp = null;
    }
    this.charpos = 0;
    return newStream;
  }

  // -- tokenizer helpers -----------------------------------------------------

  private append(s: Uint8Array, start: number, end: number): void {
    const n = end - start;
    if (n <= 0) return;
    if (this.toklen + n > this.tok.length) {
      const grown = new Uint8Array(Math.max(this.tok.length * 2, this.toklen + n));
      grown.set(this.tok.subarray(0, this.toklen));
      this.tok = grown;
    }
    if (n < 16) {
      const tok = this.tok;
      let o = this.toklen;
      for (let i = start; i < end; i += 1) tok[o++] = s[i];
    } else {
      this.tok.set(s.subarray(start, end), this.toklen);
    }
    this.toklen += n;
  }

  private appendByte(c: number): void {
    if (this.toklen === this.tok.length) {
      const grown = new Uint8Array(this.tok.length * 2);
      grown.set(this.tok);
      this.tok = grown;
    }
    this.tok[this.toklen++] = c;
  }

  private addToken(obj: Token): void {
    this.tokPos.push(this.curtokenpos);
    this.tokVal.push(obj);
  }

  /** int(self._curtoken); undefined when Python raises ValueError. */
  private parseInt(): number | bigint | undefined {
    const tok = this.tok;
    const len = this.toklen;
    let i = 0;
    let neg = false;
    if (tok[0] === 0x2d || tok[0] === 0x2b) {
      neg = tok[0] === 0x2d;
      i = 1;
    }
    const ndigits = len - i;
    if (ndigits === 0) return undefined;
    if (ndigits <= 15) {
      let v = 0;
      for (; i < len; i += 1) v = v * 10 + (tok[i] - 0x30);
      return neg ? (v === 0 ? 0 : -v) : v;
    }
    if (ndigits > 4300) return undefined; // sys.int_info.default_max_str_digits
    const big = BigInt(latin1(tok, 0, len));
    return big >= BigInt(Number.MIN_SAFE_INTEGER) && big <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(big) : big;
  }

  /** float(self._curtoken); undefined when Python raises ValueError. */
  private parseFloat(): number | undefined {
    const tok = this.tok;
    const len = this.toklen;
    let i = 0;
    let neg = false;
    if (tok[0] === 0x2d || tok[0] === 0x2b) {
      neg = tok[0] === 0x2d;
      i = 1;
    }
    let mant = 0;
    let ndigits = 0;
    let frac = -1;
    for (; i < len; i += 1) {
      const c = tok[i];
      if (c === 0x2e) frac = 0;
      else {
        mant = mant * 10 + (c - 0x30);
        ndigits += 1;
        if (frac >= 0) frac += 1;
      }
    }
    if (ndigits === 0) return undefined;
    let v: number;
    // Clinger fast path: exact mantissa and power of ten give a correctly rounded result.
    if (ndigits <= 15 && frac <= 22) v = frac > 0 ? mant / POW10[frac] : mant;
    else v = Number(latin1(tok, neg || tok[0] === 0x2b ? 1 : 0, len));
    return neg ? -v : v;
  }

  // -- PSBaseParser._parse_* -------------------------------------------------

  private parse1(s: Uint8Array, i: number): number {
    const n = s.length;
    switch (this.state) {
      case State.Main: {
        while (i < n && CLS[s[i]] & SPACE) i += 1;
        if (i >= n) return n;
        const j = i;
        const c = s[j];
        this.curtokenpos = this.bufpos + j;
        if (c === 0x25) {
          this.toklen = 0;
          this.appendByte(c);
          this.state = State.Comment;
        } else if (c === 0x2f) {
          this.toklen = 0;
          this.state = State.Literal;
        } else if (c === 0x2d || c === 0x2b || CLS[c] & DIGIT) {
          this.toklen = 0;
          this.appendByte(c);
          this.state = State.Number;
        } else if (c === 0x2e) {
          this.toklen = 0;
          this.appendByte(c);
          this.state = State.Float;
        } else if (CLS[c] & ALPHA) {
          this.toklen = 0;
          this.appendByte(c);
          this.state = State.Keyword;
        } else if (c === 0x28) {
          this.toklen = 0;
          this.paren = 1;
          this.state = State.String;
        } else if (c === 0x3c) {
          this.toklen = 0;
          this.state = State.WOpen;
        } else if (c === 0x3e) {
          this.toklen = 0;
          this.state = State.WClose;
        } else if (c !== 0x00) {
          this.addToken(kwdFrom(s.subarray(j, j + 1), 1));
        }
        return j + 1;
      }
      case State.Comment: {
        let j = i;
        while (j < n && !(CLS[s[j]] & EOL)) j += 1;
        this.append(s, i, j);
        if (j >= n) return n;
        this.state = State.Main;
        return j;
      }
      case State.Literal: {
        let j = i;
        while (j < n && !(CLS[s[j]] & END_LITERAL)) j += 1;
        this.append(s, i, j);
        if (j >= n) return n;
        if (s[j] === 0x23) {
          this.hex = 0;
          this.hexlen = 0;
          this.state = State.LiteralHex;
          return j + 1;
        }
        this.addToken(litFrom(this.tok, this.toklen));
        this.state = State.Main;
        return j;
      }
      case State.LiteralHex: {
        const c = i < n ? s[i] : -1;
        if (c >= 0 && CLS[c] & HEXD && this.hexlen < 2) {
          this.hex = this.hex * 16 + (c <= 0x39 ? c - 0x30 : (c | 0x20) - 0x57);
          this.hexlen += 1;
          return i + 1;
        }
        if (this.hexlen) this.appendByte(this.hex);
        this.state = State.Literal;
        return i;
      }
      case State.Number: {
        let j = i;
        while (j < n && CLS[s[j]] & DIGIT) j += 1;
        this.append(s, i, j);
        if (j >= n) return n;
        if (s[j] === 0x2e) {
          this.appendByte(0x2e);
          this.state = State.Float;
          return j + 1;
        }
        const v = this.parseInt();
        if (v !== undefined) this.addToken(v);
        this.state = State.Main;
        return j;
      }
      case State.Float: {
        let j = i;
        while (j < n && CLS[s[j]] & DIGIT) j += 1;
        this.append(s, i, j);
        if (j >= n) return n;
        const v = this.parseFloat();
        if (v !== undefined) this.addToken(v);
        this.state = State.Main;
        return j;
      }
      case State.Keyword: {
        let j = i;
        while (j < n && !(CLS[s[j]] & END_LITERAL)) j += 1;
        this.append(s, i, j);
        if (j >= n) return n;
        const tok = this.tok;
        const len = this.toklen;
        let token: Token;
        if (len === 4 && tok[0] === 0x74 && tok[1] === 0x72 && tok[2] === 0x75 && tok[3] === 0x65) token = true;
        else if (len === 5 && tok[0] === 0x66 && tok[1] === 0x61 && tok[2] === 0x6c && tok[3] === 0x73 && tok[4] === 0x65) token = false;
        else token = kwdFrom(tok, len);
        this.addToken(token);
        this.state = State.Main;
        return j;
      }
      case State.String: {
        let j = i;
        while (j < n && !(CLS[s[j]] & END_STRING)) j += 1;
        this.append(s, i, j);
        if (j >= n) return n;
        const c = s[j];
        if (c === 0x5c) {
          this.oct = 0;
          this.octlen = 0;
          this.state = State.String1;
          return j + 1;
        }
        if (c === 0x28) {
          this.paren += 1;
          this.appendByte(c);
          return j + 1;
        }
        // c == ")"
        this.paren -= 1;
        if (this.paren) {
          this.appendByte(c);
          return j + 1;
        }
        this.addToken(this.tok.slice(0, this.toklen));
        this.state = State.Main;
        return j + 1;
      }
      case State.String1: {
        const c = i < n ? s[i] : -1;
        if (c >= 0 && CLS[c] & OCT && this.octlen < 3) {
          this.oct = this.oct * 8 + (c - 0x30);
          this.octlen += 1;
          return i + 1;
        } else if (this.octlen) {
          if (this.oct >= 256) throw new PyError("AssertionError", `Invalid octal (${this.oct})`);
          this.appendByte(this.oct);
          this.state = State.String;
          return i;
        } else if (c >= 0 && ESC_STRING[c] >= 0) {
          this.appendByte(ESC_STRING[c]);
        } else if (c === 0x0d && n > i + 1 && s[i + 1] === 0x0a) {
          i += 1;
        }
        this.state = State.String;
        return i + 1;
      }
      case State.WOpen: {
        if (i < n && s[i] === 0x3c) {
          this.addToken(KEYWORD_DICT_BEGIN);
          this.state = State.Main;
          return i + 1;
        }
        this.state = State.HexString;
        return i;
      }
      case State.WClose: {
        if (i < n && s[i] === 0x3e) {
          this.addToken(KEYWORD_DICT_END);
          i += 1;
        }
        this.state = State.Main;
        return i;
      }
      case State.HexString: {
        let j = i;
        while (j < n && CLS[s[j]] & (SPACE | HEXD)) j += 1;
        this.append(s, i, j);
        if (j >= n) return n;
        // HEX_PAIR.sub(..., SPC.sub(b"", curtoken)): pairs, then a lone trailing digit.
        const tok = this.tok;
        const out = new Uint8Array((this.toklen + 1) >> 1);
        let o = 0;
        let pending = -1;
        for (let k = 0; k < this.toklen; k += 1) {
          const c = tok[k];
          if (CLS[c] & SPACE) continue;
          const v = c <= 0x39 ? c - 0x30 : (c | 0x20) - 0x57;
          if (pending < 0) pending = v;
          else {
            out[o++] = (pending << 4) | v;
            pending = -1;
          }
        }
        if (pending >= 0) out[o++] = pending;
        this.addToken(o === out.length ? out : out.slice(0, o));
        this.state = State.Main;
        return j;
      }
    }
  }

  private nexttoken(): void {
    if (this.eof) throw new PSEOF();
    while (this.tokHead >= this.tokVal.length) {
      try {
        const changedStream = this.fillbuf();
        if (changedStream && this.toklen) this.parse1(NEWLINE, 0);
        else this.charpos = this.parse1(this.buf, this.charpos);
      } catch (e) {
        if (!(e instanceof PSEOF)) throw e;
        this.charpos = this.parse1(NEWLINE, 0);
        this.eof = true;
        if (this.tokHead >= this.tokVal.length) throw e;
      }
    }
  }

  // -- PSStackParser ---------------------------------------------------------

  private push(pos: number, obj: PSValue): void {
    this.stackPos.push(pos);
    this.stackObj.push(obj);
  }

  private startType(pos: number, type: string): void {
    this.context.push([pos, this.curtype, this.stackPos, this.stackObj]);
    this.curtype = type;
    this.stackPos = [];
    this.stackObj = [];
  }

  /** end_type(); returns null where pdfminer raises PSTypeError (swallowed when not STRICT). */
  private endType(type: string): [number, PSValue[]] | null {
    if (this.curtype !== type) return null;
    const objs = this.stackObj;
    const [pos, curtype, stackPos, stackObj] = this.context.pop()!;
    this.curtype = curtype;
    this.stackPos = stackPos;
    this.stackObj = stackObj;
    return [pos, objs];
  }

  private flush(): void {
    const n = this.stackObj.length;
    for (let i = 0; i < n; i += 1) {
      this.resPos.push(this.stackPos[i]);
      this.resObj.push(this.stackObj[i]);
    }
    this.stackPos = [];
    this.stackObj = [];
  }

  private doKeyword(pos: number, token: PSKeyword): void {
    if (token === KEYWORD_BI) {
      this.startType(pos, "inline");
    } else if (token === KEYWORD_ID) {
      const ended = this.endType("inline");
      if (!ended) return;
      const objs = ended[1];
      if (objs.length % 2 !== 0) return; // PSTypeError, swallowed
      const d: PSDict = new Map();
      for (let k = 0; k < objs.length; k += 2) d.set(literalName(objs[k]), objs[k + 1]);
      let eos = "EI";
      let filter = d.get("F");
      if (filter !== undefined) {
        if (filter instanceof PSLiteral) filter = [filter];
        const first = subscriptZero(filter);
        if (first === LITERALS_ASCII85_DECODE[0] || first === LITERALS_ASCII85_DECODE[1]) eos = "~>";
      }
      const [dataPos, data] = this.getInlineData(pos + 3, eos);
      let payload = data;
      if (eos !== "EI") {
        payload = new Uint8Array(data.length + 2);
        payload.set(data);
        payload[data.length] = 0x7e;
        payload[data.length + 1] = 0x3e;
      }
      this.push(dataPos, new InlineStream(d, payload));
      if (eos === "EI") this.push(dataPos, KEYWORD_EI);
    } else {
      this.push(pos, token);
    }
  }

  private getInlineData(pos: number, targetStr: string): [number, Uint8Array] {
    const target = ascii(targetStr);
    this.seek(pos);
    let i = 0;
    let data = new Uint8Array(1024);
    let dlen = 0;
    const add = (s: Uint8Array, start: number, end: number) => {
      if (dlen + end - start > data.length) {
        const grown = new Uint8Array(Math.max(data.length * 2, dlen + end - start));
        grown.set(data.subarray(0, dlen));
        data = grown;
      }
      data.set(s.subarray(start, end), dlen);
      dlen += end - start;
    };
    while (i <= target.length) {
      this.fillbuf();
      if (i) {
        const c = this.buf[this.charpos];
        add(this.buf, this.charpos, this.charpos + 1);
        this.charpos += 1;
        if ((target.length <= i && CLS[c] & SPACE) || (i < target.length && c === target[i])) i += 1;
        else i = 0;
      } else {
        const j = this.buf.indexOf(target[0], this.charpos);
        if (j >= 0) {
          add(this.buf, this.charpos, j + 1);
          this.charpos = j + 1;
          i = 1;
        } else {
          add(this.buf, this.charpos, this.buf.length);
          this.charpos = this.buf.length;
        }
      }
    }
    let end = Math.max(0, dlen - (target.length + 1));
    end = stripTrailingEol(data, end);
    return [pos, data.slice(0, end)];
  }

  /** PSStackParser.nextobject(); the position is left in `objpos`. */
  nextobject(): PSValue {
    while (this.resHead >= this.resObj.length) {
      this.nexttoken();
      const pos = this.tokPos[this.tokHead];
      const token = this.tokVal[this.tokHead];
      this.tokHead += 1;
      if (this.tokHead === this.tokVal.length) {
        this.tokHead = 0;
        this.tokPos.length = 0;
        this.tokVal.length = 0;
      }
      if (!(token instanceof PSKeyword)) {
        this.push(pos, token);
      } else if (token === KEYWORD_ARRAY_BEGIN) {
        this.startType(pos, "a");
      } else if (token === KEYWORD_ARRAY_END) {
        const ended = this.endType("a");
        if (ended) this.push(ended[0], ended[1]);
      } else if (token === KEYWORD_DICT_BEGIN) {
        this.startType(pos, "d");
      } else if (token === KEYWORD_DICT_END) {
        const ended = this.endType("d");
        if (ended) {
          const objs = ended[1];
          if (objs.length % 2 !== 0) throw new PyError("PSSyntaxError", "Invalid dictionary construct");
          const d: PSDict = new Map();
          for (let k = 0; k < objs.length; k += 2) d.set(literalName(objs[k]), objs[k + 1]);
          this.push(ended[0], d);
        }
      } else if (token === KEYWORD_PROC_BEGIN) {
        this.startType(pos, "p");
      } else if (token === KEYWORD_PROC_END) {
        const ended = this.endType("p");
        if (ended) this.push(ended[0], ended[1]);
      } else {
        this.doKeyword(pos, token);
      }
      if (this.context.length) continue;
      this.flush();
    }
    this.objpos = this.resPos[this.resHead];
    const obj = this.resObj[this.resHead];
    this.resHead += 1;
    if (this.resHead === this.resObj.length) {
      this.resHead = 0;
      this.resPos.length = 0;
      this.resObj.length = 0;
    }
    return obj;
  }
}

/** Python `filter[0]` on whatever value /F holds; raises like Python on bad types. */
function subscriptZero(v: PSValue): PSValue | number {
  if (Array.isArray(v)) {
    if (!v.length) throw new PyError("IndexError", "list index out of range");
    return v[0];
  }
  if (v instanceof Uint8Array) {
    if (!v.length) throw new PyError("IndexError", "index out of range");
    return v[0];
  }
  if (v instanceof Map) throw new PyError("KeyError", "0");
  throw new PyError("TypeError", "object is not subscriptable");
}

/**
 * re.sub(rb"(\x0d\x0a|[\x0d\x0a])$", b"", data[:n]) where `$` also matches before a
 * final \n. A match must end at n or n-1, and a match ending at n-1 is always followed
 * by one ending at n, so the result is data[:first matching position].
 */
function stripTrailingEol(data: Uint8Array, n: number): number {
  const isEnd = (q: number) => q === n || (q === n - 1 && data[n - 1] === 0x0a);
  for (let p = Math.max(0, n - 3); p < n; p += 1) {
    if (data[p] === 0x0d && p + 1 < n && data[p + 1] === 0x0a && isEnd(p + 2)) return p;
    if ((data[p] === 0x0d || data[p] === 0x0a) && isEnd(p + 1)) return p;
  }
  return n;
}
