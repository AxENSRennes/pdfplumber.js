/** What both entry points export (docs/public-api.md). */

export { open, configure, PDFPlumberDocument, PDFPlumberPage, Table, DocumentReleased } from "./api.js";
export { PDFPlumberError, NotImplemented, CoreCrash } from "./errors.js";
export { PSLiteral, PSKeyword, PDFObjRef, PDFStream, PageRef, PyFloat, PyTuple, pyFloat, pyTuple } from "./pdfobjects.js";
export type {
  PDFInput,
  OpenOptions,
  RuntimeOptions,
  PDFPlumberWarning,
  PDFObject,
  Word,
  SearchResult,
  ExtractTextOptions,
  WordOptions,
  SearchOptions,
  TextLineOptions,
  TextDirection,
  CropOptions,
  DedupeOptions,
  TableOptions,
  TableStrategy,
  ExplicitTableLine,
  TableAxisGroup,
  TableFinderResult,
  SerializeOptions,
  Bbox,
  LAParams,
} from "./types.js";
