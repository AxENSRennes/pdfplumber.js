import type { BBox, PDFObject } from "./types.js";
import { cleanObject, clusterObjectsSimple, objectsToBBox } from "./utils.js";

export function applyLimitSlice(words: PDFObject[], options: Record<string, unknown>): PDFObject[] {
  let out = words;
  if (Array.isArray(options.slice)) {
    const [start, end] = options.slice.map(Number);
    out = out.slice(start, end);
  }
  if (typeof options.limit === "number") out = out.slice(0, options.limit);
  return out;
}

export function layoutSortChars(chars: PDFObject[]): PDFObject[] {
  return [...chars].sort((a, b) => Number(a.top) - Number(b.top) || Number(a.x0) - Number(b.x0));
}

export function layoutClusterSortChars(chars: PDFObject[]): PDFObject[] {
  const lineKey = (char: PDFObject): number => {
    const matrix = char.matrix;
    return char.upright !== false && Array.isArray(matrix) && Number.isFinite(Number(matrix[5])) ? -Number(matrix[5]) : Number(char.top);
  };
  return clusterObjectsSimple([...chars].sort((a, b) => lineKey(a) - lineKey(b) || Number(a.x0) - Number(b.x0)), lineKey, 3).flatMap((cluster) =>
    [...cluster].sort((a, b) => Number(a.x0) - Number(b.x0) || Number(a.top) - Number(b.top))
  );
}

export function layoutVerticalSortChars(chars: PDFObject[]): PDFObject[] {
  const columnKey = (char: PDFObject): number => -Number(char.x1 ?? char.x0 ?? 0);
  return clusterObjectsSimple([...chars].sort((a, b) => columnKey(a) - columnKey(b) || Number(a.top) - Number(b.top)), columnKey, 1).flatMap((cluster) =>
    [...cluster].sort((a, b) => Number(b.top) - Number(a.top) || Number(b.x1) - Number(a.x1))
  );
}

function objectFromChars(chars: PDFObject[], objectType: string, pageHeight: number, doctopOffset: number, text?: string): PDFObject {
  const [x0, top, x1, bottom] = objectsToBBox(chars);
  return cleanObject({
    object_type: objectType,
    page_number: chars[0]?.page_number ?? 0,
    text: text ?? chars.map((char) => char.text ?? "").join("") + "\n",
    x0,
    x1,
    y0: pageHeight - bottom,
    y1: pageHeight - top,
    top,
    bottom,
    doctop: top + doctopOffset,
    width: x1 - x0,
    height: bottom - top
  });
}

export type LayoutLineLikePdfminerType = "textlinehorizontal" | "textlinevertical";

export interface LayoutLineLikePdfminer {
  type: LayoutLineLikePdfminerType;
  obj: PDFObject;
}

type LayoutLineType = LayoutLineLikePdfminerType;

interface LayoutLine extends LayoutLineLikePdfminer {
  chars: PDFObject[];
  text: string;
}

interface LayoutBox {
  type: "textboxhorizontal" | "textboxvertical";
  lines: LayoutLine[];
  obj: PDFObject;
}

function laparamNumber(laparams: Record<string, unknown>, key: string, fallback: number): number {
  const value = Number(laparams[key] ?? fallback);
  return Number.isFinite(value) ? value : fallback;
}

function horizontalDistance(a: PDFObject, b: PDFObject): number {
  if (Number(b.x0) <= Number(a.x1) && Number(a.x0) <= Number(b.x1)) return 0;
  return Math.min(Math.abs(Number(a.x0) - Number(b.x1)), Math.abs(Number(a.x1) - Number(b.x0)));
}

function verticalDistance(a: PDFObject, b: PDFObject): number {
  if (Number(b.y0) <= Number(a.y1) && Number(a.y0) <= Number(b.y1)) return 0;
  return Math.min(Math.abs(Number(a.y0) - Number(b.y1)), Math.abs(Number(a.y1) - Number(b.y0)));
}

function horizontalOverlap(a: PDFObject, b: PDFObject): number {
  if (!(Number(b.x0) <= Number(a.x1) && Number(a.x0) <= Number(b.x1))) return 0;
  return Math.min(Math.abs(Number(a.x0) - Number(b.x1)), Math.abs(Number(a.x1) - Number(b.x0)));
}

function verticalOverlap(a: PDFObject, b: PDFObject): number {
  if (!(Number(b.y0) <= Number(a.y1) && Number(a.y0) <= Number(b.y1))) return 0;
  return Math.min(Math.abs(Number(a.y0) - Number(b.y1)), Math.abs(Number(a.y1) - Number(b.y0)));
}

function buildLineText(chars: PDFObject[], type: LayoutLineType, wordMargin: number): string {
  let text = "";
  let boundary = type === "textlinehorizontal" ? Number.POSITIVE_INFINITY : Number.NEGATIVE_INFINITY;
  for (const char of chars) {
    const margin = wordMargin * Math.max(Number(char.width), Number(char.height));
    if (type === "textlinehorizontal") {
      if (boundary < Number(char.x0) - margin) text += " ";
      boundary = Number(char.x1);
    } else {
      if (Number(char.y1) + margin < boundary) text += " ";
      boundary = Number(char.y0);
    }
    text += String(char.text ?? "");
  }
  return `${text}\n`;
}

function makeLayoutLine(chars: PDFObject[], type: LayoutLineType, pageHeight: number, doctopOffset: number, wordMargin: number): LayoutLine {
  const text = buildLineText(chars, type, wordMargin);
  return {
    type,
    chars,
    text,
    obj: objectFromChars(chars, type, pageHeight, doctopOffset, text)
  };
}

function groupLayoutChars(chars: PDFObject[], pageHeight: number, doctopOffset: number, laparams: Record<string, unknown>): LayoutLine[] {
  const lineOverlap = laparamNumber(laparams, "line_overlap", 0.5);
  const charMargin = laparamNumber(laparams, "char_margin", 2);
  const wordMargin = laparamNumber(laparams, "word_margin", 0.1);
  const detectVertical = Boolean(laparams.detect_vertical);
  const lines: LayoutLine[] = [];
  let previous: PDFObject | null = null;
  let current: { type: LayoutLineType; chars: PDFObject[] } | null = null;

  for (const char of chars) {
    if (previous) {
      const halign =
        verticalOverlap(previous, char) > Math.min(Number(previous.height), Number(char.height)) * lineOverlap &&
        horizontalDistance(previous, char) < Math.max(Number(previous.width), Number(char.width)) * charMargin;
      const valign =
        detectVertical &&
        horizontalOverlap(previous, char) > Math.min(Number(previous.width), Number(char.width)) * lineOverlap &&
        verticalDistance(previous, char) < Math.max(Number(previous.height), Number(char.height)) * charMargin;

      if ((halign && current?.type === "textlinehorizontal") || (valign && current?.type === "textlinevertical")) {
        current.chars.push(char);
      } else if (current) {
        lines.push(makeLayoutLine(current.chars, current.type, pageHeight, doctopOffset, wordMargin));
        current = null;
      } else if (valign && !halign) {
        current = { type: "textlinevertical", chars: [previous, char] };
      } else if (halign && !valign) {
        current = { type: "textlinehorizontal", chars: [previous, char] };
      } else {
        lines.push(makeLayoutLine([previous], "textlinehorizontal", pageHeight, doctopOffset, wordMargin));
      }
    }
    previous = char;
  }

  if (previous) {
    if (current) lines.push(makeLayoutLine(current.chars, current.type, pageHeight, doctopOffset, wordMargin));
    else lines.push(makeLayoutLine([previous], "textlinehorizontal", pageHeight, doctopOffset, wordMargin));
  }
  return lines;
}

function lineObjectsOverlap(a: PDFObject, b: PDFObject): boolean {
  return Number(b.x0) <= Number(a.x1) && Number(a.x0) <= Number(b.x1) && Number(b.y0) <= Number(a.y1) && Number(a.y0) <= Number(b.y1);
}

function objectHeight(obj: PDFObject): number {
  return Number(obj.y1) - Number(obj.y0);
}

function objectWidth(obj: PDFObject): number {
  return Number(obj.x1) - Number(obj.x0);
}

export function findLineNeighborsLikePdfminer<T extends LayoutLineLikePdfminer>(line: T, lines: T[], ratio: number): T[] {
  const obj = line.obj;
  if (line.type === "textlinehorizontal") {
    const height = objectHeight(obj);
    const d = ratio * height;
    const query = { x0: obj.x0, x1: obj.x1, y0: Number(obj.y0) - d, y1: Number(obj.y1) + d } as PDFObject;
    return lines.filter((other) => {
      const candidate = other.obj;
      if (other.type !== "textlinehorizontal" || !lineObjectsOverlap(candidate, query)) return false;
      return (
        Math.abs(objectHeight(candidate) - height) <= d &&
        (Math.abs(Number(candidate.x0) - Number(obj.x0)) <= d ||
          Math.abs(Number(candidate.x1) - Number(obj.x1)) <= d ||
          Math.abs((Number(candidate.x0) + Number(candidate.x1)) / 2 - (Number(obj.x0) + Number(obj.x1)) / 2) <= d)
      );
    });
  }

  const width = objectWidth(obj);
  const d = ratio * width;
  const query = { x0: Number(obj.x0) - d, x1: Number(obj.x1) + d, y0: obj.y0, y1: obj.y1 } as PDFObject;
  return lines.filter((other) => {
    const candidate = other.obj;
    if (other.type !== "textlinevertical" || !lineObjectsOverlap(candidate, query)) return false;
    return (
      Math.abs(objectWidth(candidate) - width) <= d &&
      (Math.abs(Number(candidate.y0) - Number(obj.y0)) <= d ||
        Math.abs(Number(candidate.y1) - Number(obj.y1)) <= d ||
        Math.abs((Number(candidate.y0) + Number(candidate.y1)) / 2 - (Number(obj.y0) + Number(obj.y1)) / 2) <= d)
    );
  });
}

function makeLayoutBox(lines: LayoutLine[], type: "textboxhorizontal" | "textboxvertical", pageHeight: number, doctopOffset: number): LayoutBox {
  const ordered =
    type === "textboxhorizontal" ? [...lines].sort((a, b) => Number(a.obj.top) - Number(b.obj.top)) : [...lines].sort((a, b) => Number(b.obj.x1) - Number(a.obj.x1));
  const chars = ordered.flatMap((line) => line.chars);
  const text = ordered.map((line) => line.text).join("");
  return {
    type,
    lines: ordered,
    obj: objectFromChars(chars, type, pageHeight, doctopOffset, text)
  };
}

function groupLayoutLines(lines: LayoutLine[], pageHeight: number, doctopOffset: number, laparams: Record<string, unknown>): LayoutBox[] {
  const lineMargin = laparamNumber(laparams, "line_margin", 0.5);
  const boxesByLine = new Map<LayoutLine, LayoutBox>();

  for (const line of lines) {
    const members: LayoutLine[] = [line];
    for (const neighbor of findLineNeighborsLikePdfminer(line, lines, lineMargin)) {
      members.push(neighbor);
      const existing = boxesByLine.get(neighbor);
      if (existing) {
        for (const existingLine of existing.lines) members.push(existingLine);
      }
    }
    const unique = Array.from(new Set(members));
    const type = line.type === "textlinevertical" ? "textboxvertical" : "textboxhorizontal";
    const box = makeLayoutBox(unique, type, pageHeight, doctopOffset);
    for (const member of unique) boxesByLine.set(member, box);
  }

  const boxes: LayoutBox[] = [];
  const seen = new Set<LayoutBox>();
  for (const line of lines) {
    const box = boxesByLine.get(line);
    if (box && !seen.has(box) && String(box.obj.text ?? "").trim()) {
      seen.add(box);
      boxes.push(box);
    }
  }
  return boxes;
}

function sortLayoutBoxes(boxes: LayoutBox[], laparams: Record<string, unknown>): LayoutBox[] {
  const boxesFlowValue = laparams.boxes_flow;
  if (boxesFlowValue === null) {
    return [...boxes].sort((a, b) => {
      if (a.type !== b.type) return a.type === "textboxvertical" ? -1 : 1;
      if (a.type === "textboxvertical") return Number(b.obj.x1) - Number(a.obj.x1) || Number(b.obj.y0) - Number(a.obj.y0);
      return Number(b.obj.y0) - Number(a.obj.y0) || Number(a.obj.x0) - Number(b.obj.x0);
    });
  }

  const boxesFlow = laparamNumber(laparams, "boxes_flow", 0.5);
  type LayoutNode = {
    id: number;
    kind: "box" | "group";
    groupType?: "lrtb" | "tbrl";
    box?: LayoutBox;
    children?: LayoutNode[];
    x0: number;
    y0: number;
    x1: number;
    y1: number;
    width: number;
    height: number;
  };

  let nextId = 1;
  const nodeFromBox = (box: LayoutBox): LayoutNode => {
    const x0 = Number(box.obj.x0);
    const y0 = Number(box.obj.y0);
    const x1 = Number(box.obj.x1);
    const y1 = Number(box.obj.y1);
    return { id: nextId++, kind: "box", box, x0, y0, x1, y1, width: x1 - x0, height: y1 - y0 };
  };
  const nodeFromChildren = (children: LayoutNode[], groupType: "lrtb" | "tbrl"): LayoutNode => {
    const x0 = Math.min(...children.map((child) => child.x0));
    const y0 = Math.min(...children.map((child) => child.y0));
    const x1 = Math.max(...children.map((child) => child.x1));
    const y1 = Math.max(...children.map((child) => child.y1));
    return { id: nextId++, kind: "group", groupType, children, x0, y0, x1, y1, width: x1 - x0, height: y1 - y0 };
  };
  const isTbrl = (node: LayoutNode): boolean => node.groupType === "tbrl" || node.box?.type === "textboxvertical";
  const distance = (a: LayoutNode, b: LayoutNode): number => {
    const x0 = Math.min(a.x0, b.x0);
    const y0 = Math.min(a.y0, b.y0);
    const x1 = Math.max(a.x1, b.x1);
    const y1 = Math.max(a.y1, b.y1);
    return (x1 - x0) * (y1 - y0) - a.width * a.height - b.width * b.height;
  };
  const overlaps = (node: LayoutNode, bbox: BBox): boolean => node.x1 >= bbox[0] && node.x0 <= bbox[2] && node.y1 >= bbox[1] && node.y0 <= bbox[3];
  const hasAnyBetween = (plane: LayoutNode[], a: LayoutNode, b: LayoutNode): boolean => {
    const bbox: BBox = [Math.min(a.x0, b.x0), Math.min(a.y0, b.y0), Math.max(a.x1, b.x1), Math.max(a.y1, b.y1)];
    return plane.some((node) => node !== a && node !== b && overlaps(node, bbox));
  };
  const analyze = (node: LayoutNode, out: LayoutBox[]): void => {
    if (node.kind === "box") {
      if (node.box) out.push(node.box);
      return;
    }
    const children = [...(node.children ?? [])];
    if (node.groupType === "tbrl") {
      children.sort((a, b) => (-(1 + boxesFlow) * (a.x0 + a.x1) - (1 - boxesFlow) * a.y1) - (-(1 + boxesFlow) * (b.x0 + b.x1) - (1 - boxesFlow) * b.y1));
    } else {
      children.sort((a, b) => ((1 - boxesFlow) * a.x0 - (1 + boxesFlow) * (a.y0 + a.y1)) - ((1 - boxesFlow) * b.x0 - (1 + boxesFlow) * (b.y0 + b.y1)));
    }
    for (const child of children) analyze(child, out);
  };

  const nodes = boxes.map(nodeFromBox);
  let plane = [...nodes];
  const queue: Array<{ skipIsAny: boolean; distance: number; a: LayoutNode; b: LayoutNode; order: number }> = [];
  let order = 0;
  for (let i = 0; i < nodes.length; i += 1) {
    for (let j = i + 1; j < nodes.length; j += 1) {
      queue.push({ skipIsAny: false, distance: distance(nodes[i], nodes[j]), a: nodes[i], b: nodes[j], order: order++ });
    }
  }

  while (queue.length) {
    queue.sort((a, b) => Number(a.skipIsAny) - Number(b.skipIsAny) || a.distance - b.distance || a.a.id - b.a.id || a.b.id - b.b.id || a.order - b.order);
    const item = queue.shift();
    if (!item || !plane.includes(item.a) || !plane.includes(item.b)) continue;
    if (!item.skipIsAny && hasAnyBetween(plane, item.a, item.b)) {
      queue.push({ ...item, skipIsAny: true, order: order++ });
      continue;
    }
    const group = nodeFromChildren([item.a, item.b], isTbrl(item.a) || isTbrl(item.b) ? "tbrl" : "lrtb");
    plane = plane.filter((node) => node !== item.a && node !== item.b);
    for (const other of plane) queue.push({ skipIsAny: false, distance: distance(group, other), a: group, b: other, order: order++ });
    plane.push(group);
  }

  const out: LayoutBox[] = [];
  for (const node of plane) analyze(node, out);
  return out;
}

export function buildLayoutObjects(chars: PDFObject[], images: PDFObject[], pageHeight: number, doctopOffset: number, laparams: Record<string, unknown>): { chars: PDFObject[]; objects: Record<string, PDFObject[]> } {
  const allLines = groupLayoutChars(chars, pageHeight, doctopOffset, laparams);
  const textLines = allLines.filter((line) => String(line.text).trim());
  const rawIndex = new Map<PDFObject, number>(chars.map((char, index) => [char, index]));
  const emptyLines = allLines
    .filter((line) => !String(line.text).trim())
    .sort((a, b) => (rawIndex.get(a.chars[0]) ?? 0) - (rawIndex.get(b.chars[0]) ?? 0));
  const boxes = sortLayoutBoxes(groupLayoutLines(textLines, pageHeight, doctopOffset, laparams), laparams);

  const horizontalBoxes = boxes.filter((box) => box.type === "textboxhorizontal");
  const verticalBoxes = boxes.filter((box) => box.type === "textboxvertical");
  const orderedLines = boxes.flatMap((box) => box.lines);
  const emptyChars = new Set(emptyLines.flatMap((line) => line.chars));
  const layoutChars = [...orderedLines.flatMap((line) => line.chars), ...chars.filter((char) => emptyChars.has(char))];
  const objectLines = [...orderedLines, ...emptyLines];

  return {
    chars: layoutChars.length === chars.length ? layoutChars : chars,
    objects: {
      textlinehorizontal: objectLines.filter((line) => line.type === "textlinehorizontal").map((line) => line.obj),
      textboxhorizontal: horizontalBoxes.map((box) => box.obj),
      textlinevertical: objectLines.filter((line) => line.type === "textlinevertical").map((line) => line.obj),
      textboxvertical: verticalBoxes.map((box) => box.obj),
      figure: images.map((image) => cleanObject({ ...image, object_type: "figure" }))
    }
  };
}
