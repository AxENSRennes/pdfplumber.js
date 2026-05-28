import { FONT_UNITS_PER_EM } from "../constants.js";
import type { FontRecord, ImageResource } from "../types.js";
import { colorSpaceName } from "../utils.js";
import { isVerticalCMapNameLikePdfminer, normalizeIdentityCMapNameLikePdfminer } from "../pdfminer-compat.js";
import type { PdfPageModel, PdfObjectStore } from "./document.js";
import { PdfObjectStore as PdfObjectStoreClass } from "./document.js";
import { parseObject } from "./parser.js";
import {
  asArray,
  asDict,
  asName,
  asNumber,
  isArray,
  isName,
  isRef,
  isStream
} from "./primitives.js";
import type { PdfArray, PdfDict, PdfIndirectObject, PdfPrimitive, PdfRef, PdfStream } from "./primitives.js";
import { decodePdfName } from "./primitives.js";
import { decodePdfStreamText, decodeStream, decodeStreamToLatin1 } from "./streams.js";
import { tokenizePdf, type PdfToken } from "./tokenizer.js";

export type PdfResourceContext = PdfObjectStore | Map<number, string>;
export type PdfResourceOwner = PdfPageModel | PdfIndirectObject | PdfDict | string | undefined;

function toStore(context: PdfResourceContext): PdfObjectStore {
  return context instanceof PdfObjectStoreClass ? context : new PdfObjectStoreClass("", context);
}

function rawObjects(context: PdfResourceContext): Map<number, string> {
  return context instanceof PdfObjectStoreClass ? context.rawObjects : context;
}

function isPageModel(value: PdfResourceOwner): value is PdfPageModel {
  return typeof value === "object" && value !== null && "getDecodedContent" in value && "raw" in value;
}

function isIndirectObject(value: PdfResourceOwner): value is PdfIndirectObject {
  return typeof value === "object" && value !== null && "objectNumber" in value && "value" in value && "raw" in value;
}

function parseOwnerObject(owner: PdfResourceOwner): PdfIndirectObject | null {
  if (!owner || owner instanceof Map) return null;
  if (isIndirectObject(owner)) return owner;
  if (isPageModel(owner)) return parseObject(owner.raw);
  if (typeof owner === "string") return parseObject(owner);
  return null;
}

function ownerObjectNumber(owner: PdfResourceOwner): number | undefined {
  const object = parseOwnerObject(owner);
  return object?.objectNumber;
}

function ownerDict(owner: PdfResourceOwner): PdfDict | undefined {
  if (!owner) return undefined;
  if (owner instanceof Map) return owner;
  if (isPageModel(owner)) return owner.dict;
  const object = parseOwnerObject(owner);
  const value = object?.value;
  if (value instanceof Map) return value;
  if (isStream(value)) return value.dict;
  return undefined;
}

function resolveOne(store: PdfObjectStore, value: PdfPrimitive | undefined): PdfPrimitive | undefined {
  return isRef(value) ? store.getObject(value.objectNumber)?.value : value;
}

function resolvedDict(store: PdfObjectStore, value: PdfPrimitive | undefined): PdfDict | undefined {
  const resolved = resolveOne(store, value);
  if (resolved instanceof Map) return resolved;
  if (isStream(resolved)) return resolved.dict;
  return undefined;
}

function resolvedArray(store: PdfObjectStore, value: PdfPrimitive | undefined): PdfArray | undefined {
  return asArray(resolveOne(store, value));
}

function resolvedNumber(store: PdfObjectStore, value: PdfPrimitive | undefined): number | undefined {
  return asNumber(resolveOne(store, value));
}

function resolvedBoolean(store: PdfObjectStore, value: PdfPrimitive | undefined): boolean | undefined {
  const resolved = resolveOne(store, value);
  return typeof resolved === "boolean" ? resolved : undefined;
}

function resolveToStream(store: PdfObjectStore, value: PdfPrimitive | undefined): { objectNumber?: number; stream: PdfStream } | undefined {
  if (isRef(value)) {
    const object = store.getObject(value.objectNumber);
    return object?.stream ? { objectNumber: object.objectNumber, stream: object.stream } : undefined;
  }
  return isStream(value) ? { stream: value } : undefined;
}

function primitiveName(store: PdfObjectStore, value: PdfPrimitive | undefined): string | undefined {
  const resolved = resolveOne(store, value);
  return asName(resolved);
}

function primitiveNameOrString(store: PdfObjectStore, value: PdfPrimitive | undefined): string | undefined {
  const resolved = resolveOne(store, value);
  if (isName(resolved)) return resolved.name;
  return typeof resolved === "string" ? resolved : undefined;
}

function resourceSubdict(store: PdfObjectStore, resources: PdfDict | undefined, key: string): PdfDict | undefined {
  return resolvedDict(store, resources?.get(key));
}

function findCatalogPagesRef(store: PdfObjectStore): PdfRef | undefined {
  for (const objectNumber of store.rawObjects.keys()) {
    const dict = store.getObject(objectNumber)?.value;
    if (!(dict instanceof Map)) continue;
    if (asName(dict.get("Type")) === "Catalog" && isRef(dict.get("Pages"))) return dict.get("Pages") as PdfRef;
  }
  return undefined;
}

function inheritedResourcesFromPageTree(store: PdfObjectStore, pageObjectNumber: number): PdfDict | undefined {
  const root = findCatalogPagesRef(store);
  if (!root) return undefined;
  const visited = new Set<number>();

  const visit = (ref: PdfRef, inherited?: PdfDict): PdfDict | undefined => {
    if (visited.has(ref.objectNumber)) return undefined;
    visited.add(ref.objectNumber);
    const object = store.getObject(ref.objectNumber);
    const dict = object?.value instanceof Map ? object.value : undefined;
    if (!dict) return undefined;
    const resources = resolvedDict(store, dict.get("Resources")) ?? inherited;
    const type = asName(dict.get("Type"));
    if (type === "Page" && ref.objectNumber === pageObjectNumber) return resources;
    if (type !== "Pages") return undefined;
    for (const child of resolvedArray(store, dict.get("Kids")) ?? []) {
      if (!isRef(child)) continue;
      const found = visit(child, resources);
      if (found) return found;
    }
    return undefined;
  };

  return visit(root);
}

function inheritedResourcesFromParentRefs(store: PdfObjectStore, dict: PdfDict | undefined): PdfDict | undefined {
  let current = dict;
  const seen = new Set<number>();
  while (current) {
    const resources = resolvedDict(store, current.get("Resources"));
    if (resources) return resources;
    const parent = current.get("Parent");
    if (!isRef(parent) || seen.has(parent.objectNumber)) return undefined;
    seen.add(parent.objectNumber);
    current = store.getObject(parent.objectNumber)?.value instanceof Map ? (store.getObject(parent.objectNumber)?.value as PdfDict) : undefined;
  }
  return undefined;
}

function effectivePageResources(store: PdfObjectStore, owner: PdfResourceOwner): PdfDict | undefined {
  const dict = ownerDict(owner);
  const direct = resolvedDict(store, dict?.get("Resources"));
  if (direct) return direct;
  const viaParent = inheritedResourcesFromParentRefs(store, dict);
  if (viaParent) return viaParent;
  const objectNumber = ownerObjectNumber(owner);
  return objectNumber == null ? undefined : inheritedResourcesFromPageTree(store, objectNumber);
}

function formResources(store: PdfObjectStore, form: PdfStream, fallback: PdfDict | undefined): PdfDict | undefined {
  return resolvedDict(store, form.dict.get("Resources")) ?? fallback;
}

function xObjectMap(store: PdfObjectStore, resources: PdfDict | undefined): Map<string, PdfPrimitive> {
  const out = new Map<string, PdfPrimitive>();
  const xobjects = resourceSubdict(store, resources, "XObject");
  if (!xobjects) return out;
  for (const [name, value] of xobjects) out.set(name, value);
  return out;
}

function contentStreams(store: PdfObjectStore, owner: PdfResourceOwner): PdfPrimitive[] {
  const contents = resolveOne(store, ownerDict(owner)?.get("Contents"));
  if (contents == null) return [];
  return isArray(contents) ? contents : [contents];
}

function decodeContentValue(store: PdfObjectStore, value: PdfPrimitive): string {
  const stream = resolveToStream(store, value);
  if (stream) return decodeStreamToLatin1(stream.stream);
  if (isRef(value)) return decodePdfStreamText(store.getRawObjectText(value.objectNumber));
  return "";
}

function decodedContents(store: PdfObjectStore, owner: PdfResourceOwner): string {
  return contentStreams(store, owner)
    .map((value) => decodeContentValue(store, value))
    .join("\n");
}

function matrixString(store: PdfObjectStore, dict: PdfDict): string | undefined {
  const matrix = resolvedArray(store, dict.get("Matrix"));
  if (!matrix) return undefined;
  const values = matrix.map((value) => resolvedNumber(store, value));
  return values.length >= 6 && values.slice(0, 6).every((value) => value != null) ? values.slice(0, 6).map(String).join(" ") : undefined;
}

function findDrawOperations(content: string): Array<{ name: string; start: number; end: number }> {
  const tokens = tokenizePdf(content);
  const draws: Array<{ name: string; start: number; end: number }> = [];
  let depth = 0;
  let operands: typeof tokens = [];

  for (const token of tokens) {
    if (token.type === "arrayStart" || token.type === "dictStart") {
      if (depth === 0) operands.push(token);
      depth += 1;
      continue;
    }
    if (token.type === "arrayEnd" || token.type === "dictEnd") {
      depth = Math.max(0, depth - 1);
      if (depth === 0) operands.push(token);
      continue;
    }
    if (depth > 0) continue;
    if (token.type === "keyword") {
      if (token.value === "Do") {
        const operand = [...operands].reverse().find((candidate) => candidate.type === "name");
        if (operand?.type === "name") draws.push({ name: operand.value.name, start: operand.start, end: token.end });
      }
      operands = [];
    } else {
      operands.push(token);
    }
  }

  return draws;
}

function expandFormXObjectsInContent(
  content: string,
  store: PdfObjectStore,
  resources: PdfDict | undefined,
  active = new Set<number>(),
  depth = 0
): string {
  if (!content || depth > 8) return content;
  const xobjects = xObjectMap(store, resources);
  const replacements: Array<{ start: number; end: number; text: string }> = [];

  for (const draw of findDrawOperations(content)) {
    const target = resolveToStream(store, xobjects.get(draw.name));
    if (!target || active.has(target.objectNumber ?? -1)) continue;
    if (asName(target.stream.dict.get("Subtype")) !== "Form") continue;
    const nextActive = new Set(active);
    if (target.objectNumber != null) nextActive.add(target.objectNumber);
    const childResources = formResources(store, target.stream, resources);
    const formContent = expandFormXObjectsInContent(decodeStreamToLatin1(target.stream), store, childResources, nextActive, depth + 1);
    const matrix = matrixString(store, target.stream.dict);
    replacements.push({ start: draw.start, end: draw.end, text: matrix ? `q\n${matrix} cm\n${formContent}\nQ` : formContent });
  }

  if (!replacements.length) return content;
  let out = "";
  let cursor = 0;
  for (const replacement of replacements.sort((a, b) => a.start - b.start)) {
    out += content.slice(cursor, replacement.start);
    out += replacement.text;
    cursor = replacement.end;
  }
  out += content.slice(cursor);
  return out;
}

export function extractPageContent(owner: PdfResourceOwner, context: PdfResourceContext): string {
  if (!owner) return "";
  const store = toStore(context);
  const content = isPageModel(owner) ? owner.getDecodedContent() : decodedContents(store, owner);
  return expandFormXObjectsInContent(content, store, effectivePageResources(store, owner));
}

function pdfValueRepr(store: PdfObjectStore, value: PdfPrimitive | undefined, key = ""): string | number | undefined {
  if (value == null) return undefined;
  if (isRef(value)) return `<PDFObjRef:${value.objectNumber}>`;
  const resolved = resolveOne(store, value);
  if (typeof resolved === "number") return resolved;
  if (isName(resolved)) return `/'${resolved.name}'`;
  if (isArray(resolved)) {
    const numbers = resolved.map((item) => resolvedNumber(store, item));
    if (numbers.every((item) => item != null)) {
      const format = key === "Size" ? (item: number) => String(item) : (item: number) => item.toFixed(1);
      return `[${numbers.map((item) => format(item!)).join(", ")}]`;
    }
  }
  return undefined;
}

function resolvedLength(store: PdfObjectStore, objectNumber: number | undefined, stream: PdfStream): number | undefined {
  const length = resolvedNumber(store, stream.dict.get("Length"));
  if (length != null) return length;
  if (objectNumber != null) {
    const direct = store.getRawObjectText(objectNumber)?.match(/\/Length\s+(\d+)\b(?!\s+\d+\s+R)/)?.[1];
    if (direct) return Number(direct);
  }
  return stream.rawData.length;
}

function pdfStreamRepr(store: PdfObjectStore, objectNumber: number | undefined, stream: PdfStream): string {
  const raw = resolvedLength(store, objectNumber, stream) ?? decodeStreamToLatin1(stream).length;
  const attrs: string[] = [];
  const allowedKeys = new Set(["BitsPerSample", "Domain", "Length", "N", "Alternate", "Filter", "FunctionType", "Order", "Range", "Size"]);
  const keys = Array.from(stream.dict.keys()).filter((key) => allowedKeys.has(key));
  for (const key of keys) {
    const value = pdfValueRepr(store, stream.dict.get(key), key);
    if (value !== undefined) attrs.push(`'${key}': ${typeof value === "string" ? value : String(value)}`);
  }
  return `<PDFStream(${objectNumber ?? "?"}): raw=${raw}, {${attrs.join(", ")}}>`;
}

function cleanPdfBytesLikePython(value: string): string {
  const bytes = Uint8Array.from([...value].map((char) => char.charCodeAt(0) & 0xff));
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
  }
}

function colorSpaceArrayRepr(store: PdfObjectStore, array: PdfArray): unknown[] {
  return array.map((value) => {
    if (isRef(value)) {
      const object = store.getObject(value.objectNumber);
      if (object?.stream) return pdfStreamRepr(store, value.objectNumber, object.stream);
      if (isArray(object?.value)) return colorSpaceArrayRepr(store, object.value);
      if (object?.value instanceof Map) {
        const process = resolvedDict(store, object.value.get("Process"));
        const components = resolvedArray(store, process?.get("Components"));
        if (components) return { Process: { Components: colorSpaceArrayRepr(store, components) } };
      }
      if (isName(object?.value)) return `/'${object.value.name}'`;
      return object?.raw.replace(/^\s*\d+\s+\d+\s+obj\s*/, "").replace(/\s*endobj\s*$/, "").trim();
    }
    if (isName(value)) return `/'${value.name}'`;
    if (typeof value === "number") return value;
    if (isArray(value)) return colorSpaceArrayRepr(store, value);
    if (typeof value === "string") return cleanPdfBytesLikePython(value);
    return value;
  });
}

function imageColorSpace(store: PdfObjectStore, value: PdfPrimitive | undefined, fromRef = false): unknown[] | undefined {
  if (isRef(value)) return imageColorSpace(store, resolveOne(store, value), true);
  if (isName(value)) return [`/'${value.name}'`];
  if (isArray(value)) {
    const repr = colorSpaceArrayRepr(store, value);
    return fromRef ? [repr] : repr;
  }
  return undefined;
}

function dictNumberAny(store: PdfObjectStore, dict: PdfDict, keys: string[]): number | undefined {
  for (const key of keys) {
    const value = resolvedNumber(store, dict.get(key));
    if (value != null) return value;
  }
  return undefined;
}

function dictBooleanAny(store: PdfObjectStore, dict: PdfDict, keys: string[]): boolean | undefined {
  for (const key of keys) {
    const value = resolvedBoolean(store, dict.get(key));
    if (value != null) return value;
  }
  return undefined;
}

function imageColorSpaceAny(store: PdfObjectStore, dict: PdfDict, keys: string[]): unknown[] | undefined {
  for (const key of keys) {
    const value = imageColorSpace(store, dict.get(key));
    if (value) return value;
  }
  return undefined;
}

function imageResourceFromDict(store: PdfObjectStore, name: string, dict: PdfDict): ImageResource {
  return {
    name,
    width: dictNumberAny(store, dict, ["Width", "W"]),
    height: dictNumberAny(store, dict, ["Height", "H"]),
    bits: dictNumberAny(store, dict, ["BitsPerComponent", "BPC"]) ?? 1,
    colorspace: imageColorSpaceAny(store, dict, ["ColorSpace", "CS"]) ?? [null],
    imagemask: dictBooleanAny(store, dict, ["ImageMask", "IM"]) ?? null
  };
}

function imageResource(store: PdfObjectStore, name: string, stream: PdfStream): ImageResource {
  return imageResourceFromDict(store, name, stream.dict);
}

function inlineImageKey(name: string): string {
  const aliases: Record<string, string> = {
    BPC: "BPC",
    CS: "CS",
    H: "H",
    IM: "IM",
    W: "W",
    BitsPerComponent: "BitsPerComponent",
    ColorSpace: "ColorSpace",
    Height: "Height",
    ImageMask: "ImageMask",
    Width: "Width"
  };
  return aliases[name] ?? name;
}

function tokenPrimitive(tokens: PdfToken[], index: number): { value: PdfPrimitive; next: number } | null {
  const token = tokens[index];
  if (!token) return null;
  if (token.type === "number") return { value: token.value, next: index + 1 };
  if (token.type === "name") return { value: token.value, next: index + 1 };
  if (token.type === "string" || token.type === "hexString") return { value: token.value, next: index + 1 };
  if (token.type === "keyword" && token.value === "true") return { value: true, next: index + 1 };
  if (token.type === "keyword" && token.value === "false") return { value: false, next: index + 1 };
  if (token.type !== "arrayStart") return null;
  const values: PdfArray = [];
  let cursor = index + 1;
  while (cursor < tokens.length && tokens[cursor]?.type !== "arrayEnd") {
    const item = tokenPrimitive(tokens, cursor);
    if (!item) {
      cursor += 1;
      continue;
    }
    values.push(item.value);
    cursor = item.next;
  }
  return { value: values, next: tokens[cursor]?.type === "arrayEnd" ? cursor + 1 : cursor };
}

function findInlineImages(store: PdfObjectStore, content: string): Array<{ start: number; end: number; image: ImageResource }> {
  const tokens = tokenizePdf(content);
  const images: Array<{ start: number; end: number; image: ImageResource }> = [];
  for (let i = 0; i < tokens.length; i += 1) {
    const token = tokens[i];
    if (token.type !== "keyword" || token.value !== "BI") continue;
    const dict: PdfDict = new Map();
    let cursor = i + 1;
    while (cursor < tokens.length) {
      const current = tokens[cursor];
      if (current.type === "keyword" && current.value === "ID") break;
      if (current.type !== "name") {
        cursor += 1;
        continue;
      }
      const value = tokenPrimitive(tokens, cursor + 1);
      if (!value) {
        cursor += 1;
        continue;
      }
      dict.set(inlineImageKey(current.value.name), value.value);
      cursor = value.next;
    }
    const hasWidth = dict.has("W") || dict.has("Width");
    const hasHeight = dict.has("H") || dict.has("Height");
    if (hasWidth && hasHeight) {
      images.push({
        start: token.start,
        end: tokens[cursor]?.end ?? token.end,
        image: imageResourceFromDict(store, `inline-image-${images.length + 1}`, dict)
      });
    }
  }
  return images;
}

function collectDrawnImages(
  store: PdfObjectStore,
  content: string,
  resources: PdfDict | undefined,
  active = new Set<number>(),
  depth = 0
): ImageResource[] {
  if (depth > 8) return [];
  const xobjects = xObjectMap(store, resources);
  const images: ImageResource[] = [];
  const imageOps = [
    ...findDrawOperations(content).map((draw) => ({ ...draw, kind: "xobject" as const })),
    ...findInlineImages(store, content).map((draw) => ({ ...draw, kind: "inline" as const }))
  ].sort((a, b) => a.start - b.start);
  for (const draw of imageOps) {
    if (draw.kind === "inline") {
      images.push(draw.image);
      continue;
    }
    const value = xobjects.get(draw.name);
    const target = resolveToStream(store, value);
    if (!target) {
      images.push({ name: draw.name });
      continue;
    }
    const subtype = asName(target.stream.dict.get("Subtype"));
    if (subtype === "Form") {
      if (active.has(target.objectNumber ?? -1)) continue;
      const nextActive = new Set(active);
      if (target.objectNumber != null) nextActive.add(target.objectNumber);
      images.push(...collectDrawnImages(store, decodeStreamToLatin1(target.stream), formResources(store, target.stream, resources), nextActive, depth + 1));
    } else if (subtype === "Image") {
      images.push(imageResource(store, draw.name, target.stream));
    }
  }
  return images;
}

function collectResourceImages(store: PdfObjectStore, resources: PdfDict | undefined, out: ImageResource[], seen = new Set<number>()): void {
  for (const [name, value] of xObjectMap(store, resources)) {
    const target = resolveToStream(store, value);
    if (!target) continue;
    if (target.objectNumber != null && seen.has(target.objectNumber)) continue;
    if (target.objectNumber != null) seen.add(target.objectNumber);
    const subtype = asName(target.stream.dict.get("Subtype"));
    if (subtype === "Image") out.push(imageResource(store, name, target.stream));
    else if (subtype === "Form") collectResourceImages(store, formResources(store, target.stream, resources), out, seen);
  }
}

export function parseImageResources(owner: PdfResourceOwner, context: PdfResourceContext, pageContent = ""): ImageResource[] {
  const store = toStore(context);
  const resources = effectivePageResources(store, owner);
  const drawn = collectDrawnImages(store, decodedContents(store, owner), resources);
  if (drawn.length) return drawn;
  const resourcesImages: ImageResource[] = [];
  collectResourceImages(store, resources, resourcesImages);
  const drawNames = Array.from(new Set(findDrawOperations(pageContent).map((draw) => draw.name)));
  if (drawNames.length) {
    const byName = new Map(resourcesImages.map((image) => [image.name, image]));
    return drawNames.map((name) => byName.get(name) ?? { name });
  }
  return resourcesImages;
}

function colorSpacePrimitiveName(store: PdfObjectStore, value: PdfPrimitive | undefined): string | undefined {
  const resolved = resolveOne(store, value);
  if (isName(resolved)) return resolved.name;
  if (isArray(resolved)) {
    const first = resolveOne(store, resolved[0]);
    if (isName(first)) return first.name;
  }
  if (isStream(resolved)) return asName(resolved.dict.get("CMapName"));
  return undefined;
}

function collectColorSpaceResourcesFrom(
  store: PdfObjectStore,
  resources: PdfDict | undefined,
  out: Record<string, string>,
  seenForms = new Set<number>()
): void {
  const colorSpaces = resourceSubdict(store, resources, "ColorSpace");
  if (colorSpaces) {
    for (const [name, spec] of colorSpaces) out[name] = colorSpaceName(colorSpacePrimitiveName(store, spec) ?? name);
  }
  for (const value of xObjectMap(store, resources).values()) {
    const target = resolveToStream(store, value);
    if (!target || asName(target.stream.dict.get("Subtype")) !== "Form") continue;
    if (target.objectNumber != null && seenForms.has(target.objectNumber)) continue;
    if (target.objectNumber != null) seenForms.add(target.objectNumber);
    collectColorSpaceResourcesFrom(store, formResources(store, target.stream, resources), out, seenForms);
  }
}

export function parseColorSpaceResources(owner: PdfResourceOwner, context: PdfResourceContext): Record<string, string> {
  const store = toStore(context);
  const out: Record<string, string> = {};
  collectColorSpaceResourcesFrom(store, effectivePageResources(store, owner), out);
  return out;
}

function primitiveArrayNumbers(store: PdfObjectStore, value: PdfPrimitive | undefined): number[] {
  return (resolvedArray(store, value) ?? []).flatMap((item) => {
    const number = resolvedNumber(store, item);
    return number == null ? [] : [number];
  });
}

function parseEncodingDifferences(store: PdfObjectStore, encoding: PdfPrimitive | undefined): Record<number, string> | undefined {
  const dict = resolvedDict(store, encoding);
  const differences = resolvedArray(store, dict?.get("Differences"));
  if (!differences) return undefined;
  const out: Record<number, string> = {};
  let code: number | null = null;
  for (const value of differences) {
    const resolved = resolveOne(store, value);
    if (typeof resolved === "number") {
      code = resolved;
      continue;
    }
    if (code == null || !isName(resolved)) continue;
    out[code] = resolved.name;
    code += 1;
  }
  return out;
}

function firstFontDescriptor(store: PdfObjectStore, fontDict: PdfDict, descendant: PdfDict | undefined): PdfDict | undefined {
  return resolvedDict(store, fontDict.get("FontDescriptor")) ?? resolvedDict(store, descendant?.get("FontDescriptor"));
}

function firstDescendantFont(store: PdfObjectStore, fontDict: PdfDict): PdfDict | undefined {
  const descendants = resolvedArray(store, fontDict.get("DescendantFonts"));
  if (!descendants?.length) return undefined;
  const first = resolveOne(store, descendants[0]);
  if (first instanceof Map) return first;
  if (isArray(first) && first[0]) return resolvedDict(store, first[0]);
  return undefined;
}

function cidCodingName(store: PdfObjectStore, descendant: PdfDict | undefined): string | undefined {
  const systemInfo = resolvedDict(store, descendant?.get("CIDSystemInfo"));
  const registry = primitiveNameOrString(store, systemInfo?.get("Registry"))?.trim();
  const ordering = primitiveNameOrString(store, systemInfo?.get("Ordering"))?.trim();
  return registry && ordering ? `${registry}-${ordering}` : undefined;
}

function charSetNames(value: PdfPrimitive | undefined): string[] | undefined {
  if (typeof value !== "string") return undefined;
  const names = Array.from(value.matchAll(/\/([^/\s()]+)/g), (match) => decodePdfName(match[1]));
  return names.length ? names : undefined;
}

function fontEncodingName(store: PdfObjectStore, fontDict: PdfDict): string | undefined {
  const encoding = resolveOne(store, fontDict.get("Encoding"));
  if (isName(encoding)) return normalizeIdentityCMapNameLikePdfminer(encoding.name);
  if (encoding instanceof Map) return normalizeIdentityCMapNameLikePdfminer(primitiveNameOrString(store, encoding.get("CMapName")));
  if (isStream(encoding)) return normalizeIdentityCMapNameLikePdfminer(primitiveNameOrString(store, encoding.dict.get("CMapName")));
  return undefined;
}

function hasToUnicode(store: PdfObjectStore, fontDict: PdfDict): boolean {
  const value = fontDict.get("ToUnicode");
  return isRef(value) || isStream(resolveOne(store, value));
}

function uint16(bytes: Uint8Array, offset: number): number {
  return (bytes[offset] << 8) | bytes[offset + 1];
}

function int16(bytes: Uint8Array, offset: number): number {
  const value = uint16(bytes, offset);
  return value & 0x8000 ? value - 0x10000 : value;
}

function uint32(bytes: Uint8Array, offset: number): number {
  return ((bytes[offset] << 24) >>> 0) + (bytes[offset + 1] << 16) + (bytes[offset + 2] << 8) + bytes[offset + 3];
}

function tableOffset(bytes: Uint8Array, tag: string): number | null {
  if (bytes.length < 12) return null;
  const numTables = uint16(bytes, 4);
  for (let i = 0; i < numTables; i += 1) {
    const offset = 12 + i * 16;
    const current = String.fromCharCode(bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]);
    if (current === tag) return uint32(bytes, offset + 8);
  }
  return null;
}

function invertFormat4CMap(bytes: Uint8Array, offset: number): Record<number, string> {
  const length = uint16(bytes, offset + 2);
  const end = Math.min(bytes.length, offset + length);
  const segCount = uint16(bytes, offset + 6) / 2;
  const endCodes = offset + 14;
  const startCodes = endCodes + segCount * 2 + 2;
  const idDeltas = startCodes + segCount * 2;
  const idRangeOffsets = idDeltas + segCount * 2;
  const out: Record<number, string> = {};
  for (let segment = 0; segment < segCount; segment += 1) {
    const start = uint16(bytes, startCodes + segment * 2);
    const stop = uint16(bytes, endCodes + segment * 2);
    const delta = int16(bytes, idDeltas + segment * 2);
    const rangeOffset = uint16(bytes, idRangeOffsets + segment * 2);
    if (start === 0xffff && stop === 0xffff) continue;
    for (let code = start; code <= stop; code += 1) {
      let glyph = 0;
      if (rangeOffset === 0) {
        glyph = (code + delta) & 0xffff;
      } else {
        const glyphOffset = idRangeOffsets + segment * 2 + rangeOffset + (code - start) * 2;
        if (glyphOffset + 1 >= end) continue;
        const raw = uint16(bytes, glyphOffset);
        glyph = raw === 0 ? 0 : (raw + delta) & 0xffff;
      }
      if (glyph !== 0 && out[glyph] == null) out[glyph] = String.fromCodePoint(code);
    }
  }
  return out;
}

function invertFormat12CMap(bytes: Uint8Array, offset: number): Record<number, string> {
  const groups = uint32(bytes, offset + 12);
  const out: Record<number, string> = {};
  for (let group = 0; group < groups; group += 1) {
    const groupOffset = offset + 16 + group * 12;
    if (groupOffset + 11 >= bytes.length) break;
    const startChar = uint32(bytes, groupOffset);
    const endChar = uint32(bytes, groupOffset + 4);
    const startGlyph = uint32(bytes, groupOffset + 8);
    for (let code = startChar; code <= endChar; code += 1) {
      const glyph = startGlyph + code - startChar;
      if (glyph !== 0 && out[glyph] == null) out[glyph] = String.fromCodePoint(code);
    }
  }
  return out;
}

function trueTypeGlyphUnicodeMap(bytes: Uint8Array): Record<number, string> | undefined {
  const cmap = tableOffset(bytes, "cmap");
  if (cmap == null || cmap + 4 > bytes.length) return undefined;
  const records = uint16(bytes, cmap + 2);
  const subtables: Array<{ score: number; offset: number; format: number }> = [];
  for (let i = 0; i < records; i += 1) {
    const record = cmap + 4 + i * 8;
    if (record + 7 >= bytes.length) break;
    const platform = uint16(bytes, record);
    const encoding = uint16(bytes, record + 2);
    const subtable = cmap + uint32(bytes, record + 4);
    if (subtable + 1 >= bytes.length) continue;
    const format = uint16(bytes, subtable);
    if (format !== 4 && format !== 12) continue;
    const score = (platform === 3 && encoding === 10 ? 0 : platform === 3 && encoding === 1 ? 1 : platform === 0 ? 2 : 3) + (format === 12 ? 0 : 0.5);
    subtables.push({ score, offset: subtable, format });
  }
  subtables.sort((a, b) => a.score - b.score);
  for (const subtable of subtables) {
    const map = subtable.format === 12 ? invertFormat12CMap(bytes, subtable.offset) : invertFormat4CMap(bytes, subtable.offset);
    if (Object.keys(map).length) return map;
  }
  return undefined;
}

function embeddedUnicodeMap(store: PdfObjectStore, descriptor: PdfDict | undefined): Record<number, string> | undefined {
  const fontFile = resolveToStream(store, descriptor?.get("FontFile2")) ?? resolveToStream(store, descriptor?.get("FontFile"));
  if (!fontFile) return undefined;
  return trueTypeGlyphUnicodeMap(decodeStream(fontFile.stream));
}

function hexToCode(value: string): number {
  return Number.parseInt(value.replace(/\s+/g, ""), 16);
}

function hexToUtf16Text(value: string): string {
  const hex = value.replace(/\s+/g, "");
  let out = "";
  for (let i = 0; i < hex.length - 3; i += 4) {
    out += String.fromCharCode(Number.parseInt(hex.slice(i, i + 4), 16));
  }
  return out;
}

function incrementUtf16Text(value: string, offset: number): string {
  if (!value.length || offset === 0) return value;
  const chars = [...value];
  const last = chars[chars.length - 1].charCodeAt(0) + offset;
  chars[chars.length - 1] = String.fromCharCode(last);
  return chars.join("");
}

function parseToUnicodeMap(store: PdfObjectStore, fontDict: PdfDict): Record<number, string> | undefined {
  const stream = resolveToStream(store, fontDict.get("ToUnicode"));
  if (!stream) return undefined;
  const text = decodeStreamToLatin1(stream.stream);
  const out: Record<number, string> = {};

  for (const block of text.matchAll(/beginbfchar([\s\S]*?)endbfchar/g)) {
    for (const match of block[1].matchAll(/<([0-9A-Fa-f\s]+)>\s*<([0-9A-Fa-f\s]+)>/g)) {
      out[hexToCode(match[1])] = hexToUtf16Text(match[2]);
    }
  }

  for (const block of text.matchAll(/beginbfrange([\s\S]*?)endbfrange/g)) {
    for (const match of block[1].matchAll(/<([0-9A-Fa-f\s]+)>\s*<([0-9A-Fa-f\s]+)>\s*<([0-9A-Fa-f\s]+)>/g)) {
      const start = hexToCode(match[1]);
      const end = hexToCode(match[2]);
      const target = hexToUtf16Text(match[3]);
      for (let code = start; code <= end; code += 1) out[code] = incrementUtf16Text(target, code - start);
    }

    for (const match of block[1].matchAll(/<([0-9A-Fa-f\s]+)>\s*<([0-9A-Fa-f\s]+)>\s*\[([\s\S]*?)\]/g)) {
      const start = hexToCode(match[1]);
      const values = Array.from(match[3].matchAll(/<([0-9A-Fa-f\s]+)>/g), (item) => hexToUtf16Text(item[1]));
      values.forEach((value, index) => {
        out[start + index] = value;
      });
    }
  }

  return Object.keys(out).length ? out : undefined;
}

export function parseFontRecords(context: PdfResourceContext): FontRecord[] {
  const store = toStore(context);
  const fonts: FontRecord[] = [];
  for (const objectNumber of rawObjects(context).keys()) {
    const object = store.getObject(objectNumber);
    const fontDict = asDict(object?.value);
    if (!fontDict || asName(fontDict.get("Type")) !== "Font") continue;
    const baseFont = primitiveName(store, fontDict.get("BaseFont"));
    if (!baseFont) continue;
    const subtype = primitiveName(store, fontDict.get("Subtype"));
    const descendant = firstDescendantFont(store, fontDict);
    const descriptor = firstFontDescriptor(store, fontDict, descendant);
    const encodingName = fontEncodingName(store, fontDict);
    const flags = resolvedNumber(store, descriptor?.get("Flags"));
    const ascent = resolvedNumber(store, descriptor?.get("Ascent"));
    const descent = resolvedNumber(store, descriptor?.get("Descent"));
    const encodingDifferences = parseEncodingDifferences(store, fontDict.get("Encoding"));
    const firstChar = resolvedNumber(store, fontDict.get("FirstChar")) ?? 0;
    const unicodeMap = parseToUnicodeMap(store, fontDict) ?? embeddedUnicodeMap(store, descriptor);
    fonts.push({
      objectNumber,
      baseFont,
      subtype,
      cidCoding: cidCodingName(store, descendant),
      encodingName,
      hasToUnicode: hasToUnicode(store, fontDict),
      symbolic: flags == null ? undefined : (flags & 4) !== 0,
      charSet: charSetNames(resolveOne(store, descriptor?.get("CharSet"))),
      encodingDifferences,
      embeddedUnicodeMap: unicodeMap,
      firstChar,
      widths: primitiveArrayNumbers(store, fontDict.get("Widths")),
      ascent: ascent == null ? undefined : ascent / FONT_UNITS_PER_EM,
      descent: descent == null ? undefined : -Math.abs(descent / FONT_UNITS_PER_EM),
      vertical: isVerticalCMapNameLikePdfminer(encodingName)
    });
  }
  return fonts;
}

export function parsePageFontResourceMap(owner: PdfResourceOwner, context: PdfResourceContext): Map<string, number> {
  const store = toStore(context);
  const fontDict = resourceSubdict(store, effectivePageResources(store, owner), "Font");
  const out = new Map<string, number>();
  if (!fontDict) return out;
  for (const [name, value] of fontDict) {
    if (isRef(value)) out.set(name, value.objectNumber);
  }
  return out;
}

function parseDirectFontRefs(pageObjectText: string | undefined): number[] {
  const fontDict = pageObjectText?.match(/\/Font\s*<<([\s\S]*?)>>/)?.[1];
  if (!fontDict) return [];
  return Array.from(fontDict.matchAll(/\/[^\s/<>[\]()]+\s+(\d+)\s+\d+\s+R/g), (match) => Number(match[1]));
}

export function parsePageFontObjectNumbers(owner: PdfResourceOwner, context?: PdfResourceContext): number[] {
  if (!context) return typeof owner === "string" ? parseDirectFontRefs(owner) : [];
  return Array.from(new Set(parsePageFontResourceMap(owner, context).values()));
}
