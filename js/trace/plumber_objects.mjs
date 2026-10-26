// Stage plumber_objects through the public API: the traversal and the
// lines of oracle/stages/plumber_objects.py. The `ids` lines are Python's
// `id()` ranking (`ORACLE_INPUT`): the second opening (`laparams={}`)
// takes them from the oracle's trace (`ORACLE` hook of `open()`) and each
// `ids` line is the ranking the analysis used (`oracle_page_ids`).
import {
  Names,
  ObjectsWriter,
  annotValue,
  boxLines,
  canon,
  digest,
  exc,
  geometry,
  lib,
  openPdf,
  pageIds,
  typed,
} from "./common.mjs";

/** The `ids` lines of an oracle trace: page number -> [[key, rank], ...]. */
export function oracleIds(trace) {
  const ids = {};
  for (const line of trace.split("\n")) {
    if (!line.startsWith("ids ")) continue;
    const rest = line.slice(4);
    const sp = rest.indexOf(" ");
    ids[rest.slice(0, sp)] = JSON.parse(rest.slice(sp + 1));
  }
  return ids;
}

async function objectsLines(lines, page, writer, refer) {
  let objects;
  try {
    await page.objects;
    objects = typed(page.pdf);
  } catch (e) {
    lines.push("objects " + exc(e));
    return false;
  }
  writer.write(lines, objects, refer);
  return true;
}

async function annotsLines(lines, label, item, get, names) {
  let annots;
  try {
    const pdf = await get();
    annots = typed(pdf);
  } catch (e) {
    lines.push(label + " " + exc(e));
    return;
  }
  lines.push(`${label} ${annots.length}`);
  annots.forEach((a, i) => lines.push(`${item} ${i} ` + canon(annotValue(a, names))));
}

async function pageLines(lines, page, writer) {
  lines.push(`page ${geometry(page).get("page_number")}`);
  boxLines(lines, page);
  await objectsLines(lines, page, writer, undefined);
  const names = new Names();
  await annotsLines(lines, "annots", "annot", async () => (await page.annots, page.pdf), names);
  await annotsLines(lines, "hyperlinks", "hyperlink", async () => (await page.hyperlinks, page.pdf), names);
}

async function pdfLines(lines, pdf) {
  try {
    await pdf.objects;
    const objs = typed(pdf);
    lines.push("pdf objects " + canon(objs.entries.map(([k, v]) => [k, v.length])));
  } catch (e) {
    lines.push("pdf objects " + exc(e));
  }
  const names = new Names();
  for (const what of ["annots", "hyperlinks"]) {
    let values;
    try {
      await pdf[what];
      values = typed(pdf);
    } catch (e) {
      lines.push(`pdf ${what} ` + exc(e));
      continue;
    }
    lines.push(`pdf ${what} ${values.length} ` + digest(values.map((a) => annotValue(a, names))));
  }
}

async function laparamsLines(lines, file, chars, oracle) {
  lines.push("laparams {}");
  let pdf;
  try {
    pdf = await openPdf(file.path, file.password, file.maxPages, { laparams: new lib.PyDict([]) }, { [lib.ORACLE]: { ids: oracle } });
  } catch (e) {
    lines.push("open " + exc(e));
    return;
  }
  try {
    let pages;
    try {
      pages = await pdf.pages;
    } catch (e) {
      lines.push("pages " + exc(e));
      return;
    }
    for (const page of pages) {
      const n = geometry(page).get("page_number");
      lines.push(`page ${n}`);
      const ok = await objectsLines(lines, page, new ObjectsWriter(new Names()), chars.get(String(n)) ?? new Map());
      if (ok) lines.push(`ids ${n} ` + canon(await pageIds(page)));
    }
  } finally {
    await pdf.release();
  }
}

/** The trace lines (without the final `!end` or exception line, which
 * the runner adds). */
export async function run(lines, file) {
  const oracle = oracleIds(file.oracle ?? "");
  let pdf;
  try {
    pdf = await openPdf(file.path, file.password, file.maxPages);
  } catch (e) {
    lines.push("open " + exc(e));
    return;
  }
  const chars = new Map();
  try {
    let pages = [];
    try {
      pages = await pdf.pages;
    } catch (e) {
      lines.push("pages " + exc(e));
    }
    for (const page of pages) {
      const writer = new ObjectsWriter(new Names());
      await pageLines(lines, page, writer);
      chars.set(String(geometry(page).get("page_number")), writer.chars);
    }
    await pdfLines(lines, pdf);
  } finally {
    await pdf.release();
  }
  await laparamsLines(lines, file, chars, oracle);
}

export const ORACLE_INPUT = true;
