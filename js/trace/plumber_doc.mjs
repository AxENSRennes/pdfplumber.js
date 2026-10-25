// Stage plumber_doc through the public API: the traversal and the lines of
// oracle/stages/plumber_doc.py. `PDFStructTree(pdf, page)` is
// `pdf.structTree(page)` (findAll, find, allMcids, elementBbox, repr).
import {
  Names,
  annotValue,
  canon,
  derive,
  digest,
  exc,
  geometry,
  kwargs,
  lib,
  openPdf,
  select,
  sha256Hex,
  tables,
  typed,
} from "./common.mjs";

/** `bounded(label, value)` of plumber_doc.py. */
function bounded(label, value) {
  const text = canon(value);
  if (text.length > tables().OUT_LIMIT) return `${label}d ${text.length} ${sha256Hex(text).slice(0, 16)}`;
  return `${label} ${text}`;
}

/** `objects_digest(page)` */
async function objectsDigest(page) {
  const names = new Names();
  await page.objects;
  const objects = typed(page.pdf);
  names.learn(objects);
  return objects.entries.map(([kind, objs]) => [kind, objs.length, digest(objs.map((o) => names.obj(o)))]);
}

/** `page_names(page)` */
async function pageNames(page) {
  const names = new Names();
  try {
    await page.objects;
    names.learn(typed(page.pdf));
  } catch (e) {
    if (!(e instanceof lib.PDFPlumberError)) throw e;
  }
  return names;
}

/** `_paths(children)`: the first MAX_PROBED elements, depth-first, with
 * their paths. */
function paths(children, max) {
  const out = [];
  const stack = children.map((el, i) => [[i], el]).reverse();
  while (stack.length && out.length < max) {
    const [path, el] = stack.pop();
    out.push([path, el]);
    stack.push(...el.children.map((c, j) => [[...path, j], c]).reverse());
  }
  return out;
}

async function treeLines(lines, label, getList, makeTree, pdf) {
  const t = tables();
  let elements;
  try {
    await getList();
    elements = typed(pdf);
  } catch (e) {
    lines.push(`${label} ` + exc(e));
  }
  // (outside the try, as Python's `else:` clause)
  if (elements !== undefined) {
    lines.push(`${label} ${elements.length} ` + digest(elements));
    elements.slice(0, t.DOC_MAX_ELEMENTS).forEach((el, i) => lines.push(bounded(`struct ${i}`, el)));
  }
  let tree;
  try {
    tree = await makeTree();
  } catch (e) {
    lines.push("tree " + exc(e));
    return;
  }
  const tag = tree.children.length ? tree.children[0].type : "P";
  const matchers = [
    [["tag", tag], tag],
    [["tag", "Span"], "Span"],
    [["function", "starts-with-H"], (el) => el.type.startsWith("H")],
  ];
  for (const [desc, matcher] of matchers) {
    try {
      const found = [];
      for (const el of await tree.findAll(matcher)) {
        if (found.length === t.DOC_MAX_FOUND) {
          found.push("...");
          break;
        }
        found.push(el.type);
      }
      lines.push("find_all " + canon(desc) + " " + canon(found));
    } catch (e) {
      lines.push("find_all " + canon(desc) + " " + exc(e));
    }
    try {
      const el = await tree.find(matcher);
      lines.push("find " + canon(desc) + " " + canon(el === null ? null : el.type));
    } catch (e) {
      lines.push("find " + canon(desc) + " " + exc(e));
    }
  }
  for (const [path, el] of paths(tree.children, t.DOC_MAX_PROBED)) {
    const p = new lib.PyTuple(path);
    try {
      await el.allMcids();
      lines.push("mcids " + canon(p) + " " + canon(typed(pdf)));
    } catch (e) {
      lines.push("mcids " + canon(p) + " " + exc(e));
    }
    try {
      await tree.elementBbox(el);
      lines.push("element_bbox " + canon(p) + " " + canon(typed(pdf)));
    } catch (e) {
      lines.push("element_bbox " + canon(p) + " " + exc(e));
    }
  }
  if (tree.children.length) {
    try {
      const text = await tree.children[0].repr();
      const bytes = lib.encodeGeneralizedUtf8(text);
      lines.push(`element_repr ${[...text].length} ` + sha256Hex(bytes).slice(0, 16));
    } catch (e) {
      lines.push("element_repr " + exc(e));
    }
  }
}

async function derivedLines(lines, page, steps) {
  lines.push("derived " + canon(steps));
  let d;
  try {
    d = await derive(page, steps);
  } catch (e) {
    lines.push("derive " + exc(e));
    return;
  }
  // d.width, d.height raise out of the stage as in Python
  void d.width;
  void d.height;
  const g = geometry(d);
  lines.push("dbox " + [g.get("bbox"), g.get("width"), g.get("height"), String(d)].map(canon).join(" "));
  try {
    lines.push("dobjects " + canon(await objectsDigest(d)));
  } catch (e) {
    lines.push("dobjects " + exc(e));
  }
  let annots;
  try {
    await d.annots;
    annots = typed(d.pdf);
  } catch (e) {
    lines.push("dannots " + exc(e));
    return;
  }
  // (outside the try, as Python's `else:` clause: an annotation the
  // harness cannot write raises out of the stage)
  const names = new Names();
  lines.push(`dannots ${annots.length} ` + digest(annots.map((a) => annotValue(a, names))));
}

/** `page_dict(value, names)` */
function pageDict(value, names) {
  return new lib.PyDict(
    value.entries.map(([k, v]) => [k, Array.isArray(v) ? v.map((a) => (k === "annots" ? annotValue(a, names) : names.obj(a))) : v]),
  );
}

const SERIALIZERS = { to_dict: "toDict", to_json: "toJson", to_csv: "toCsv" };

async function serializeLines(lines, page, op) {
  const [method, kw] = op;
  const label = "serialize " + canon(op);
  let value;
  try {
    if (method === "to_dict") await page.toDict(kw.has("object_types") ? kw.get("object_types") : undefined);
    else await page[SERIALIZERS[method]](kwargs(kw));
    value = typed(page.pdf);
  } catch (e) {
    lines.push(label + " " + exc(e));
    return;
  }
  const names = await pageNames(page);
  value = method === "to_dict" ? pageDict(value, names) : names.text(value);
  const text = canon(value);
  if (text.length > tables().OUT_LIMIT) lines.push(`serialized ${canon(op)} ${text.length} ` + sha256Hex(text).slice(0, 16));
  else lines.push(`${label} ${text}`);
}

async function pageLines(lines, pdf, page, sha, index) {
  const t = tables();
  lines.push(`page ${geometry(page).get("page_number")} ` + canon(String(page)));
  await treeLines(lines, "page_structure_tree", () => page.structure_tree, () => pdf.structTree(page), pdf);
  for (const steps of [...t.DOC_DERIVED_BASE, ...select(sha, index, t.DOC_DERIVED, t.DOC_DERIVED_PER_PAGE)])
    await derivedLines(lines, page, steps);
  for (const op of select(sha, index, t.DOC_SERIALIZE, t.DOC_SERIALIZE_PER_PAGE)) await serializeLines(lines, page, op);
}

async function optionLines(lines, file, options) {
  const t = tables();
  lines.push("options " + canon(options));
  let pdf;
  if (options.has("pages")) {
    try {
      pdf = await lib.open(file.path, { password: file.password, ...kwargs(options) });
    } catch (e) {
      lines.push("open " + exc(e));
      return;
    }
    try {
      const pages = await pdf.pages;
      lines.push("selected " + canon(pages.map((p) => new lib.PyTuple([geometry(p).get("page_number"), geometry(p).get("initial_doctop")]))));
    } catch (e) {
      lines.push("selected " + exc(e));
    } finally {
      await pdf.release();
    }
    return;
  }
  try {
    pdf = await openPdf(file.path, file.password, file.maxPages, kwargs(options));
  } catch (e) {
    lines.push("open " + exc(e));
    return;
  }
  try {
    if (options.has("strict_metadata")) {
      await pdf.metadata;
      lines.push("metadata " + canon(typed(pdf)));
      return;
    }
    let pages;
    try {
      pages = await pdf.pages;
    } catch (e) {
      lines.push("pages " + exc(e));
      return;
    }
    if (options.has("unicode_norm")) {
      for (const page of pages.slice(0, t.DOC_NORM_PAGES)) {
        const n = geometry(page).get("page_number");
        try {
          await page.chars;
          lines.push(bounded(`texts ${n}`, typed(pdf).map((c) => c.get("text"))));
        } catch (e) {
          lines.push(`texts ${n} ` + exc(e));
        }
      }
      return;
    }
    for (const page of pages) {
      const n = geometry(page).get("page_number");
      let annots;
      try {
        await page.annots;
        annots = typed(pdf);
      } catch (e) {
        lines.push(`annots ${n} ` + exc(e));
        continue;
      }
      const names = new Names();
      lines.push(`annots ${n} ${annots.length} ` + digest(annots.map((a) => annotValue(a, names))));
    }
  } finally {
    await pdf.release();
  }
}

async function passwordLines(lines, file) {
  const t = tables();
  for (const other of [...(file.password !== null ? [null] : []), t.DOC_WRONG_PASSWORD]) {
    try {
      const pdf = await openPdf(file.path, other, file.maxPages);
      await pdf.release();
      lines.push("password " + canon(other) + " ok");
    } catch (e) {
      lines.push("password " + canon(other) + " " + exc(e));
    }
  }
}

export async function run(lines, file) {
  const t = tables();
  let pdf;
  try {
    pdf = await openPdf(file.path, file.password, file.maxPages);
  } catch (e) {
    lines.push("open " + exc(e));
    return;
  }
  try {
    await pdf.metadata;
    lines.push("metadata " + canon(typed(pdf)));
    await treeLines(lines, "structure_tree", () => pdf.structure_tree, () => pdf.structTree(), pdf);
    let pages = [];
    try {
      pages = await pdf.pages;
    } catch (e) {
      lines.push("pages " + exc(e));
    }
    for (let index = 0; index < pages.length; index++) await pageLines(lines, pdf, pages[index], file.sha, index);
    try {
      await pdf.toDict(["char", "rect", "line", "curve", "image"]);
      const value = typed(pdf);
      const dicts = value.get("pages");
      const again = await pdf.pages;
      const mapped = [];
      for (let k = 0; k < Math.min(again.length, dicts.length); k++) mapped.push(pageDict(dicts[k], await pageNames(again[k])));
      const out = new lib.PyDict(value.entries.map(([k, v]) => [k, k === "pages" ? mapped : v]));
      if (!value.has("pages")) out.entries.push(["pages", mapped]);
      lines.push("pdf_to_dict " + digest(out));
    } catch (e) {
      lines.push("pdf_to_dict " + exc(e));
    }
    try {
      await pdf.close();
      lines.push("close ok");
    } catch (e) {
      lines.push("close " + exc(e));
    }
    try {
      const reopened = await pdf.pages;
      if (reopened.length) lines.push("closed_objects " + canon(await objectsDigest(reopened[0])));
    } catch (e) {
      lines.push("closed_objects " + exc(e));
    }
  } finally {
    await pdf.release();
  }
  for (const options of t.DOC_OPEN_OPTIONS) await optionLines(lines, file, options);
  await passwordLines(lines, file);
}
