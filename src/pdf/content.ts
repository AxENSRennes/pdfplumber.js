import type { ColorOp, Matrix, Point } from "../types.js";
import { cleanNumber, colorSpaceName } from "../utils.js";
import { asName, isName } from "./primitives.js";
import type { PdfOperation, PdfPrimitive } from "./primitives.js";
import { parseOperatorStream } from "./parser.js";

export interface GraphicsHints {
  colorOps: ColorOp[];
  lineWidths: number[];
  dashOps: Array<[unknown[], number]>;
  textMatrices: Matrix[];
  textMoves: Point[];
  leadingTextMoves: Point[];
  transforms: Matrix[];
  pathOps: number[][];
  drawnImages: string[];
}

export function extractContentOps(content: string): PdfOperation[] {
  return parseOperatorStream(content);
}

export function patternColorValueLikePdfminer(components: number[], pattern: string): unknown {
  if (components.length === 0) return pattern;
  const base = components.length === 1 ? cleanNumber(components[0]) : components.map(cleanNumber);
  return [base, pattern];
}

function numericArgs(args: PdfPrimitive[]): number[] {
  return args.filter((arg): arg is number => typeof arg === "number" && Number.isFinite(arg));
}

function lastName(args: PdfPrimitive[]): string | undefined {
  return [...args].reverse().find(isName)?.name;
}

function isUpperOperator(operator: string): boolean {
  return operator === operator.toUpperCase();
}

function dashArrayValueLikePdfminer(value: PdfPrimitive): unknown {
  if (isName(value)) return `/'${value.name}'`;
  if (typeof value === "string") return `/b'${value}'`;
  if (Array.isArray(value)) return value.map(dashArrayValueLikePdfminer);
  return value;
}

export function collectGraphicsHints(operations: PdfOperation[], colorSpaces: Record<string, string> = {}): GraphicsHints {
  const colorOps: ColorOp[] = [];
  const lineWidths: number[] = [];
  const dashOps: Array<[unknown[], number]> = [];
  const textMatrices: Matrix[] = [];
  const textMoves: Point[] = [];
  const leadingTextMoves: Point[] = [];
  const transforms: Matrix[] = [];
  const pathOps: number[][] = [];
  const drawnImages: string[] = [];
  let fillColorSpace = "DeviceGray";
  let strokeColorSpace = "DeviceGray";
  let currentPath: number[] = [];
  let point: Point = [0, 0];

  const flushPath = (close = false, includeEmpty = false): void => {
    if (close) currentPath.push(4);
    if (currentPath.length || includeEmpty) pathOps.push(currentPath);
    currentPath = [];
  };

  for (const operation of operations) {
    const nums = numericArgs(operation.args);
    const op = operation.operator;

    if (op === "w" && nums.length >= 1) lineWidths.push(nums.at(-1)!);
    if (op === "d" && operation.args.length >= 2) {
      const rawDashArray = operation.args.at(-2);
      const rawPhase = operation.args.at(-1);
      dashOps.push([Array.isArray(rawDashArray) ? rawDashArray.map(dashArrayValueLikePdfminer) : [], typeof rawPhase === "number" ? rawPhase : 0]);
    }
    if (op === "cm" && nums.length >= 6) transforms.push(nums.slice(-6) as Matrix);
    else if (op === "Tm" && nums.length >= 6) textMatrices.push(nums.slice(-6) as Matrix);
    else if ((op === "Td" || op === "TD") && nums.length >= 2) {
      const point = nums.slice(-2) as Point;
      if (op === "TD") leadingTextMoves.push(point);
      else textMoves.push(point);
    } else if (op === "Do") {
      const name = lastName(operation.args);
      if (name) drawnImages.push(name);
    }

    if (op === "cs" || op === "CS") {
      const name = lastName(operation.args) ?? "";
      const resolved = colorSpaceName(colorSpaces[name] ?? name);
      if (op === "cs") fillColorSpace = resolved;
      else strokeColorSpace = resolved;
      continue;
    }

    const isStroke = isUpperOperator(op);
    let colorSpace = isStroke ? strokeColorSpace : fillColorSpace;
    if (op === "g" || op === "G") colorSpace = "DeviceGray";
    else if (op === "rg" || op === "RG") colorSpace = "DeviceRGB";
    else if (op === "k" || op === "K") colorSpace = "DeviceCMYK";
    if (["g", "G", "rg", "RG", "k", "K", "sc", "SC", "scn", "SCN"].includes(op)) {
      const pattern = op === "scn" || op === "SCN" ? lastName(operation.args) : undefined;
      colorOps.push({ target: isStroke ? "stroke" : "fill", components: nums, colorSpace, pattern });
      if (isStroke) strokeColorSpace = colorSpace;
      else fillColorSpace = colorSpace;
      continue;
    }

    if (op === "m" && nums.length >= 2) {
      const [x, y] = nums.slice(-2);
      currentPath.push(0, x, y);
      point = [x, y];
    } else if (op === "l" && nums.length >= 2) {
      const [x, y] = nums.slice(-2);
      currentPath.push(1, x, y);
      point = [x, y];
    } else if (op === "c" && nums.length >= 6) {
      const values = nums.slice(-6);
      currentPath.push(2, ...values);
      point = [values[4], values[5]];
    } else if (op === "v" && nums.length >= 4) {
      const values = nums.slice(-4);
      currentPath.push(2, point[0], point[1], ...values);
      point = [values[2], values[3]];
    } else if (op === "y" && nums.length >= 4) {
      const values = nums.slice(-4);
      currentPath.push(3, ...values);
      point = [values[2], values[3]];
    } else if (op === "h") {
      currentPath.push(4);
    } else if (op === "re" && nums.length >= 4) {
      const [x, y, width, height] = nums.slice(-4);
      currentPath.push(5, x, y, width, height);
      point = [x, y];
    } else if (["S", "f", "F", "f*", "B", "B*", "n"].includes(op)) {
      flushPath(false, op === "n");
    } else if (["s", "b", "b*"].includes(op)) {
      flushPath(true);
    }
  }

  return { colorOps, lineWidths, dashOps, textMatrices, textMoves, leadingTextMoves, transforms, pathOps, drawnImages };
}

export function collectGraphicsHintsFromContent(content: string, colorSpaces: Record<string, string> = {}): GraphicsHints {
  return collectGraphicsHints(extractContentOps(content), colorSpaces);
}
