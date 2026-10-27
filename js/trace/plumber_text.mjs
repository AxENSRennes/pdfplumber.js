// Stage plumber_text through the public API: the traversal and the lines
// of oracle/stages/plumber_text.py. Search: the matches of Python's `re`
// (`ORACLE_INPUT`: the oracle's `spans` lines, by page number and op
// index, as the Rust stage reads them: crates/pdftrace/src/stages/
// plumber_text.rs `oracle_spans`) are given to `search()` through its
// `ORACLE` hook in place of the JavaScript `RegExp`'s, and each `spans`
// line is copied; an op the oracle has no `spans` line for (its trace
// stopped earlier: a depth window, whose RecursionError the API meets
// elsewhere) gets `None`, as in Rust.
import {
  bounded,
  canon,
  derive,
  exc,
  geometry,
  kwargs,
  lib,
  objectIndices,
  openPdf,
  select,
  tables,
  typed,
} from "./common.mjs";

/** The `spans <i> <spans>` lines of an oracle trace: `"<page number>
 * <i>"` -> their text, and the spans as values (`None` when Python's text
 * map raised). */
export function oracleSpans(trace) {
  const out = new Map();
  let page = "";
  for (const line of trace.split("\n")) {
    if (line.startsWith("page ")) {
      page = line.slice(5);
      continue;
    }
    const m = /^spans (\S+) (.*)$/.exec(line);
    if (!m) continue;
    // canon of a list of lists of tuples of ints or None
    const json = m[2].replace(/\(/g, "[").replace(/\)/g, "]").replace(/None/g, "null");
    out.set(`${page} ${m[1]}`, { text: m[2], spans: JSON.parse(json) });
  }
  return out;
}

/** The spans of an op the oracle has none for: `None`
 * (crates/pdftrace/src/stages/plumber_text.rs `ops_lines`). */
const NO_SPANS = { text: "None", spans: null };

const METHODS = {
  extract_text: "extractText",
  extract_text_simple: "extractTextSimple",
  extract_words: "extractWords",
  extract_text_lines: "extractTextLines",
};

/** `run_op(page, op)`; resolves to the typed result. */
async function runOp(page, op, spans) {
  const name = op[0];
  if (name === "search") {
    await page.search(op[1], { ...kwargs(op[2]), [lib.ORACLE]: { spans: spans ?? [], named: null } });
    return typed(page.pdf);
  }
  if (name === "dedupe_chars") {
    const deduped = await page.dedupeChars(kwargs(op[1]));
    // index = {id(c): i for i, c in enumerate(page.chars)}; [index[id(c)] for c in deduped.chars]
    await page.chars;
    await deduped.chars;
    return objectIndices(deduped, page, "char");
  }
  await page[METHODS[name]](kwargs(op[1]));
  return typed(page.pdf);
}

async function opsLines(lines, page, pageNumber, ops, counter, oracle) {
  for (const op of ops) {
    const i = counter.i++;
    lines.push(`op ${i} ` + canon(op));
    let spans;
    if (op[0] === "search") {
      const s = oracle.get(`${pageNumber} ${i}`) ?? NO_SPANS;
      lines.push(`spans ${i} ${s.text}`);
      spans = s.spans;
    }
    let value;
    try {
      value = await runOp(page, op, spans);
    } catch (e) {
      lines.push(`out ${i} ` + exc(e));
      continue;
    }
    lines.push(bounded("out", i, value));
  }
}

export async function run(lines, file) {
  const t = tables();
  const oracle = oracleSpans(file.oracle ?? "");
  let pdf;
  try {
    pdf = await openPdf(file.path, file.password, file.maxPages);
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
    for (let index = 0; index < pages.length; index++) {
      const page = pages[index];
      const pageNumber = geometry(page).get("page_number");
      lines.push(`page ${pageNumber}`);
      try {
        await page.chars;
      } catch (e) {
        lines.push("chars " + exc(e));
        continue;
      }
      const counter = { i: 0 };
      const ops = [...t.TEXT_BASE_OPS, ...select(file.sha, index, t.TEXT_VARIANT_OPS, t.TEXT_VARIANTS_PER_PAGE)];
      await opsLines(lines, page, pageNumber, ops, counter, oracle);
      for (const steps of select(file.sha, index, t.TEXT_DERIVED, t.TEXT_DERIVED_PER_PAGE)) {
        lines.push("derived " + canon(steps));
        let derived;
        try {
          derived = await derive(page, steps);
        } catch (e) {
          lines.push("derive " + exc(e));
          continue;
        }
        await opsLines(lines, derived, pageNumber, t.TEXT_DERIVED_OPS, counter, oracle);
      }
    }
  } finally {
    await pdf.release();
  }
}

export const ORACLE_INPUT = true;
