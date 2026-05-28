import * as pdfjs from "pdfjs-dist/legacy/build/pdf.mjs";

import { METADATA_KEYS } from "./constants.js";
import { PdfPlumberDocumentImpl } from "./document.js";
import { buildLayoutObjects } from "./layout.js";
import { collectGraphicsHintsFromContent } from "./pdf/content.js";
import { parsePageLabelsLikePdfminer, parsePdfDocument } from "./pdf/document.js";
import { namedError } from "./errors.js";
import { asArray, isRef } from "./pdf/primitives.js";
import {
  extractPageContent,
  parseColorSpaceResources,
  parseFontRecords,
  parseImageResources,
  parsePageFontResourceMap,
  parsePageFontObjectNumbers
} from "./pdf/resources.js";
import { PdfPlumberPageImpl } from "./page.js";
import { annotationStringDecodeErrorLikePdfminer, shouldEmulatePdfminerOpenError, shouldSuppressPagesLikePdfminer, validateStreamsLikePdfminer } from "./pdfminer-compat.js";
import {
  annotationToObject,
  extractPageObjects,
  extractPredefinedCMapCharsFromContent,
  parseInfoMetadata,
  resolvePageBoxes
} from "./pdf.js";
import type { Matrix, OpenOptions, PDFInput, PDFObject, PDFPlumberDocument, PDFPlumberPage } from "./types.js";

interface InputBytes {
  data: Uint8Array;
  raw: string;
}

let nodePdfjsAssetOptions: Record<string, string> | null = null;
let nodeReadFile: ((filePath: string) => Promise<Uint8Array>) | null = null;
let nodeFileURLToPath: ((url: string | URL) => string) | null = null;

function isNodeRuntime(): boolean {
  return typeof process !== "undefined" && process.versions?.node != null;
}

async function importNodeBuiltin(name: string): Promise<any> {
  if (!isNodeRuntime()) throw new Error("Node builtins are only available in Node.js");
  const importer = Function("specifier", "return import(specifier)") as (specifier: string) => Promise<any>;
  return importer(`node:${name}`);
}

function urlProtocol(input: string): string | null {
  try {
    return new URL(input).protocol;
  } catch {
    return null;
  }
}

function bytesToLatin1(bytes: Uint8Array): string {
  const chunkSize = 0x8000;
  const chunks: string[] = [];
  for (let i = 0; i < bytes.length; i += chunkSize) {
    chunks.push(String.fromCharCode(...bytes.subarray(i, i + chunkSize)));
  }
  return chunks.join("");
}

function isBlobInput(input: PDFInput): input is Blob {
  return typeof Blob !== "undefined" && input instanceof Blob;
}

async function readNodeFileBytes(input: string): Promise<Uint8Array> {
  if (!isNodeRuntime()) throw new Error("Filesystem paths are only supported in Node.js");
  if (!nodeReadFile) {
    const { readFile } = await importNodeBuiltin("fs/promises");
    nodeReadFile = readFile;
  }
  const readFile = nodeReadFile;
  if (!readFile) throw new Error("Node file reader is unavailable");
  return new Uint8Array(await readFile(input));
}

async function nodeFilePathFromUrl(input: string | URL): Promise<string> {
  if (!isNodeRuntime()) throw new Error("File URLs are only supported in Node.js");
  if (!nodeFileURLToPath) {
    const { fileURLToPath } = await importNodeBuiltin("url");
    nodeFileURLToPath = fileURLToPath;
  }
  const fileURLToPath = nodeFileURLToPath;
  if (!fileURLToPath) throw new Error("Node file URL adapter is unavailable");
  return fileURLToPath(input);
}

async function fetchBytes(url: string): Promise<Uint8Array> {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`Failed to fetch PDF: ${response.status} ${response.statusText}`);
  return new Uint8Array(await response.arrayBuffer());
}

async function inputToBytes(input: PDFInput): Promise<InputBytes> {
  if (typeof input === "string") {
    const protocol = urlProtocol(input);
    const bytes =
      protocol === "http:" || protocol === "https:"
        ? await fetchBytes(input)
        : protocol === "file:"
          ? await readNodeFileBytes(await nodeFilePathFromUrl(input))
          : await readNodeFileBytes(input);
    return { data: bytes, raw: bytesToLatin1(bytes) };
  }
  if (input instanceof URL) {
    const bytes = input.protocol === "file:" ? await readNodeFileBytes(await nodeFilePathFromUrl(input)) : await fetchBytes(input.href);
    return { data: bytes, raw: bytesToLatin1(bytes) };
  }
  if (isBlobInput(input)) {
    const bytes = new Uint8Array(await input.arrayBuffer());
    return { data: bytes, raw: bytesToLatin1(bytes) };
  }
  const bytes = input instanceof Uint8Array ? new Uint8Array(input) : new Uint8Array(input.slice(0));
  return { data: bytes, raw: bytesToLatin1(bytes) };
}

function ensurePdfjsWorkerConfigured(): void {
  if (isNodeRuntime()) return;
  const workerOptions = (pdfjs as any).GlobalWorkerOptions;
  if (workerOptions && !workerOptions.workerSrc) {
    workerOptions.workerSrc = new URL("./pdf.worker.js", import.meta.url).href;
  }
}

async function pdfjsAssetOptions(): Promise<Record<string, string>> {
  if (!isNodeRuntime()) return {};
  if (nodePdfjsAssetOptions) return nodePdfjsAssetOptions;
  const [{ createRequire }, path] = await Promise.all([importNodeBuiltin("module"), importNodeBuiltin("path")]);
  const require = createRequire(import.meta.url);
  const pdfjsRoot = path.dirname(require.resolve("pdfjs-dist/package.json"));
  nodePdfjsAssetOptions = {
    cMapUrl: path.join(pdfjsRoot, "cmaps/"),
    standardFontDataUrl: path.join(pdfjsRoot, "standard_fonts/"),
    wasmUrl: path.join(pdfjsRoot, "wasm/")
  };
  return nodePdfjsAssetOptions;
}

function transformOpsMatch(rawTransforms: Matrix[], operatorTransforms: Matrix[]): boolean {
  if (rawTransforms.length !== operatorTransforms.length) return false;
  return rawTransforms.every((raw, index) => {
    const operator = operatorTransforms[index];
    return raw.every((value, i) => Math.abs(value - Number(operator[i] ?? Number.NaN)) < 1e-6);
  });
}

function charText(objects: PDFObject[]): string {
  return objects.map((object) => String(object.text ?? "")).join("");
}

function nativeFallbackPreservesPdfjsText(nativeChars: PDFObject[], pdfjsChars: PDFObject[]): boolean {
  if (!nativeChars.length || !pdfjsChars.length || nativeChars.length <= pdfjsChars.length) return false;
  return charText(nativeChars).includes(charText(pdfjsChars));
}

function sortAnnotationsByRawPageOrder(annotationList: any[], pageModel: ReturnType<ReturnType<typeof parsePdfDocument>["getPageModel"]> | undefined, store: ReturnType<typeof parsePdfDocument>): any[] | null {
  const annots = asArray(store.resolve(pageModel?.dict?.get("Annots")));
  const objectOrder = annots?.filter(isRef).map((ref) => ref.objectNumber) ?? [];
  if (!objectOrder.length) return null;
  const rank = new Map(objectOrder.map((objectNumber, index) => [objectNumber, index]));
  const withRanks = annotationList.map((annotation, index) => {
    const objectNumber = Number.parseInt(String(annotation.id ?? ""), 10);
    return { annotation, index, rank: rank.get(objectNumber) };
  });
  if (!withRanks.some((item) => item.rank != null)) return null;
  return withRanks
    .sort((a, b) => (a.rank ?? Number.POSITIVE_INFINITY) - (b.rank ?? Number.POSITIVE_INFINITY) || a.index - b.index)
    .map((item) => item.annotation);
}

function annotationResolutionErrorLikePdfminer(pageModel: ReturnType<ReturnType<typeof parsePdfDocument>["getPageModel"]> | undefined, store: ReturnType<typeof parsePdfDocument>): Error | null {
  const annots = pageModel?.dict?.get("Annots");
  if (annots == null) return null;
  const annotationArray = asArray(store.resolve(annots));
  if (!annotationArray) return namedError("MalformedPDFException", "maximum recursion depth exceeded");

  const catalogObjectNumbers = new Set(
    [...store.rawObjects.entries()]
      .filter(([, objectText]) => /\/Type\s*\/Catalog\b/.test(objectText))
      .map(([objectNumber]) => objectNumber)
  );
  for (const annotationRef of annotationArray) {
    if (!isRef(annotationRef)) continue;
    const annotationObject = store.getRawObjectText(annotationRef.objectNumber) ?? "";
    if (!/\/Subtype\s*\/Widget\b/.test(annotationObject) || !/\/FT\s*\/Sig\b/.test(annotationObject)) continue;
    const signatureObjectNumber = Number(/\/V\s+(\d+)\s+\d+\s+R\b/.exec(annotationObject)?.[1] ?? Number.NaN);
    const signatureObject = Number.isFinite(signatureObjectNumber) ? (store.getRawObjectText(signatureObjectNumber) ?? "") : "";
    const dataRefs = [...signatureObject.matchAll(/\/Data\s+(\d+)\s+\d+\s+R\b/g)].map((match) => Number(match[1]));
    if (dataRefs.some((objectNumber) => catalogObjectNumbers.has(objectNumber))) {
      return namedError("MalformedPDFException", "maximum recursion depth exceeded");
    }
  }
  return null;
}

export async function open(input: PDFInput, options: OpenOptions = {}): Promise<PDFPlumberDocument> {
  ensurePdfjsWorkerConfigured();
  const source = await inputToBytes(input);
  const raw = source.raw;
  const store = parsePdfDocument(raw, { password: options.password ?? "" });
  const rawObjects = store.rawObjects;
  const compatContext = { raw, objects: rawObjects, password: options.password };
  const compatOpenError = shouldEmulatePdfminerOpenError(compatContext);
  if (compatOpenError) throw compatOpenError;
  validateStreamsLikePdfminer(compatContext);
  if (shouldSuppressPagesLikePdfminer(compatContext)) {
    const rawMetadata = parseInfoMetadata(raw, rawObjects);
    return new PdfPlumberDocumentImpl({ destroy: async () => undefined }, rawMetadata, []);
  }
  const loadingTask = (pdfjs as any).getDocument({
    data: new Uint8Array(source.data),
    password: options.password,
    disableWorker: true,
    fontExtraProperties: true,
    cMapPacked: true,
    ...(await pdfjsAssetOptions())
  });
  const pdf = await loadingTask.promise;
  const fontRecords = parseFontRecords(store);
  const metadataResult = (await pdf.getMetadata().catch(() => ({ info: {} }))) as { info?: Record<string, unknown> };
  const metadata: Record<string, unknown> = {};
  const rawMetadata = options.password ? {} : parseInfoMetadata(raw, rawObjects);
  const useRawMetadata = Object.keys(rawMetadata).length > 0;
  if (useRawMetadata) {
    Object.assign(metadata, rawMetadata);
  } else {
    for (const key of METADATA_KEYS) {
      const info = metadataResult.info ?? {};
      if (Object.prototype.hasOwnProperty.call(info, key)) metadata[key] = info[key];
    }
  }

  const pages: PDFPlumberPage[] = [];
  let doctopOffset = 0;
  const selected = new Set(options.pages ?? Array.from({ length: pdf.numPages }, (_, i) => i + 1));
  const maxSelectedPage = selected.size ? Math.max(...selected) : 0;
  const pageTotal = pdf.numPages;
  const pageLabels = parsePageLabelsLikePdfminer(store, pageTotal);
  for (let pageNumber = 1; pageNumber <= pageTotal; pageNumber += 1) {
    if (pageNumber > maxSelectedPage) break;
    const pdfPage = await pdf.getPage(pageNumber);
    const pageModel = store.getPageModel(pdfPage.ref?.num ? Number(pdfPage.ref.num) : undefined);
    const pageObjectText = pageModel?.raw ?? (pdfPage.ref?.num ? rawObjects.get(Number(pdfPage.ref.num)) : undefined);
    const pageOwner = pageModel ?? pageObjectText;
    const pageFontResourceMap = parsePageFontResourceMap(pageOwner, store);
    const pageFontObjectNumbers = new Set(parsePageFontObjectNumbers(pageOwner, store));
    const allReferencedPageFontsMissing =
      pageFontObjectNumbers.size > 0 && [...pageFontObjectNumbers].every((objectNumber) => store.getObject(objectNumber) == null);
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
    const rawLineWidthOps = graphicsHints.lineWidths;
    const setLineWidthCount = operatorList.fnArray.filter((fn: number) => fn === pdfjs.OPS.setLineWidth).length;
    const rawDashOps = graphicsHints.dashOps;
    const setDashCount = operatorList.fnArray.filter((fn: number) => fn === pdfjs.OPS.setDash).length;
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
      rawLineWidthOps.length === setLineWidthCount ? rawLineWidthOps : [],
      rawDashOps.length === setDashCount ? rawDashOps : [],
      transformOpsMatch(rawTransformOps, operatorTransformOps) ? rawTransformOps : [],
      rawTextMatrixOps.length === setTextMatrixCount ? rawTextMatrixOps : [],
      rawTextMoveOps.move.length === moveTextCount ? rawTextMoveOps.move : [],
      rawTextMoveOps.leadingMove.length === setLeadingMoveTextCount ? rawTextMoveOps.leadingMove : [],
      rawPathOps.length === constructPathCount ? rawPathOps : nonEmptyRawPathOps.length ? nonEmptyRawPathOps : [],
      parseImageResources(pageOwner, store, pageContent),
      options.unicode_norm,
      Boolean(options.password)
    );
    const nativeCMapChars = extractPredefinedCMapCharsFromContent(
      pageContent,
      pageFontResourceMap,
      pageFontRecords,
      pageNumber,
      boxes.width,
      boxes.height,
      boxes.rotate,
      doctopOffset,
      boxes.mediabox[0],
      boxes.mediabox[1]
    );
    if (nativeCMapChars.length) {
      const nativeAllTextChars = extractPredefinedCMapCharsFromContent(
        pageContent,
        pageFontResourceMap,
        pageFontRecords,
        pageNumber,
        boxes.width,
        boxes.height,
        boxes.rotate,
        doctopOffset,
        boxes.mediabox[0],
        boxes.mediabox[1],
        { includeSimpleText: true }
      );
      lowLevel.chars.splice(
        0,
        lowLevel.chars.length,
        ...(nativeFallbackPreservesPdfjsText(nativeAllTextChars, nativeCMapChars) ? nativeAllTextChars : nativeCMapChars)
      );
    } else if (allReferencedPageFontsMissing) lowLevel.chars.splice(0, lowLevel.chars.length);
    else {
      const nativeMalformedTextChars = extractPredefinedCMapCharsFromContent(
        pageContent,
        pageFontResourceMap,
        pageFontRecords,
        pageNumber,
        boxes.width,
        boxes.height,
        boxes.rotate,
        doctopOffset,
        boxes.mediabox[0],
        boxes.mediabox[1],
        { includeSimpleText: true }
      );
      if (
        nativeFallbackPreservesPdfjsText(nativeMalformedTextChars, lowLevel.chars) ||
        (nativeMalformedTextChars.length > lowLevel.chars.length * 2 && nativeMalformedTextChars.length >= lowLevel.chars.length + 100)
      ) {
        lowLevel.chars.splice(0, lowLevel.chars.length, ...nativeMalformedTextChars);
      }
    }
    const annotations = await pdfPage.getAnnotations({ intent: "display" }).catch(() => []);
    const annotationList = annotations as any[];
    const canSortAnnotationIds = annotationList.every((annot: any) => Number.isFinite(Number.parseInt(String(annot.id ?? ""), 10)));
    const rawOrderSorted = sortAnnotationsByRawPageOrder(annotationList, pageModel, store);
    const annotationIdSorted =
      !rawOrderSorted && canSortAnnotationIds && annotationList.some((annot: any) => annot.subtype === "Popup")
        ? [...annotationList].sort((a: any, b: any) => Number.parseInt(String(a.id ?? ""), 10) - Number.parseInt(String(b.id ?? ""), 10))
        : null;
    const rawOrIdSorted = rawOrderSorted ?? annotationIdSorted;
    const sortedAnnotations = rawOrIdSorted && rawOrIdSorted[0]?.subtype !== "Popup" ? rawOrIdSorted : annotationList;
    const annotationError = annotationResolutionErrorLikePdfminer(pageModel, store) ?? annotationStringDecodeErrorLikePdfminer(
      sortedAnnotations.map((annot: any) => {
        const objectNumber = Number.parseInt(String(annot.id ?? ""), 10);
        return Number.isFinite(objectNumber) ? rawObjects.get(objectNumber) : undefined;
      })
    );
    const annotsError = options.raise_unicode_errors === false ? null : annotationError;
    const annots = annotsError
      ? []
      : sortedAnnotations.map((annot: any) => annotationToObject(annot, pageNumber, boxes.width, boxes.height, boxes.rotate, doctopOffset, rawObjects));
    const hyperlinks = annotsError ? [] : annots.filter((annot) => annot.uri != null);
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
        extraObjects,
        annotsError,
        pageLabels?.[pageNumber - 1] ?? null
      )
    );
    doctopOffset += boxes.height;
  }
  return new PdfPlumberDocumentImpl(pdf, metadata, pages, selected.size < pdf.numPages ? "cleanup" : "destroy");
}
