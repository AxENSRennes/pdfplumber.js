// The unit data a JavaScript test replays (docs/verification.md): the
// directory <cache>/unit/<module>/<fingerprint>/ whose meta.json lists the
// oracle sources of this checkout, the most recent one, as
// pdftrace::unit::UnitData picks it on the Rust side.
import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
export const PDFPLUMBER_ROOT = process.env.PDFPLUMBER_ROOT ?? ROOT;
export const CACHE = process.env.PDFPLUMBER_CACHE ?? join(PDFPLUMBER_ROOT, ".cache");

const sha256 = (path) => createHash("sha256").update(readFileSync(path)).digest("hex");

/** The data directory of a generator, or an error naming the command
 * that writes it. */
export function oracleDir(module) {
  const base = join(CACHE, "unit", module);
  const how = `PDFPLUMBER_ROOT=${PDFPLUMBER_ROOT} PYTHONPATH=${ROOT} ${PDFPLUMBER_ROOT}/.venv/bin/python -m oracle.unit.${module}`;
  const candidates = existsSync(base) ? readdirSync(base) : [];
  let best;
  for (const name of candidates) {
    const meta = join(base, name, "meta.json");
    if (!existsSync(meta)) continue;
    const sources = JSON.parse(readFileSync(meta, "utf8")).sources ?? {};
    const ok = Object.entries(sources).every(([path, sha]) => existsSync(join(ROOT, path)) && sha256(join(ROOT, path)) === sha);
    if (!ok) continue;
    const mtime = statSync(meta).mtimeMs;
    if (!best || mtime > best.mtime) best = { dir: join(base, name), mtime };
  }
  if (!best) throw new Error(`no unit data of ${module} for this checkout: run\n  ${how}`);
  return best.dir;
}

/** The lines of <name>.jsonl, parsed (typed JSON, see typed.mjs). */
export function oracleLines(module, name) {
  return readFileSync(join(oracleDir(module), `${name}.jsonl`), "utf8")
    .split("\n")
    .filter(Boolean)
    .map((l) => JSON.parse(l));
}
