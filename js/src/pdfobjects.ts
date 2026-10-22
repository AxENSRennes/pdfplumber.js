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

/** `pdfminer.pdftypes.PDFStream`: a stream's dictionary and, when
 * pdfminer has read them, its raw and decoded data. */
export class PDFStream {
  constructor(
    readonly objid: number | bigint | null,
    readonly genno: number | bigint | null,
    readonly attrs: Record<string, unknown>,
    readonly rawdata: Uint8Array | null,
    readonly data: Uint8Array | null,
  ) {}
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
