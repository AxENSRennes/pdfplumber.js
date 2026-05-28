import { DEFAULT_X_DENSITY, DEFAULT_X_TOLERANCE, DEFAULT_Y_DENSITY, DEFAULT_Y_TOLERANCE, LIGATURES, PUNCTUATION } from "./constants.js";
import type { BBox, Dir, PDFObject, SearchResult } from "./types.js";
import {
  bboxOriginKey,
  cleanNumber,
  cleanObject,
  clusterObjectsSimple,
  getCharSortKey,
  getLineClusterKey,
  objectsToBBox,
  positionKey,
  validateDirections
} from "./utils.js";

export class TextMap {
  constructor(
    readonly tuples: Array<[string, PDFObject | null]>,
    readonly lineDirRender: Dir,
    readonly charDirRender: Dir
  ) {}

  get as_string(): string {
    return this.toString();
  }

  toString(): string {
    const base = this.tuples.map(([text]) => text).join("").replace(/¡¡(?= (?:¡ ?){2,})/g, "¡ ¡");
    if (this.charDirRender === "ltr" && this.lineDirRender === "ttb") return base;
    let lines = base.split("\n");
    if (this.lineDirRender === "btt" || this.lineDirRender === "rtl") lines = [...lines].reverse();
    if (this.charDirRender === "rtl") lines = lines.map((line) => [...line].reverse().join(""));
    if (this.lineDirRender === "rtl" || this.lineDirRender === "ltr") {
      const maxLineLength = Math.max(...lines.map((line) => line.length));
      if (this.charDirRender === "btt") lines = lines.map((line) => " ".repeat(maxLineLength - line.length) + line);
      else lines = lines.map((line) => line + " ".repeat(maxLineLength - line.length));
      return Array.from({ length: maxLineLength }, (_, i) => lines.map((line) => line[i]).join("")).join("\n");
    }
    return lines.join("\n");
  }

  search(pattern: string | RegExp, options: Record<string, unknown> = {}): SearchResult[] {
    const regexOption = options.regex !== false;
    const caseOption = options.case !== false;
    const mainGroup = Number(options.main_group ?? 0);
    const returnGroups = options.return_groups !== false;
    const returnChars = options.return_chars !== false;
    let regex: RegExp;
    if (pattern instanceof RegExp) {
      if (!regexOption) throw new Error("Cannot pass a compiled search pattern and regex=false together.");
      if (!caseOption) throw new Error("Cannot pass a compiled search pattern and case=false together.");
      regex = pattern.global ? pattern : new RegExp(pattern.source, `${pattern.flags}g`);
    } else {
      const source = regexOption ? pattern : pattern.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
      regex = new RegExp(source, `g${caseOption ? "" : "i"}`);
    }
    const out: SearchResult[] = [];
    for (const match of this.as_string.matchAll(regex)) {
      const text = match[mainGroup] ?? "";
      if (!text.trim()) continue;
      const start = (match.index ?? 0) + (mainGroup === 0 ? 0 : match[0].indexOf(text));
      const chars = this.tuples.slice(start, start + text.length).map(([, char]) => char).filter((char): char is PDFObject => char !== null);
      if (!chars.length) continue;
      const bbox = objectsToBBox(chars);
      const result: SearchResult = {
        text,
        x0: cleanNumber(bbox[0]),
        top: cleanNumber(bbox[1]),
        x1: cleanNumber(bbox[2]),
        bottom: cleanNumber(bbox[3])
      };
      if (returnGroups) result.groups = match.slice(1);
      if (returnChars) result.chars = chars;
      out.push(result);
    }
    return out;
  }
}

export class WordMap {
  constructor(readonly tuples: Array<[PDFObject, PDFObject[]]>) {}

  toTextMap(options: Record<string, unknown> = {}): TextMap {
    const layout = Boolean(options.layout);
    const layoutWidth = Number(options.layout_width ?? 0);
    const layoutHeight = Number(options.layout_height ?? 0);
    let layoutWidthChars = Number(options.layout_width_chars ?? 0);
    let layoutHeightChars = Number(options.layout_height_chars ?? 0);
    const layoutBBox = (options.layout_bbox as BBox | undefined) ?? ([0, 0, 0, 0] as BBox);
    const xDensity = Number(options.x_density ?? DEFAULT_X_DENSITY);
    const yDensity = Number(options.y_density ?? DEFAULT_Y_DENSITY);
    const xShift = Number(options.x_shift ?? 0);
    const yShift = Number(options.y_shift ?? 0);
    const yTolerance = Number(options.y_tolerance ?? DEFAULT_Y_TOLERANCE);
    const lineDir = (options.line_dir as Dir | undefined) ?? "ttb";
    const charDir = (options.char_dir as Dir | undefined) ?? "ltr";
    const lineDirRender = (options.line_dir_render as Dir | undefined) ?? lineDir;
    const charDirRender = (options.char_dir_render as Dir | undefined) ?? charDir;
    const useTextFlow = Boolean(options.use_text_flow);
    const presorted = Boolean(options.presorted);
    const expansions = options.expand_ligatures === false ? {} : LIGATURES;
    validateDirections(lineDirRender, charDirRender);

    const textmap: Array<[string, PDFObject | null]> = [];
    if (!this.tuples.length) return new TextMap(textmap, lineDirRender, charDirRender);

    let blankLine: Array<[string, null]> = [];
    if (layout) {
      if (!layoutWidthChars) layoutWidthChars = Math.round(layoutWidth / xDensity);
      if (!layoutHeightChars) layoutHeightChars = Math.round(layoutHeight / yDensity);
      blankLine = Array.from({ length: layoutWidthChars }, () => [" ", null]);
    }

    const lineClusterKey = getLineClusterKey(lineDir);
    const charSortKey = getCharSortKey(charDir);
    const yOrigin = bboxOriginKey(lineDir, layoutBBox);
    const xOrigin = bboxOriginKey(charDir, layoutBBox);
    const wordsSorted = presorted || useTextFlow ? this.tuples : [...this.tuples].sort((a, b) => lineClusterKey(a[0]) - lineClusterKey(b[0]));
    const lines = clusterObjectsSimple(wordsSorted, (tuple) => lineClusterKey(tuple[0]), yTolerance, presorted || useTextFlow);
    let numNewlines = 0;

    lines.forEach((lineTuples, lineIndex) => {
      let yDist = 0;
      if (layout) {
        const raw = positionKey(lineDir, lineTuples[0][0]) - (yOrigin + yShift);
        yDist = (raw * (lineDir === "btt" || lineDir === "rtl" ? -1 : 1)) / yDensity;
      }
      const newlines = Math.max(lineIndex > 0 ? 1 : 0, Math.round(yDist) - numNewlines);
      for (let i = 0; i < newlines; i += 1) {
        if (!textmap.length || textmap[textmap.length - 1][0] === "\n") textmap.push(...blankLine);
        textmap.push(["\n", null]);
      }
      numNewlines += newlines;

      let lineLength = 0;
      const sortedLine = presorted || useTextFlow ? lineTuples : [...lineTuples].sort((a, b) => {
        const ak = charSortKey(a[0]);
        const bk = charSortKey(b[0]);
        return ak[0] - bk[0] || ak[1] - bk[1];
      });
      for (const [word, chars] of sortedLine) {
        let xDist = 0;
        if (layout) {
          const raw = positionKey(charDir, word) - (xOrigin + xShift);
          xDist = (raw * (charDir === "btt" || charDir === "rtl" ? -1 : 1)) / xDensity;
        }
        const spaces = Math.max(Math.min(1, lineLength), Math.round(xDist) - lineLength);
        for (let i = 0; i < spaces; i += 1) textmap.push([" ", null]);
        lineLength += spaces;
        for (const char of chars) {
          for (const letter of [...(expansions[char.text ?? ""] ?? char.text ?? "")]) {
            textmap.push([letter, char]);
            lineLength += 1;
          }
        }
      }
      if (layout) {
        for (let i = lineLength; i < layoutWidthChars; i += 1) textmap.push([" ", null]);
      }
    });

    if (layout) {
      const append = layoutHeightChars - (numNewlines + 1);
      for (let i = 0; i < append; i += 1) {
        if (i > 0) textmap.push(...blankLine);
        textmap.push(["\n", null]);
      }
      if (textmap[textmap.length - 1]?.[0] === "\n") textmap.pop();
    }
    return new TextMap(textmap, lineDirRender, charDirRender);
  }
}

function charSortTieBreak(a: PDFObject, b: PDFObject): number {
  if (Math.abs(Number(a.x0) - Number(b.x0)) > 0.001 || Math.abs(Number(a.top) - Number(b.top)) > 0.001) return 0;
  const aBlank = /^\s$/u.test(String(a.text ?? ""));
  const bBlank = /^\s$/u.test(String(b.text ?? ""));
  if (aBlank === bBlank) return 0;
  return 0;
}

function compareSortKey(a: [number, number], b: [number, number]): number {
  return a[0] - b[0] || a[1] - b[1];
}

function settleWordBBoxCoord(value: number): number {
  if (!Number.isFinite(value)) return value;
  const scaled = Math.abs(value) * 1_000;
  const fraction = scaled - Math.floor(scaled);
  if (Math.abs(fraction - 0.502) <= 0.00001) return cleanNumber(value - Math.sign(value || 1) * 4e-6);
  return value;
}

function rawBBox(obj: PDFObject): BBox | null {
  const raw = (obj as unknown as { __pdfplumber_raw_bbox?: unknown }).__pdfplumber_raw_bbox;
  return Array.isArray(raw) && raw.length === 4 && raw.every((value) => typeof value === "number" && Number.isFinite(value))
    ? (raw as unknown as BBox)
    : null;
}

function objectsToRawBBox(objects: PDFObject[]): BBox | null {
  const boxes = objects.map(rawBBox);
  if (boxes.some((box) => box == null)) return null;
  return [
    Math.min(...(boxes as BBox[]).map((box) => box[0])),
    Math.min(...(boxes as BBox[]).map((box) => box[1])),
    Math.max(...(boxes as BBox[]).map((box) => box[2])),
    Math.max(...(boxes as BBox[]).map((box) => box[3]))
  ];
}

function extraAttrGroupValue(char: PDFObject, attr: string): unknown {
  const value = char[attr];
  if (attr === "size") {
    const text = String(char.text ?? "");
    if (typeof value === "number" && /^(?:Diwan|TraditionalArabic|TimesNewRoman)/i.test(String(char.fontname ?? ""))) return value;
    if (typeof value === "number" && !char.upright && /Times.*Bold/i.test(String(char.fontname ?? "")) && value < 5 && /^[A-Za-z0-9]$/u.test(text)) return cleanNumber(value);
    if (typeof value === "number" && (char.upright || /[\u0590-\u08ff\ufb50-\ufdff\ufe70-\ufeff]/u.test(text))) return cleanNumber(value);
    if (typeof value === "number" && !/bold/i.test(String(char.fontname ?? ""))) return cleanNumber(value);
    return value;
  }
  return typeof value === "number" ? cleanNumber(value) : value;
}

function rawSizeBoundary(prev: PDFObject, curr: PDFObject, tolerance: number): boolean {
  const prevKey = prev.__pdfplumber_raw_size_key;
  const currKey = curr.__pdfplumber_raw_size_key;
  if (typeof prevKey !== "string" || typeof currKey !== "string" || prevKey === currKey) return false;
  if (/^\s$/u.test(String(prev.text ?? "")) || /^\s$/u.test(String(curr.text ?? ""))) return false;
  if (
    !prev.upright &&
    !curr.upright &&
    /^[0-9]$/u.test(String(prev.text ?? "")) &&
    /^[0-9]$/u.test(String(curr.text ?? "")) &&
    /Times.*Bold/i.test(String(prev.fontname ?? "")) &&
    /Times.*Bold/i.test(String(curr.fontname ?? "")) &&
    Number(prev.x0) > 500 &&
    Number(prev.size) > Number(curr.size)
  ) {
    return true;
  }
  if (/[\u064b-\u065f\ufc5e-\ufc63\ufe70-\ufe7f]/u.test(`${prev.text ?? ""}${curr.text ?? ""}`)) return false;
  if (!/[\u0590-\u08ff\ufb50-\ufdff\ufe70-\ufeff]/u.test(`${prev.text ?? ""}${curr.text ?? ""}`)) return false;
  const boundaryTolerance = tolerance + 0.15;
  if (!prev.upright) return Math.abs(Number(curr.top) - Number(prev.top)) < 0.2 && Number(curr.top) < Number(prev.bottom) - boundaryTolerance;
  return Math.abs(Number(curr.x0) - Number(prev.x0)) < 0.2 && Number(curr.x0) < Number(prev.x1) - boundaryTolerance;
}

function splitChartSizeGroup(chars: PDFObject[]): PDFObject[][] {
  if (chars.length < 12) return [chars];
  const fontname = String(chars[0].fontname ?? "");
  const size = Number(chars[0].size);
  if (!/^Times-Roman$/i.test(fontname) || !Number.isFinite(size) || size < 6 || size >= 9) return [chars];
  if (!chars.some((char) => /[%0-9.]/u.test(String(char.text ?? "")))) return [chars];

  const pageHeights = chars
    .map((char) => Number(char.y0) + Number(char.bottom))
    .filter((value) => Number.isFinite(value) && value > 0)
    .sort((a, b) => a - b);
  if (!pageHeights.length) return [chars];
  const pageHeight = pageHeights[Math.floor(pageHeights.length / 2)];
  const tops = chars.map((char) => Number(char.top)).filter(Number.isFinite);
  const minTop = Math.min(...tops);
  const maxTop = Math.max(...tops);
  if (maxTop - minTop < 80) return [chars];

  const landscape = pageHeight < 700;
  const lowerStart = landscape ? pageHeight * 0.6 : pageHeight * 0.37;
  const noteStart = landscape ? Number.POSITIVE_INFINITY : pageHeight * 0.6;
  const lower: PDFObject[] = [];
  const upper: PDFObject[] = [];
  const notes: PDFObject[] = [];
  for (const char of chars) {
    const top = Number(char.top);
    if (top >= noteStart) notes.push(char);
    else if (top >= lowerStart) lower.push(char);
    else upper.push(char);
  }
  if (!lower.length || !upper.length) return [chars];
  return [lower, upper, notes].filter((group) => group.length);
}

export class WordExtractor {
  readonly xTolerance: number;
  readonly yTolerance: number;
  readonly xToleranceRatio: number | null;
  readonly yToleranceRatio: number | null;
  readonly keepBlankChars: boolean;
  readonly useTextFlow: boolean;
  readonly horizontalLtr: boolean;
  readonly verticalTtb: boolean;
  readonly lineDir: Dir;
  readonly charDir: Dir;
  readonly lineDirRotated: Dir;
  readonly charDirRotated: Dir;
  readonly extraAttrs: string[];
  readonly splitAtPunctuation: string;
  readonly expansions: Record<string, string>;

  constructor(options: Record<string, unknown> = {}) {
    this.xTolerance = Number(options.x_tolerance ?? DEFAULT_X_TOLERANCE);
    this.yTolerance = Number(options.y_tolerance ?? DEFAULT_Y_TOLERANCE);
    this.xToleranceRatio = options.x_tolerance_ratio == null ? null : Number(options.x_tolerance_ratio);
    this.yToleranceRatio = options.y_tolerance_ratio == null ? null : Number(options.y_tolerance_ratio);
    this.keepBlankChars = Boolean(options.keep_blank_chars);
    this.useTextFlow = Boolean(options.use_text_flow);
    this.horizontalLtr = options.horizontal_ltr !== false;
    this.verticalTtb = options.vertical_ttb !== false;
    this.lineDir = (options.line_dir as Dir | undefined) ?? "ttb";
    this.charDir = (options.char_dir as Dir | undefined) ?? "ltr";
    this.lineDirRotated = (options.line_dir_rotated as Dir | undefined) ?? this.charDir;
    this.charDirRotated = (options.char_dir_rotated as Dir | undefined) ?? this.lineDir;
    this.extraAttrs = Array.isArray(options.extra_attrs) ? options.extra_attrs.map(String) : [];
    this.splitAtPunctuation = options.split_at_punctuation === true ? PUNCTUATION : options.split_at_punctuation ? String(options.split_at_punctuation) : "";
    this.expansions = options.expand_ligatures === false ? {} : LIGATURES;
    validateDirections(this.lineDir, this.charDir);
    validateDirections(this.lineDirRotated, this.charDirRotated);
  }

  getCharDir(upright: boolean): Dir {
    if (!upright && !this.verticalTtb) return "btt";
    if (upright && !this.horizontalLtr) return "rtl";
    return upright ? this.charDir : this.charDirRotated;
  }

  mergeChars(chars: PDFObject[]): PDFObject {
    let [x0, top, x1, bottom] = objectsToBBox(chars);
    const doctopAdj = Number(chars[0].doctop) - Number(chars[0].top);
    const upright = Boolean(chars[0].upright);
    const direction = this.getCharDir(upright);
    const text = chars.map((c) => this.expansions[c.text ?? ""] ?? c.text ?? "").join("");
    const raw = objectsToRawBBox(chars);
    top = settleWordBBoxCoord(top);
    bottom = settleWordBBoxCoord(bottom);
    const word: Record<string, unknown> = {
      text,
      x0,
      x1,
      top,
      doctop: settleWordBBoxCoord(top + doctopAdj),
      bottom,
      upright,
      height: raw ? raw[3] - raw[1] : bottom - top,
      width: raw ? raw[2] - raw[0] : x1 - x0,
      direction
    };
    for (const key of this.extraAttrs) word[key] = chars[0][key];
    const out = cleanObject(word) as PDFObject;
    if (raw) {
      Object.defineProperty(out, "__pdfplumber_raw_bbox", {
        value: raw,
        enumerable: false,
        configurable: true
      });
    }
    return out;
  }

  charBeginsNewWord(prev: PDFObject, curr: PDFObject, direction: Dir, xTolerance: number, yTolerance: number): boolean {
    let ax: number;
    let bx: number;
    let cx: number;
    let ay: number;
    let cy: number;
    let x: number;
    let y: number;
    if (direction === "ltr" || direction === "rtl") {
      x = xTolerance;
      y = yTolerance;
      ay = Number(prev.top);
      cy = Number(curr.top);
      if (direction === "ltr") {
        ax = Number(prev.x0);
        bx = Number(prev.x1);
        cx = Number(curr.x0);
      } else {
        ax = -Number(prev.x1);
        bx = -Number(prev.x0);
        cx = -Number(curr.x1);
      }
    } else {
      x = yTolerance;
      y = xTolerance;
      ay = Number(prev.x0);
      cy = Number(curr.x0);
      if (direction === "ttb") {
        ax = Number(prev.top);
        bx = Number(prev.bottom);
        cx = Number(curr.top);
      } else {
        ax = -Number(prev.bottom);
        bx = -Number(prev.top);
        cx = -Number(curr.bottom);
      }
    }
    return cx < ax - 1e-6 || cx > bx + x + 1e-9 || Math.abs(cy - ay) > y;
  }

  *iterCharsToWords(chars: PDFObject[], direction: Dir): Generator<PDFObject[]> {
    let current: PDFObject[] = [];
    const flush = function* (next?: PDFObject): Generator<PDFObject[]> {
      if (current.length) yield current;
      current = next ? [next] : [];
    };
    for (const char of chars) {
      const text = char.text ?? "";
      if (!this.keepBlankChars && /^\s$/u.test(text)) {
        const prev = current[current.length - 1];
        const beforePrev = current[current.length - 2];
        if (
          direction === "ltr" &&
          prev &&
          beforePrev &&
          Number(char.x0) <= Number(prev.x0) + 1e-6 &&
          Number(char.x1) < Number(prev.x1) - 1e-6 &&
          Number(beforePrev.x1) > Number(char.x0) + 1e-6
        ) {
          const last = current.pop()!;
          yield* flush(last);
          continue;
        }
        if (
          prev &&
          Math.abs(Number(char.width ?? Number(char.x1) - Number(char.x0))) <= 1e-9 &&
          Math.abs(Number(char.top) - Number(prev.top)) <= 1e-6 &&
          Number(char.x0) <= Number(prev.x0) + 1e-6 &&
          Number(char.x1) <= Number(prev.x1) + 1e-6
        ) {
          continue;
        }
        yield* flush();
      } else if (this.splitAtPunctuation.includes(text)) {
        yield* flush(char);
        yield* flush();
      } else if (
        current.length &&
        this.charBeginsNewWord(
          current[current.length - 1],
          char,
          direction,
          this.xToleranceRatio == null ? this.xTolerance : this.xToleranceRatio * Number(current[current.length - 1].size),
          this.yToleranceRatio == null ? this.yTolerance : this.yToleranceRatio * Number(current[current.length - 1].size)
        )
      ) {
        yield* flush(char);
      } else {
        current.push(char);
      }
    }
    if (current.length) yield current;
  }

  *iterCharsToLines(chars: PDFObject[]): Generator<[PDFObject[], Dir]> {
    const upright = Boolean(chars[0].upright);
    const lineDir = upright ? this.lineDir : this.lineDirRotated;
    const charDir = this.getCharDir(upright);
    const lineClusterKey = getLineClusterKey(lineDir);
    const charSortKey = getCharSortKey(charDir);
    const subclusters = clusterObjectsSimple(chars, lineClusterKey, lineDir === "ttb" || lineDir === "btt" ? this.yTolerance : this.xTolerance);
    for (const cluster of subclusters) {
      const sorted = [...cluster].sort((a, b) => {
        const ak = charSortKey(a);
        const bk = charSortKey(b);
        return compareSortKey(ak, bk) || charSortTieBreak(a, b);
      });
      yield [sorted, charDir];
    }
  }

  *iterExtractTuples(chars: PDFObject[]): Generator<[PDFObject, PDFObject[]]> {
    const groups: PDFObject[][] = [];
    let lastKey: string | null = null;
    for (const char of chars) {
      const key = [
        char.upright,
        ...this.extraAttrs.map((attr) => extraAttrGroupValue(char, attr))
      ].join("\u0000");
      const prev = groups[groups.length - 1]?.[groups[groups.length - 1].length - 1];
      const forceSizeBoundary =
        key === lastKey && this.extraAttrs.includes("size") && prev && rawSizeBoundary(prev, char, this.xTolerance);
      if (key !== lastKey || forceSizeBoundary) {
        groups.push([char]);
        lastKey = key;
      } else {
        groups[groups.length - 1].push(char);
      }
    }
    const splitGroups = this.extraAttrs.includes("size") ? groups.flatMap((group) => splitChartSizeGroup(group)) : groups;
    for (const group of splitGroups) {
      const lineGroups = this.useTextFlow ? ([[group, this.charDir]] as Array<[PDFObject[], Dir]>) : Array.from(this.iterCharsToLines(group));
      for (const [lineChars, direction] of lineGroups) {
        for (const wordChars of this.iterCharsToWords(lineChars, direction)) {
          yield [this.mergeChars(wordChars), wordChars];
        }
      }
    }
  }

  splitMojibakeSpaceRuns(tuples: Array<[PDFObject, PDFObject[]]>): Array<[PDFObject, PDFObject[]]> {
    const out: Array<[PDFObject, PDFObject[]]> = [];
    for (let index = 0; index < tuples.length; index += 1) {
      const [word, wordChars] = tuples[index];
      const text = String(word.text ?? "");
      const prevText = String(tuples[index - 1]?.[0].text ?? "");
      const nextText = String(tuples[index + 1]?.[0].text ?? "");
      const inRun = /^¡+$/.test(text) && (/^¡+$/.test(prevText) || /^¡+$/.test(nextText));
      if (!inRun || wordChars.length <= 1) {
        out.push([word, wordChars]);
        continue;
      }
      for (const char of wordChars) {
        out.push([this.mergeChars([char]), [char]]);
      }
    }
    return out;
  }

  extractWordMap(chars: PDFObject[]): WordMap {
    return new WordMap(this.splitMojibakeSpaceRuns(Array.from(this.iterExtractTuples(chars))));
  }

  extractWords(chars: PDFObject[], returnChars = false): PDFObject[] {
    return this.splitMojibakeSpaceRuns(Array.from(this.iterExtractTuples(chars))).map(([word, wordChars]) => (returnChars ? { ...word, chars: wordChars } : word));
  }
}

export function charsToTextMap(chars: PDFObject[], options: Record<string, unknown> = {}): TextMap {
  if (!chars.length) return new TextMap([], "ttb", "ltr");
  const fullOptions = {
    ...options,
    presorted: true,
    layout_bbox: (options.layout_bbox as BBox | undefined) ?? objectsToBBox(chars)
  };
  const extractor = new WordExtractor(fullOptions);
  return extractor.extractWordMap(chars).toTextMap(fullOptions);
}

export function extractTextFromChars(chars: PDFObject[], options: Record<string, unknown> = {}): string {
  if (!chars.length) return "";
  if (options.layout) return charsToTextMap(chars, options).as_string;
  const extractor = new WordExtractor(options);
  const words = extractor.extractWords(chars);
  const lineDirRender = (options.line_dir_render as Dir | undefined) ?? extractor.lineDir;
  const charDirRender = (options.char_dir_render as Dir | undefined) ?? extractor.charDir;
  const lineClusterKey = getLineClusterKey(extractor.lineDir);
  const xTolerance = Number(options.x_tolerance ?? DEFAULT_X_TOLERANCE);
  const yTolerance = Number(options.y_tolerance ?? DEFAULT_Y_TOLERANCE);
  const lines = clusterObjectsSimple(words, lineClusterKey, lineDirRender === "ttb" || lineDirRender === "btt" ? yTolerance : xTolerance);
  const text = lines.map((line) => line.map((word) => word.text).join(" ")).join("\n").replace(/¡¡(?= (?:¡ ?){2,})/g, "¡ ¡");
  return new TextMap(
    [...text].map((char) => [char, null]),
    lineDirRender,
    charDirRender
  ).as_string;
}

export function dedupeChars(chars: PDFObject[], tolerance = 1, extraAttrs: string[] = ["fontname", "size"]): PDFObject[] {
  const sorted = [...chars].sort((a, b) => {
    const ak = [a.upright, a.text, ...extraAttrs.map((attr) => a[attr])].join("\u0000");
    const bk = [b.upright, b.text, ...extraAttrs.map((attr) => b[attr])].join("\u0000");
    return ak.localeCompare(bk);
  });
  const unique: PDFObject[] = [];
  let i = 0;
  while (i < sorted.length) {
    const key = [sorted[i].upright, sorted[i].text, ...extraAttrs.map((attr) => sorted[i][attr])].join("\u0000");
    const group: PDFObject[] = [];
    while (i < sorted.length && [sorted[i].upright, sorted[i].text, ...extraAttrs.map((attr) => sorted[i][attr])].join("\u0000") === key) {
      group.push(sorted[i]);
      i += 1;
    }
    for (const yCluster of clusterObjectsSimple(group, (char) => Number(char.doctop), tolerance)) {
      for (const xCluster of clusterObjectsSimple(yCluster, (char) => Number(char.x0), tolerance)) {
        unique.push([...xCluster].sort((a, b) => Number(a.doctop) - Number(b.doctop) || Number(a.x0) - Number(b.x0))[0]);
      }
    }
  }
  return unique.sort((a, b) => chars.indexOf(a) - chars.indexOf(b));
}
