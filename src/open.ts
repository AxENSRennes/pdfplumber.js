import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import path from "node:path";

import * as pdfjs from "pdfjs-dist/legacy/build/pdf.mjs";

import { METADATA_KEYS } from "./constants.js";
import { PdfPlumberDocumentImpl } from "./document.js";
import { buildLayoutObjects } from "./layout.js";
import { collectGraphicsHintsFromContent } from "./pdf/content.js";
import { parsePdfDocument } from "./pdf/document.js";
import {
  extractPageContent,
  parseColorSpaceResources,
  parseFontRecords,
  parseImageResources,
  parsePageFontObjectNumbers
} from "./pdf/resources.js";
import { PdfPlumberPageImpl } from "./page.js";
import { shouldEmulatePdfminerOpenError, shouldSuppressPagesLikePdfminer, validateStreamsLikePdfminer } from "./pdfminer-compat.js";
import {
  annotationToObject,
  extractPageObjects,
  parseInfoMetadata,
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

function transformOpsMatch(rawTransforms: Matrix[], operatorTransforms: Matrix[]): boolean {
  if (rawTransforms.length !== operatorTransforms.length) return false;
  return rawTransforms.every((raw, index) => {
    const operator = operatorTransforms[index];
    return raw.every((value, i) => Math.abs(value - Number(operator[i] ?? Number.NaN)) < 1e-6);
  });
}

export async function open(input: PDFInput, options: OpenOptions = {}): Promise<PDFPlumberDocument> {
  const source = await inputToBytes(input);
  const raw = source.raw ?? (source.data ? Buffer.from(source.data).toString("latin1") : "");
  const store = parsePdfDocument(raw, { password: options.password ?? "" });
  const rawObjects = store.rawObjects;
  const compatContext = { raw, objects: rawObjects, password: options.password };
  const compatOpenError = shouldEmulatePdfminerOpenError(compatContext);
  if (compatOpenError) throw compatOpenError;
  validateStreamsLikePdfminer(compatContext);
  if (shouldSuppressPagesLikePdfminer(compatContext)) {
    const rawMetadata = parseInfoMetadata(raw, rawObjects);
    const metadata: Record<string, unknown> = {};
    for (const key of METADATA_KEYS) {
      if (Object.prototype.hasOwnProperty.call(rawMetadata, key)) metadata[key] = rawMetadata[key];
    }
    return new PdfPlumberDocumentImpl({ destroy: async () => undefined }, metadata, []);
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
  const fontRecords = parseFontRecords(store);
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
  const pageTotal = pdf.numPages;
  for (let pageNumber = 1; pageNumber <= pageTotal; pageNumber += 1) {
    const pdfPage = await pdf.getPage(pageNumber);
    const pageModel = store.getPageModel(pdfPage.ref?.num ? Number(pdfPage.ref.num) : undefined);
    const pageObjectText = pageModel?.raw ?? (pdfPage.ref?.num ? rawObjects.get(Number(pdfPage.ref.num)) : undefined);
    const pageOwner = pageModel ?? pageObjectText;
    const pageFontObjectNumbers = new Set(parsePageFontObjectNumbers(pageOwner, store));
    const pageFontRecords = pageFontObjectNumbers.size
      ? [
          ...fontRecords.filter((record) => pageFontObjectNumbers.has(record.objectNumber)).map((record) => ({ ...record, pageScoped: true })),
          ...fontRecords.filter((record) => !pageFontObjectNumbers.has(record.objectNumber))
        ]
      : fontRecords;
    const boxes = resolvePageBoxes(pdfPage, pageObjectText);
    if (!selected.has(pageNumber)) {
      doctopOffset += boxes.height;
      continue;
    }
    const textContent = await pdfPage.getTextContent({ includeMarkedContent: true, disableNormalization: true });
    const operatorList = await pdfPage.getOperatorList({ annotationMode: (pdfjs as any).AnnotationMode?.DISABLE ?? 0 });
    const pageContent = extractPageContent(pageOwner, store);
    const graphicsHints = collectGraphicsHintsFromContent(pageContent, parseColorSpaceResources(pageOwner, store));
    const colorOps = graphicsHints.colorOps;
    const rawTransformOps = graphicsHints.transforms;
    const operatorTransformOps = operatorList.fnArray.flatMap((fn: number, i: number) => (fn === pdfjs.OPS.transform ? [operatorList.argsArray[i] as Matrix] : []));
    const rawTextMatrixOps = graphicsHints.textMatrices;
    const setTextMatrixCount = operatorList.fnArray.filter((fn: number) => fn === pdfjs.OPS.setTextMatrix).length;
    const rawTextMoveOps = { move: graphicsHints.textMoves, leadingMove: graphicsHints.leadingTextMoves };
    const moveTextCount = operatorList.fnArray.filter((fn: number) => fn === pdfjs.OPS.moveText).length;
    const setLeadingMoveTextCount = operatorList.fnArray.filter((fn: number) => fn === pdfjs.OPS.setLeadingMoveText).length;
    const rawPathOps = graphicsHints.pathOps;
    const nonEmptyRawPathOps = rawPathOps.filter((op) => op.length);
    const constructPathCount = operatorList.fnArray.filter((fn: number) => fn === pdfjs.OPS.constructPath).length;
    const lowLevel = extractPageObjects(
      pdfPage,
      operatorList,
      textContent.styles,
      pageFontRecords,
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
      parseImageResources(pageOwner, store, pageContent),
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
