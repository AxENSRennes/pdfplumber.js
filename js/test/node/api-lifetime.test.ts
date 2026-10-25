// The identities and lifetimes of the public API (docs/public-api.md,
// "Values" and "Lifetime") that a session of oracle/unit/js_api.py cannot
// see (it compares values): pdf.page(ref) is the page pdf.pages gives
// (as annot["data"]["P"] is pdf.pages[i] in Python), a stream is one
// object while it lives, an element found is an element of the tree, and
// unreachable derived pages, tables and trees free their handles in the
// core, never a handle of another document.
import { join } from "node:path";
import v8 from "node:v8";
import vm from "node:vm";
import { describe, expect, it, vi } from "vitest";

import * as api from "../../src/index.js";
import { getCore, internals } from "../../src/internal.js";

const lib = { getCore };
import { PDFPLUMBER_ROOT } from "../oracle-data.mjs";

const pdfs = join(PDFPLUMBER_ROOT, "upstream/pdfplumber-python/tests/pdfs");
// annotations whose /P names their page, on its first two pages
const ANNOTATED = join(pdfs, "issue-463-example.pdf");

v8.setFlagsFromString("--expose-gc");
const gc = vm.runInNewContext("gc") as () => void;

/** Collects garbage until the finalizers have run (they run after a
 * collection, in later tasks), then lets the frees they queued run. */
async function collect(): Promise<void> {
  for (let i = 0; i < 6; i++) {
    gc();
    await new Promise((r) => setTimeout(r, 20));
  }
}

async function handles(): Promise<number> {
  const { info } = await internals();
  return (await info()).handles as number;
}

type Annot = { data: { P: api.PageRef } };

describe("public API identities and lifetimes", () => {
  it("page(ref) of a reference read before pdf.pages is pdf.pages' page", async () => {
    api.configure({});
    const pdf = await api.open(ANNOTATED);
    const annots = (await pdf.annots) as unknown as Annot[];
    const first = await pdf.page(annots[0].data.P);
    const pages = await pdf.pages;
    expect(pages[0]).toBe(first);
    // read again from the page: the same object
    const again = (await pages[0].annots) as unknown as Annot[];
    expect(await pdf.page(again[0].data.P)).toBe(pages[0]);
    await pdf.release();
  });

  it("page(ref) of a reference read after pdf.pages is pdf.pages' page, until close()", async () => {
    api.configure({});
    const pdf = await api.open(ANNOTATED);
    const pages = await pdf.pages;
    const annots = (await pages[1].annots) as unknown as Annot[];
    expect(await pdf.page(annots[0].data.P)).toBe(pages[1]);
    await pdf.close();
    // a reference of before close() names a page of the flushed list: the
    // page object of before, while it lives
    const old = await pdf.page(annots[0].data.P);
    expect(old).toBe(pages[1]);
    const fresh = await pdf.pages;
    expect(fresh[1]).not.toBe(pages[1]);
    const now = (await fresh[1].annots) as unknown as Annot[];
    expect(await pdf.page(now[0].data.P)).toBe(fresh[1]);
    expect(await pdf.page(annots[0].data.P)).toBe(pages[1]);
    await pdf.release();
    await expect(pdf.page(now[0].data.P)).rejects.toThrow(api.DocumentReleased);
  });

  it("pdf.pages is the same array until close(); a reference read before close() never names a page of the new list", async () => {
    api.configure({});
    const pdf = await api.open(ANNOTATED);
    // the document's annotations, no page object made yet
    const annots = (await pdf.annots) as unknown as Annot[];
    const first = await pdf.pages;
    expect(await pdf.pages).toBe(first);
    await pdf.close();
    const fresh = await pdf.pages;
    expect(fresh).not.toBe(first);
    expect(await pdf.pages).toBe(fresh);
    // Python: annot["data"]["P"] is a page of the list close() flushed,
    // not pdf.pages[0]
    const old = await pdf.page(annots[0].data.P);
    expect(old.page_number).toBe(annots[0].data.P.page_number);
    expect(fresh).not.toContain(old);
    expect(await pdf.page(annots[0].data.P)).toBe(old);
    await pdf.release();
  });

  it("a reference whose handle the API does not hold is the current page of its number", async () => {
    api.configure({});
    for (const read of ["annots", "toDict", "docToDict", "hyperlinks"] as const) {
      const pdf = await api.open(ANNOTATED);
      // the document's annotations first: their pages get handles of
      // their own, which later values name (the first live handle)
      await pdf.annots;
      const pages = await pdf.pages;
      let annots: Annot[];
      if (read === "annots") annots = (await pages[0].annots) as unknown as Annot[];
      else if (read === "hyperlinks") annots = (await pdf.hyperlinks) as unknown as Annot[];
      else if (read === "toDict") annots = (await pages[0].toDict()).annots as Annot[];
      else annots = ((await pdf.toDict()).pages as { annots: Annot[] }[])[0].annots;
      if (annots.length) expect(await pdf.page(annots[0].data.P)).toBe(pages[annots[0].data.P.page_number as number - 1]);
      await pdf.release();
    }
  });

  it("release() frees the document's handles", async () => {
    api.configure({});
    await collect();
    const before = await handles();
    const pdf = await api.open(ANNOTATED);
    const pages = await pdf.pages;
    await pages[0].crop([0, 0, 100, 100]);
    expect(await handles()).toBeGreaterThan(before);
    await pdf.release();
    expect(await handles()).toBe(before);
    await pdf.release();
  });

  it("page(ref) of a cropped page's annotation is the cropped page", async () => {
    api.configure({});
    const pdf = await api.open(ANNOTATED);
    const [page] = await pdf.pages;
    const crop = await page.crop([0, 0, page.width, page.height]);
    const annots = (await crop.annots) as unknown as Annot[];
    expect(annots.length).toBeGreaterThan(0);
    expect(await pdf.page(annots[0].data.P)).toBe(crop);
    await expect(pdf.page({ handle: 1, page_number: 1 } as unknown as api.PageRef)).rejects.toThrow(TypeError);
    await pdf.release();
  });

  it("a stream is one object while it lives", async () => {
    api.configure({});
    const pdf = await api.open(join(pdfs, "pdffill-demo.pdf"));
    const [page] = await pdf.pages;
    const images = await page.images;
    if (images.length) {
      const objects = await page.objects;
      expect(objects.image[0].stream).toBe(images[0].stream);
    }
    await pdf.release();
  });

  it("an element found is an element of the tree", async () => {
    api.configure({});
    const pdf = await api.open(join(pdfs, "word365_structure.pdf"));
    const tree = await pdf.structTree();
    const found = await tree.findAll("P");
    expect(found.length).toBeGreaterThan(0);
    const all: api.PDFStructElement[] = [];
    const stack = [...tree.children];
    while (stack.length) {
      const el = stack.pop()!;
      all.push(el);
      stack.push(...el.children);
    }
    for (const el of found) expect(all).toContain(el);
    expect(await tree.find((el) => el.type === "P")).toBe(found[0]);
    await expect(tree.elementBbox({} as api.PDFStructElement)).rejects.toThrow(TypeError);
    const other = await pdf.structTree();
    await expect(other.elementBbox(tree.children[0])).rejects.toThrow(TypeError);
    expect(await other.elementBbox(other.children[0])).toEqual(await tree.elementBbox(tree.children[0]));
    const [page] = await pdf.pages;
    const another = await api.open(join(pdfs, "word365_structure.pdf"));
    await expect(another.structTree(page)).rejects.toThrow(TypeError);
    await another.release();
    await pdf.release();
  });

  it("unreachable derived pages, tables and trees free their handles", async () => {
    api.configure({});
    const pdf = await api.open(join(pdfs, "nics-background-checks-2015-11.pdf"));
    const [page] = await pdf.pages;
    await page.chars;
    await collect();
    const before = await handles();
    // (in a function of its own: the frame of a suspended async function
    // may keep its last values)
    const churn = async () => {
      for (let i = 0; i < 20; i++) {
        const crop = await page.crop([0, 0, 300 + i, 300]);
        await crop.findTables();
        await crop.dedupeChars();
      }
    };
    await churn();
    const word = await api.open(join(pdfs, "word365_structure.pdf"));
    const trees = async () => {
      for (let i = 0; i < 5; i++) await word.structTree();
    };
    const wordBefore = await handles();
    await trees();
    expect(await handles()).toBe(wordBefore + 5);
    expect(await handles()).toBeGreaterThan(before + 40);
    await collect();
    // only `word` itself is left besides what was there
    expect(await handles()).toBe(before + 1);
    // the page and the document live on
    expect((await page.chars).length).toBeGreaterThan(0);
    await pdf.release();
    await word.release();
  });

  it("a derived page a value names is kept (its handle is not freed)", async () => {
    api.configure({});
    const pdf = await api.open(ANNOTATED);
    const [page] = await pdf.pages;
    let annots: Annot[] | undefined = (await (await page.crop([0, 0, page.width, page.height])).annots) as unknown as Annot[];
    const ref = annots[0].data.P;
    annots = undefined;
    await collect();
    const crop = await pdf.page(ref);
    expect(crop.parent).toBe(page);
    expect((await crop.chars).length).toBeGreaterThan(0);
    await pdf.release();
  });

  it("no finalizer sends a free once its document is released or collected", async () => {
    api.configure({});
    const core = await lib.getCore();
    const frees: Promise<unknown>[] = [];
    const call = core.call.bind(core);
    const spy = vi.spyOn(core, "call").mockImplementation((op, ...rest) => {
      const r = call(op, ...rest);
      if (op === "free") frees.push(r);
      return r;
    });
    try {
      // released: the finalizers of its derived pages free nothing
      const a = await api.open(join(pdfs, "nics-background-checks-2015-11.pdf"));
      const [pa] = await a.pages;
      let crops: unknown[] | undefined = [];
      for (let i = 0; i < 10; i++) crops.push(await pa.crop([0, 0, 200 + i, 200]));
      await a.release();
      expect(frees.length).toBe(1);
      crops = undefined;
      await collect();
      expect(frees.length).toBe(1);
      void crops;
      // collected with its derived pages: no free of a handle the
      // document's free already took along (it would be refused)
      const make = async () => {
        const b = await api.open(join(pdfs, "nics-background-checks-2015-11.pdf"));
        const [pb] = await b.pages;
        const kept = [];
        for (let i = 0; i < 10; i++) kept.push(await pb.crop([0, 0, 200 + i, 200]));
        return kept.length;
      };
      expect(await make()).toBe(10);
      await collect();
      const outcomes = await Promise.allSettled(frees);
      expect(outcomes.length).toBeGreaterThan(1);
      expect(outcomes.filter((o) => o.status === "rejected")).toEqual([]);
    } finally {
      spy.mockRestore();
    }
  });

  it("finalizers never free a handle of a released document", async () => {
    api.configure({});
    const a = await api.open(join(pdfs, "nics-background-checks-2015-11.pdf"));
    const [pa] = await a.pages;
    let crops: unknown[] | undefined = [];
    for (let i = 0; i < 10; i++) crops.push(await pa.crop([0, 0, 200 + i, 200]));
    await a.release();
    // the handles of a's derived pages are free in the core: b's objects
    // take them
    const b = await api.open(join(pdfs, "nics-background-checks-2015-11.pdf"));
    const [pb] = await b.pages;
    const kept = [];
    for (let i = 0; i < 10; i++) kept.push(await pb.crop([0, 0, 200 + i, 200]));
    crops = undefined;
    await collect();
    for (const k of kept) expect((await k.chars).length).toBeGreaterThan(0);
    await b.release();
    void crops;
  });
});
