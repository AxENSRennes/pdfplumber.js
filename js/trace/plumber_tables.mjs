// Stage plumber_tables through the public API: the traversal and the lines
// of oracle/stages/plumber_tables.py. `debug_tablefinder` is
// `page.debugTablefinder()` (its edges, intersections, cells and tables).
import { bounded, canon, derive, digest, exc, geometry, items, kwargs, lib, openPdf, pyArith, select, tables, typed } from "./common.mjs";

/** `settings_of(page, op)`: a plain object of typed values. */
async function settingsOf(page, op) {
  const [name, settings] = op;
  if (name === "settings") return kwargs(settings);
  const out = kwargs(new lib.PyDict(settings.entries.filter(([k]) => !["v", "h", "v_objects", "h_objects"].includes(k))));
  const [x0, top, x1, bottom] = items(geometry(page).get("bbox"));
  await page.objects;
  const objects = typed(page.pdf);
  const get = (k) => (objects.has(k) ? objects.get(k) : []);
  const objs = [...get("rect"), ...get("line"), ...get("curve")];
  const v = settings.get("v");
  const h = settings.get("h");
  const vObjects = settings.get("v_objects");
  const hObjects = settings.get("h_objects");
  if (v.length || vObjects)
    out.explicit_vertical_lines = [...v.map((f) => pyArith("+", x0, pyArith("*", pyArith("-", x1, x0), f))), ...objs.slice(0, vObjects)];
  if (h.length || hObjects)
    out.explicit_horizontal_lines = [...h.map((f) => pyArith("+", top, pyArith("*", pyArith("-", bottom, top), f))), ...objs.slice(0, hObjects)];
  return out;
}

const edge = (e) => new lib.PyTuple([e.get("x0"), e.get("top"), e.get("x1"), e.get("bottom"), e.get("orientation"), e.has("object_type") ? e.get("object_type") : null]);
const objBbox = (e) => new lib.PyTuple([e.get("x0"), e.get("top"), e.get("x1"), e.get("bottom")]);

async function finderLines(lines, page, i, settings) {
  let finder;
  try {
    finder = await page.debugTablefinder(settings);
  } catch (e) {
    lines.push(`finder ${i} ` + exc(e));
    finder = undefined;
  }
  if (finder) {
    const f = typed(page.pdf);
    const fedges = f.get("edges");
    lines.push(bounded("fedges", i, fedges.map(edge)));
    lines.push(`fedges_all ${i} ${fedges.length} ` + digest(fedges));
    lines.push(
      bounded(
        "intersections",
        i,
        f.get("intersections").map((entry) => {
          const [pt, d] = items(entry);
          return new lib.PyTuple([pt, d.get("v").map(objBbox), d.get("h").map(objBbox)]);
        }),
      ),
    );
    lines.push(bounded("cells", i, f.get("cells")));
    const tablesTyped = f.get("tables");
    // TableSettings.resolve(settings).text_settings or {}: the finder's
    // resolved settings (`finder.settings`)
    const textSettings = f.get("settings").get("text_settings");
    for (let t = 0; t < finder.tables.length; t++) {
      const table = tablesTyped[t];
      lines.push(`table ${i} ${t} ` + canon(table.get("bbox")) + ` ${table.get("cells").length}`);
      lines.push(bounded("rows", `${i} ${t}`, table.get("rows").map((r) => new lib.PyTuple([r.get("cells"), r.get("bbox")]))));
      lines.push(bounded("columns", `${i} ${t}`, table.get("columns").map((c) => new lib.PyTuple([c.get("cells"), c.get("bbox")]))));
      try {
        await finder.tables[t].extract(textSettings instanceof lib.PyDict ? kwargs(textSettings) : {});
        lines.push(bounded("extract", `${i} ${t}`, typed(page.pdf)));
      } catch (e) {
        lines.push(`extract ${i} ${t} ` + exc(e));
      }
    }
  }
  try {
    const found = await page.findTable(settings);
    lines.push(`find_table ${i} ` + canon(found === null ? null : typed(page.pdf).get("bbox")));
  } catch (e) {
    lines.push(`find_table ${i} ` + exc(e));
  }
  for (const [name, method] of [
    ["extract_tables", "extractTables"],
    ["extract_table", "extractTable"],
  ]) {
    try {
      await page[method](settings);
      lines.push(bounded(name, i, typed(page.pdf)));
    } catch (e) {
      lines.push(`${name} ${i} ` + exc(e));
    }
  }
}

async function opLines(lines, page, op, counter) {
  const i = counter.i++;
  lines.push(`op ${i} ` + canon(op));
  let settings;
  try {
    settings = await settingsOf(page, op);
  } catch (e) {
    lines.push(`finder ${i} ` + exc(e));
    return;
  }
  await finderLines(lines, page, i, settings);
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
    let pages;
    try {
      pages = await pdf.pages;
    } catch (e) {
      lines.push("pages " + exc(e));
      return;
    }
    for (let index = 0; index < pages.length; index++) {
      const page = pages[index];
      lines.push(`page ${geometry(page).get("page_number")}`);
      let edges;
      try {
        await page.edges;
        edges = typed(pdf);
      } catch (e) {
        lines.push("edges " + exc(e));
        continue;
      }
      lines.push(`edges ${edges.length} ` + digest(edges));
      await page.horizontal_edges;
      const h = typed(pdf);
      await page.vertical_edges;
      const v = typed(pdf);
      lines.push(`hedges ${h.length} ` + digest(h));
      lines.push(`vedges ${v.length} ` + digest(v));
      const counter = { i: 0 };
      for (const op of [...t.TABLES_BASE_OPS, ...select(file.sha, index, t.TABLES_VARIANT_OPS, t.TABLES_VARIANTS_PER_PAGE)])
        await opLines(lines, page, op, counter);
      for (const [steps, op] of select(file.sha, index, t.TABLES_DERIVED_OPS, 1)) {
        lines.push("derived " + canon(steps));
        let derived;
        try {
          derived = await derive(page, steps);
        } catch (e) {
          lines.push("derive " + exc(e));
          continue;
        }
        await opLines(lines, derived, op, counter);
      }
    }
  } finally {
    await pdf.release();
  }
}
