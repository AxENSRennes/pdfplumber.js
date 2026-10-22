/**
 * pdfminer objects that can appear in results (an image's `stream`, a
 * colorspace name, the resolved dictionary of an annotation...). Plain
 * Python values map to plain JavaScript values (see docs/js-runtime.md,
 * "Values"); these classes stand for the pdfminer ones.
 */

/** `pdfminer.psparser.PSLiteral`: a PDF name (`/DeviceRGB`). Its name is a
 * `str`, or `bytes` when it was not valid UTF-8. */
export class PSLiteral {
  constructor(readonly name: string | Uint8Array) {}
  toString(): string {
    return typeof this.name === "string" ? `/${this.name}` : `/<bytes ${this.name.length}>`;
  }
}

/** `pdfminer.psparser.PSKeyword`: a PostScript keyword (operator). */
export class PSKeyword {
  constructor(readonly name: Uint8Array) {}
}

/** `pdfminer.pdftypes.PDFObjRef`: an indirect reference (`12 0 R`). */
export class PDFObjRef {
  constructor(readonly objid: number | bigint) {}
}

/** Where a stream by reference reads its data: a request on its handle
 * (`stream_rawdata`, `stream_data`, `stream_get_data`,
 * `stream_get_rawdata`, crates/wasm/src/streams.rs), run as a job of its
 * document (core/runtime.ts `StreamRefs`). */
export interface StreamSource {
  read(op: string, handle: number): Promise<unknown>;
}

/** `pdfminer.pdftypes.PDFStream`: a stream's `objid`, `genno` and
 * dictionary (`attrs`), and its data.
 *
 * A stream in a result of the API is a reference to the stream in the
 * core (docs/public-api.md, "PDFStream"): `rawdata` and `data` are
 * promises that read the stream as it is when they are read
 * (`stream.rawdata`, `stream.data`: `null` once decoded, resp. before),
 * `getData()` is `stream.get_data()` (which decodes it: `rawdata` is then
 * `null`) and `getRawdata()` `stream.get_rawdata()`. The same stream is the
 * same object while it is alive; the reference lives as long as its
 * document (a released document's streams reject with `DocumentReleased`).
 *
 * A stream made from its values (`new PDFStream(objid, genno, attrs,
 * rawdata, data)`, the typed results of the harness, js/trace/) is a
 * snapshot: `rawdata` and `data` are its values themselves, and
 * `getData()` gives `data` when it is known. */
export class PDFStream {
  readonly #rawdata: Uint8Array | null;
  readonly #data: Uint8Array | null;
  #ref: { handle: number; source: StreamSource | null } | undefined;

  constructor(
    readonly objid: number | bigint | null,
    readonly genno: number | bigint | null,
    readonly attrs: Record<string, unknown>,
    rawdata: Uint8Array | null = null,
    data: Uint8Array | null = null,
  ) {
    this.#rawdata = rawdata;
    this.#data = data;
  }

  /** @internal A stream by reference (`source` null: its data cannot be
   * read, a response decoded outside its core). */
  static reference(
    objid: number | bigint | null,
    genno: number | bigint | null,
    attrs: Record<string, unknown>,
    handle: number,
    source: StreamSource | null,
  ): PDFStream {
    const s = new PDFStream(objid, genno, attrs);
    s.#ref = { handle, source };
    return s;
  }

  /** @internal The handle of a stream by reference (`undefined`: a
   * snapshot), what an argument holding it crosses as. */
  get handle(): number | undefined {
    return this.#ref?.handle;
  }

  /** @internal The values of a snapshot: `[rawdata, data]`. */
  snapshot(): [Uint8Array | null, Uint8Array | null] {
    return [this.#rawdata, this.#data];
  }

  #read(op: string): Promise<Uint8Array | null> {
    const ref = this.#ref!;
    if (!ref.source) return Promise.reject(new Error("this stream was decoded outside its core: its data cannot be read"));
    return ref.source.read(op, ref.handle) as Promise<Uint8Array | null>;
  }

  /** `stream.rawdata`: the raw data, `null` once decoded. (A snapshot's
   * own value, not a promise.) */
  get rawdata(): Promise<Uint8Array | null> {
    if (!this.#ref) return this.#rawdata as unknown as Promise<Uint8Array | null>;
    return this.#read("stream_rawdata");
  }

  /** `stream.data`: the decoded data, `null` before decoding. (A
   * snapshot's own value, not a promise.) */
  get data(): Promise<Uint8Array | null> {
    if (!this.#ref) return this.#data as unknown as Promise<Uint8Array | null>;
    return this.#read("stream_data");
  }

  /** `stream.get_data()`: decodes the stream if it is not (pdfminer's
   * filters; a filter pdfminer leaves, such as `DCTDecode`, gives the raw
   * bytes), then its data. */
  getData(): Promise<Uint8Array> {
    if (!this.#ref) {
      if (this.#data !== null) return Promise.resolve(this.#data);
      return Promise.reject(new Error("a stream snapshot that was not decoded cannot be decoded"));
    }
    return this.#read("stream_get_data") as Promise<Uint8Array>;
  }

  /** `stream.get_data()` */
  get_data(): Promise<Uint8Array> {
    return this.getData();
  }

  /** `stream.get_rawdata()`: the raw data, `null` once decoded. */
  getRawdata(): Promise<Uint8Array | null> {
    if (!this.#ref) return Promise.resolve(this.#rawdata);
    return this.#read("stream_get_rawdata");
  }

  /** `stream.get_rawdata()` */
  get_rawdata(): Promise<Uint8Array | null> {
    return this.getRawdata();
  }
}

/** A pdfplumber `Page` held as a value (an annotation's `data.P`, the
 * pages of `pdf.pages` read by name): the handle of the page in the core,
 * and its `page_number`. */
export class PageRef {
  constructor(
    readonly handle: number,
    readonly page_number: number | bigint,
  ) {}
}

/** Forces a JavaScript number to cross as a Python `float` (an integral
 * number otherwise crosses as an `int`). */
export class PyFloat {
  constructor(readonly value: number) {}
}

/** Makes an array cross as a Python `tuple` (an array otherwise crosses as
 * a `list`). */
export class PyTuple<T = unknown> {
  readonly items: readonly T[];
  constructor(items: Iterable<T>) {
    this.items = [...items];
  }
}

/** `PyFloat` shorthand. */
export function pyFloat(value: number): PyFloat {
  return new PyFloat(value);
}

/** `PyTuple` shorthand. */
export function pyTuple<T>(...items: T[]): PyTuple<T> {
  return new PyTuple(items);
}
