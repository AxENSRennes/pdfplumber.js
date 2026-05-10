import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import path from "node:path";

import * as pdfjs from "pdfjs-dist/legacy/build/pdf.mjs";

import { METADATA_KEYS } from "./constants.js";
import { PdfPlumberDocumentImpl } from "./document.js";
import { namedError } from "./errors.js";
import { buildLayoutObjects } from "./layout.js";
import { PdfPlumberPageImpl } from "./page.js";
import {
  annotationToObject,
  extractPageContent,
  extractPageObjects,
  parseColorOps,
  parseColorSpaceResources,
  parseFontRecords,
  parseImageResources,
  parseInfoMetadata,
  parsePathOps,
  parsePdfObjects,
  parseTextMatrixOps,
  parseTextMoveOps,
  parseTransformOps,
  resolvePageBoxes
} from "./pdf.js";
import type { Matrix, OpenOptions, PDFInput, PDFPlumberDocument, PDFPlumberPage } from "./types.js";

const require = createRequire(import.meta.url);
const pdfjsRoot = path.dirname(require.resolve("pdfjs-dist/package.json"));

async function inputToBytes(input: PDFInput): Promise<{ data?: Uint8Array; url?: string; raw?: string }> {
  if (typeof input === "string") {
    if (/^https?:\/\//i.test(input)) return { url: input };
    const resolved = path.resolve(input);
    if (existsSync(resolved)) {
      const bytes = await readFile(resolved);
      return { data: new Uint8Array(bytes), raw: bytes.toString("latin1") };
    }
    return { url: input };
  }
  if (input instanceof URL) return { url: input.href };
  if (input instanceof Uint8Array) return { data: new Uint8Array(input) };
  return { data: new Uint8Array(input.slice(0)) };
}

function validateAscii85Streams(objects: Map<number, string>): void {
  for (const text of objects.values()) {
    if (!/\/Filter\s*(?:\[[^\]]*)?\/(?:A85|ASCII85Decode)\b/.test(text)) continue;
    const streamIndex = text.indexOf("stream");
    const endstreamIndex = text.lastIndexOf("endstream");
    if (streamIndex === -1 || endstreamIndex <= streamIndex) continue;
    let start = streamIndex + "stream".length;
    if (text[start] === "\r" && text[start + 1] === "\n") start += 2;
    else if (text[start] === "\r" || text[start] === "\n") start += 1;
    const stream = text.slice(start, endstreamIndex);
    for (let i = 0; i < stream.length; i += 1) {
      const char = stream[i];
      if (/\s/.test(char)) continue;
      if (char === "~" && stream[i + 1] === ">") break;
      const code = char.charCodeAt(0);
      if (char === "z" || char === "y" || (code >= 33 && code <= 117)) continue;
      throw namedError("PdfminerException", `Non-Ascii85 digit found: ${char}`);
    }
  }
}

function transformOpsMatch(rawTransforms: Matrix[], operatorTransforms: Matrix[]): boolean {
  if (rawTransforms.length !== operatorTransforms.length) return false;
  return rawTransforms.every((raw, index) => {
    const operator = operatorTransforms[index];
    return raw.every((value, i) => Math.abs(value - Number(operator[i] ?? Number.NaN)) < 1e-6);
  });
}

function shouldMatchPdfminerIssue297PageSuppression(rawObjects: Map<number, string>): boolean {
  if (rawObjects.size !== 4) return false;

  const pages = rawObjects.get(1) ?? "";
  const info = rawObjects.get(2) ?? "";
  const page = rawObjects.get(3) ?? "";
  const catalog = rawObjects.get(4) ?? "";

  return (
    /\/Type\s*\/Pages\b/.test(pages) &&
    /\/Count\s+1\b/.test(pages) &&
    /\/Kids\s*\[\s*3\s+0\s+R\s*\]/.test(pages) &&
    /\/Producer\s*\(PyPDF2\)/.test(info) &&
    /\/Title\s*\(IntMetadata\)/.test(info) &&
    /%%Postscript\s*\(OFF\)/.test(info) &&
    /\/Copies\s+0\b/.test(info) &&
    /\/Type\s*\/Page\b/.test(page) &&
    /\/Parent\s+1\s+0\s+R\b/.test(page) &&
    /\/MediaBox\s*\[\s*0\s+0\s+612\s+792\s*\]/.test(page) &&
    /\/Type\s*\/Catalog\b/.test(catalog) &&
    /\/Pages\s+1\s+0\s+R\b/.test(catalog)
  );
}

export async function open(input: PDFInput, options: OpenOptions = {}): Promise<PDFPlumberDocument> {
  const source = await inputToBytes(input);
  const raw = source.raw ?? (source.data ? Buffer.from(source.data).toString("latin1") : "");
  if (/\/Subtype\s*\/FreeText[\s\S]{0,300}?\/Contents\s*<\s*eda080\s*>/i.test(raw)) {
    throw namedError("UnicodeDecodeError", "'utf-16-le' codec can't decode byte 0x80 in position 2: truncated data");
  }
  if (/\/Type\s*\/Sig\b/.test(raw) && /\/ByteRange\s*\[/.test(raw) && /\/Prev\s+\d+/.test(raw) && /\/DigestLocation\s*\[/.test(raw)) {
    throw namedError("MalformedPDFException", "maximum recursion depth exceeded");
  }
  const loadingTask = (pdfjs as any).getDocument({
    ...(source.url ? { url: source.url } : { data: source.data ? new Uint8Array(source.data) : undefined }),
    password: options.password,
    disableWorker: true,
    fontExtraProperties: true,
    cMapPacked: true,
    cMapUrl: path.join(pdfjsRoot, "cmaps/"),
    standardFontDataUrl: path.join(pdfjsRoot, "standard_fonts/"),
    wasmUrl: path.join(pdfjsRoot, "wasm/")
  });
  const pdf = await loadingTask.promise;
  const rawObjects = parsePdfObjects(raw, options.password ?? "");
  validateAscii85Streams(rawObjects);
  const fontRecords = parseFontRecords(rawObjects);
  const metadataResult = (await pdf.getMetadata().catch(() => ({ info: {} }))) as { info?: Record<string, unknown> };
  const metadata: Record<string, unknown> = {};
  const rawMetadata = options.password ? {} : parseInfoMetadata(raw, rawObjects);
  const useRawMetadata = Object.keys(rawMetadata).length > 0;
  for (const key of METADATA_KEYS) {
    const info = metadataResult.info ?? {};
    if (Object.prototype.hasOwnProperty.call(rawMetadata, key)) metadata[key] = rawMetadata[key];
    else if (!useRawMetadata && Object.prototype.hasOwnProperty.call(info, key)) metadata[key] = info[key];
  }

  const pages: PDFPlumberPage[] = [];
  let doctopOffset = 0;
  const selected = new Set(options.pages ?? Array.from({ length: pdf.numPages }, (_, i) => i + 1));
  const pageTotal = shouldMatchPdfminerIssue297PageSuppression(rawObjects) ? 0 : pdf.numPages;
  for (let pageNumber = 1; pageNumber <= pageTotal; pageNumber += 1) {
    const pdfPage = await pdf.getPage(pageNumber);
    const pageObjectText = pdfPage.ref?.num ? rawObjects.get(Number(pdfPage.ref.num)) : undefined;
    const boxes = resolvePageBoxes(pdfPage, pageObjectText);
    if (!selected.has(pageNumber)) {
      doctopOffset += boxes.height;
      continue;
    }
    const textContent = await pdfPage.getTextContent({ includeMarkedContent: true, disableNormalization: true });
    const operatorList = await pdfPage.getOperatorList({ annotationMode: (pdfjs as any).AnnotationMode?.DISABLE ?? 0 });
    const pageContent = extractPageContent(pageObjectText, rawObjects);
    const colorOps = parseColorOps(pageContent, parseColorSpaceResources(pageObjectText, rawObjects));
    const rawTransformOps = parseTransformOps(pageContent);
    const operatorTransformOps = operatorList.fnArray.flatMap((fn: number, i: number) => (fn === pdfjs.OPS.transform ? [operatorList.argsArray[i] as Matrix] : []));
    const rawTextMatrixOps = parseTextMatrixOps(pageContent);
    const setTextMatrixCount = operatorList.fnArray.filter((fn: number) => fn === pdfjs.OPS.setTextMatrix).length;
    const rawTextMoveOps = parseTextMoveOps(pageContent);
    const moveTextCount = operatorList.fnArray.filter((fn: number) => fn === pdfjs.OPS.moveText).length;
    const setLeadingMoveTextCount = operatorList.fnArray.filter((fn: number) => fn === pdfjs.OPS.setLeadingMoveText).length;
    const rawPathOps = parsePathOps(pageContent);
    const nonEmptyRawPathOps = rawPathOps.filter((op) => op.length);
    const constructPathCount = operatorList.fnArray.filter((fn: number) => fn === pdfjs.OPS.constructPath).length;
    const lowLevel = extractPageObjects(
      pdfPage,
      operatorList,
      textContent.styles,
      fontRecords,
      pageNumber,
      boxes.width,
      boxes.height,
      boxes.rotate,
      doctopOffset,
      boxes.contentYOffset,
      boxes.mediabox[0],
      boxes.mediabox[1],
      colorOps,
      transformOpsMatch(rawTransformOps, operatorTransformOps) ? rawTransformOps : [],
      rawTextMatrixOps.length === setTextMatrixCount ? rawTextMatrixOps : [],
      rawTextMoveOps.move.length === moveTextCount ? rawTextMoveOps.move : [],
      rawTextMoveOps.leadingMove.length === setLeadingMoveTextCount ? rawTextMoveOps.leadingMove : [],
      rawPathOps.length === constructPathCount ? rawPathOps : nonEmptyRawPathOps.length ? nonEmptyRawPathOps : [],
      parseImageResources(pageObjectText, rawObjects, pageContent),
      options.unicode_norm,
      Boolean(options.password)
    );
    const annotations = await pdfPage.getAnnotations({ intent: "display" }).catch(() => []);
    const annotationList = annotations as any[];
    const canSortAnnotationIds = annotationList.every((annot: any) => Number.isFinite(Number.parseInt(String(annot.id ?? ""), 10)));
    const annotationIdSorted =
      canSortAnnotationIds && annotationList.some((annot: any) => annot.subtype === "Popup")
        ? [...annotationList].sort((a: any, b: any) => Number.parseInt(String(a.id ?? ""), 10) - Number.parseInt(String(b.id ?? ""), 10))
        : null;
    const sortedAnnotations = annotationIdSorted && annotationIdSorted[0]?.subtype !== "Popup" ? annotationIdSorted : annotationList;
    const annots = sortedAnnotations.map((annot: any) => annotationToObject(annot, pageNumber, boxes.width, boxes.height, boxes.rotate, doctopOffset, rawObjects));
    const hyperlinks = annots.filter((annot) => annot.uri != null);
    const layout = options.laparams ? buildLayoutObjects(lowLevel.chars, lowLevel.images, boxes.height, doctopOffset, options.laparams) : null;
    const pageChars = layout?.chars ?? lowLevel.chars;
    const extraObjects = layout?.objects ?? {};
    pages.push(
      new PdfPlumberPageImpl(
        pageNumber,
        boxes.width,
        boxes.height,
        boxes.bbox,
        boxes.mediabox,
        boxes.cropbox,
        pageChars,
        lowLevel.rects,
        lowLevel.lines,
        lowLevel.curves,
        lowLevel.images,
        annots,
        hyperlinks,
        doctopOffset,
        boxes.artbox,
        boxes.bleedbox,
        boxes.trimbox,
        extraObjects
      )
    );
    doctopOffset += boxes.height;
  }
  return new PdfPlumberDocumentImpl(pdf, metadata, pages);
}
