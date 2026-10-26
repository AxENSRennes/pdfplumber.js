/**
 * The structure tree (pdfplumber/structure.py:151 `PDFStructTree`,
 * pdfplumber/structure.py:102 `PDFStructElement`) through the operations
 * of crates/wasm/src/structs.rs (docs/public-api.md, "Structure Tree").
 *
 * `pdf.structTree(page?)` makes the tree in the core (`struct_tree`): a
 * handle, and the elements as values, from which this module makes the
 * `PDFStructElement` objects; an element is named in the core by its path
 * (the indices from `tree.children` down). Every member is one job of the
 * core: `findAll`/`find` (`struct_find_all`/`struct_find`; a `RegExp` or a
 * function is run here on every element of the tree, in `_find_all`'s
 * depth-first order, and the core takes the truth of each: the mask),
 * `elementBbox` (`struct_element_bbox`), an element's `allMcids`
 * (`struct_all_mcids`), `repr` (`struct_element_repr`) and `toDict`
 * (`struct_element_to_dict`). Iterating over a tree or an element gives
 * its children, as `__iter__` does.
 */

import type { PDFPlumberDocument, PDFPlumberPage } from "../api.js";
import type { Bbox, StructMatcher } from "../types.js";

/** An element as `struct_tree` writes it: the dataclass's fields in order,
 * `children` their values. */
interface ElementValue {
  type: string;
  revision: unknown;
  id: string | null;
  lang: string | null;
  alt_text: string | null;
  actual_text: string | null;
  title: string | null;
  page_number: number | null;
  attributes: Record<string, unknown>;
  mcids: number[];
  children: ElementValue[];
}

/** `struct_tree`'s value. */
export interface TreeValue {
  handle: number;
  children: ElementValue[];
}

type Path = number[];

/** The elements of a tree (or of `children`) in `_find_all`'s order
 * (pdfplumber/structure.py:39: a deque popped from the left, each
 * element's children pushed back in front of it, in order: depth-first,
 * preorder); an element reached twice is listed twice. */
function preorder(children: readonly PDFStructElement[]): PDFStructElement[] {
  const out: PDFStructElement[] = [];
  const stack = [...children].reverse();
  while (stack.length) {
    const el = stack.pop()!;
    out.push(el);
    for (let i = el.children.length - 1; i >= 0; i--) stack.push(el.children[i]);
  }
  return out;
}

/** The arguments of `struct_find_all`/`struct_find` after the path: for
 * a `RegExp` or a function, its truth on every element of the tree (the
 * mask, an argument of its own); any other value as it is (a tag, a `str`;
 * anything else Python calls: `TypeError`, not callable, a plain object
 * such as `{ mask: [...] }` included). */
function matcherArgs(tree: PDFStructTree, matcher: unknown): unknown[] {
  if (matcher instanceof RegExp) {
    // `matcher.match(x.type)`: a match at the start (sticky, from 0)
    const re = new RegExp(matcher.source, matcher.flags.replace(/[gy]/g, "") + "y");
    return [
      null,
      preorder(tree.children).map((el) => {
        re.lastIndex = 0;
        return re.test(el.type);
      }),
    ];
  }
  if (typeof matcher === "function") return [null, preorder(tree.children).map((el) => Boolean(matcher(el)))];
  return [matcher];
}

/** A structure element (`PDFStructElement`): its fields (snake_case, as
 * the dataclass's) and children, `findAll`/`find` in its subtree,
 * `allMcids()` and `repr()`. */
export class PDFStructElement {
  readonly type: string;
  readonly revision: unknown;
  readonly id: string | null;
  readonly lang: string | null;
  readonly alt_text: string | null;
  readonly actual_text: string | null;
  readonly title: string | null;
  readonly page_number: number | null;
  readonly attributes: Record<string, unknown>;
  readonly mcids: number[];
  readonly children: PDFStructElement[] = [];

  /** @internal */
  constructor(
    value: ElementValue,
    /** The tree it was read from. */
    readonly tree: PDFStructTree,
    /** @internal Its path in the tree (indices from `tree.children`). */
    readonly path: Path,
  ) {
    this.type = value.type;
    this.revision = value.revision;
    this.id = value.id;
    this.lang = value.lang;
    this.alt_text = value.alt_text;
    this.actual_text = value.actual_text;
    this.title = value.title;
    this.page_number = value.page_number;
    this.attributes = value.attributes;
    this.mcids = value.mcids;
  }

  get altText(): string | null {
    return this.alt_text;
  }
  get actualText(): string | null {
    return this.actual_text;
  }
  get pageNumber(): number | null {
    return this.page_number;
  }

  /** `el.find_all(matcher)`: the matching elements of its subtree,
   * depth-first. */
  findAll(matcher: StructMatcher<PDFStructElement>): Promise<PDFStructElement[]> {
    return this.tree.find_(this.path, matcher, true) as Promise<PDFStructElement[]>;
  }
  find_all(matcher: StructMatcher<PDFStructElement>): Promise<PDFStructElement[]> {
    return this.findAll(matcher);
  }

  /** `el.find(matcher)`: the first matching element of its subtree. */
  find(matcher: StructMatcher<PDFStructElement>): Promise<PDFStructElement | null> {
    return this.tree.find_(this.path, matcher, false) as Promise<PDFStructElement | null>;
  }

  /** `list(el.all_mcids())`: `[page_number, mcid]` of its marked content
   * and its descendants', depth-first. */
  allMcids(): Promise<[number | null, number][]> {
    return this.tree.run_("struct_all_mcids", [this.path]) as Promise<[number | null, number][]>;
  }
  all_mcids(): Promise<[number | null, number][]> {
    return this.allMcids();
  }

  /** `repr(el)` (the dataclass's repr). */
  repr(): Promise<string> {
    return this.tree.run_("struct_element_repr", [this.path]) as Promise<string>;
  }

  /** `el.to_dict()`: the compacted dict of the element and its subtree
   * (`dataclasses.asdict`, then every `None`, `[]` and `{}` value removed),
   * as `structure_tree` gives each element. */
  toDict(): Promise<Record<string, unknown>> {
    return this.tree.run_("struct_element_to_dict", [this.path]) as Promise<Record<string, unknown>>;
  }
  to_dict(): Promise<Record<string, unknown>> {
    return this.toDict();
  }

  /** `iter(el)`: its children (`PDFStructElement.__iter__`). */
  [Symbol.iterator](): Iterator<PDFStructElement> {
    return this.children[Symbol.iterator]();
  }
}

/** `PDFStructTree(pdf, page)`: the structure tree of a document, or the
 * part of it on one page. */
export class PDFStructTree {
  readonly children: PDFStructElement[];

  /** @internal */
  constructor(
    /** The document. */
    readonly pdf: PDFPlumberDocument,
    /** The page (`PDFStructTree.page`), or `null` for the document's tree. */
    readonly page: PDFPlumberPage | null,
    /** @internal */ readonly handle: number,
    children: ElementValue[],
  ) {
    // without recursion: a tree of any depth
    this.children = [];
    const stack: [ElementValue, Path, PDFStructElement[]][] = children.map((v, i) => [v, [i], this.children]);
    stack.reverse();
    while (stack.length) {
      const [value, path, into] = stack.pop()!;
      const el = new PDFStructElement(value, this, path);
      into.push(el);
      for (let i = value.children.length - 1; i >= 0; i--) stack.push([value.children[i], [...path, i], el.children]);
    }
    pdf.track(this, handle);
  }

  /** @internal One operation on the tree, in a job of its document. */
  run_(op: string, args: unknown): Promise<unknown> {
    const pdf = this.pdf;
    return pdf.job((call) => call(op, this.handle, args, pdf.callOptions(this.page?.page_number)));
  }

  /** The element of a path. */
  #at(path: Path): PDFStructElement {
    let children = this.children;
    let el: PDFStructElement | undefined;
    for (const i of path) {
      el = children[i];
      children = el.children;
    }
    return el!;
  }

  /** @internal `find_all`/`find` on the tree (`path` null) or on an
   * element: the matcher (its mask computed here, in the job), then the
   * paths of the elements found. */
  find_(path: Path | null, matcher: unknown, all: boolean): Promise<PDFStructElement[] | PDFStructElement | null> {
    const pdf = this.pdf;
    return pdf.job(async (call) => {
      const args = [path, ...matcherArgs(this, matcher)];
      const found = await call(all ? "struct_find_all" : "struct_find", this.handle, args, pdf.callOptions(this.page?.page_number));
      if (all) return (found as Path[]).map((p) => this.#at(p));
      return found === null ? null : this.#at(found as Path);
    });
  }

  /** `tree.find_all(matcher)`: the matching elements, depth-first. */
  findAll(matcher: StructMatcher<PDFStructElement>): Promise<PDFStructElement[]> {
    return this.find_(null, matcher, true) as Promise<PDFStructElement[]>;
  }
  find_all(matcher: StructMatcher<PDFStructElement>): Promise<PDFStructElement[]> {
    return this.findAll(matcher);
  }

  /** `iter(tree)`: its children (`PDFStructTree.__iter__`). */
  [Symbol.iterator](): Iterator<PDFStructElement> {
    return this.children[Symbol.iterator]();
  }

  /** `tree.find(matcher)`: the first matching element, or `null`. */
  find(matcher: StructMatcher<PDFStructElement>): Promise<PDFStructElement | null> {
    return this.find_(null, matcher, false) as Promise<PDFStructElement | null>;
  }

  /** `tree.element_bbox(el)`: the bounding box of an element of this tree
   * (its `BBox` attribute, else its marked content's objects). */
  elementBbox(el: PDFStructElement): Promise<Bbox> {
    if (!(el instanceof PDFStructElement) || el.tree !== this)
      return Promise.reject(new TypeError("elementBbox() takes an element of this tree"));
    return this.run_("struct_element_bbox", [el.path]) as Promise<Bbox>;
  }
  element_bbox(el: PDFStructElement): Promise<Bbox> {
    return this.elementBbox(el);
  }
}
