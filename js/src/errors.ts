/**
 * Errors of the public API (docs/public-api.md, "Errors").
 *
 * An exception raised by the core (a Python exception of pdfminer,
 * pdfplumber or the builtins) is a `PDFPlumberError` whose `name` is the
 * Python class name (`"PdfminerException"`, `"PDFSyntaxError"`,
 * `"ValueError"`...) and whose `message` is `str(e)`. `pythonMro` lists the
 * class and its bases (`type(e).__mro__`, as `module.qualname`), so that
 * `error.isInstance("PDFException")` answers like Python's `isinstance`.
 */

/** What the core sends for an exception (crates/wasm/src/errors.rs). */
export interface PythonErrorInfo {
  name: string;
  module: string;
  qualname: string;
  mro: string[];
  message: string;
  /** The exception it wraps (`PdfminerException(e)`,
   * `MalformedPDFException(e)`), absent or `null` otherwise. */
  cause?: PythonErrorInfo | null;
}

/** A Python exception raised by the core. */
export class PDFPlumberError extends Error {
  /** `type(e).__module__` */
  readonly pythonModule: string;
  /** `type(e).__qualname__` */
  readonly pythonQualname: string;
  /** `type(e).__mro__`, each class as `module.qualname`. */
  readonly pythonMro: readonly string[];

  constructor(info: PythonErrorInfo) {
    // `cause`: the Python exception it wraps, as an error of its own
    super(info.message, info.cause ? { cause: fromPython(info.cause) } : undefined);
    this.name = info.name;
    this.pythonModule = info.module;
    this.pythonQualname = info.qualname;
    this.pythonMro = Object.freeze([...info.mro]);
  }

  /** `isinstance(e, cls)`: `cls` is a class name (`"PDFException"`) or a
   * qualified one (`"pdfminer.cmapdb.CMapDB.CMapNotFound"`). */
  isInstance(cls: string): boolean {
    return this.pythonMro.some((q) => q === cls || q.slice(q.lastIndexOf(".") + 1) === cls);
  }
}

/** A feature of the API whose Rust core is not ported yet (the page
 * layer), or a core path marked "not ported" (`NotPorted`). */
export class NotImplemented extends PDFPlumberError {
  constructor(what: string) {
    super({
      name: "NotImplemented",
      module: "pdfplumber-js",
      qualname: "NotImplemented",
      mro: ["pdfplumber-js.NotImplemented"],
      message: `${what} is not implemented yet: its Rust core (the page layer of the port) has not landed`,
    });
    this.name = "NotImplemented";
  }
}

/** The WebAssembly core stopped on an internal error (a Rust panic: a bug).
 * Its state is lost; the next `open()` starts a fresh instance. */
export class CoreCrash extends Error {
  constructor(cause: unknown) {
    super(`the pdfplumber core crashed: ${cause instanceof Error ? cause.message : String(cause)}`, { cause });
    this.name = "CoreCrash";
  }
}

/** The error of a core exception. */
export function fromPython(info: PythonErrorInfo): PDFPlumberError {
  if (info.module === "pycompat" && info.name === "NotPorted") {
    const e = new NotImplemented("this feature");
    e.message = info.message;
    return e;
  }
  return new PDFPlumberError(info);
}
