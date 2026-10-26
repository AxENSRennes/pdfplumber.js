#!/usr/bin/env node
// THIRD_PARTY_NOTICES (shipped in the npm package): the notices of what
// dist/pdfplumber.wasm and dist/cmaps carry from other projects, as their
// licenses ask of a redistribution in binary form.
//
//   node scripts/third-party-notices.mjs           # writes THIRD_PARTY_NOTICES
//   node scripts/third-party-notices.mjs --check   # checks it (check.sh)
//
// Written from: the upstream sources of the port (PDFPLUMBER_ROOT/upstream:
// pdfminer.six, pdfplumber, Adobe's CMap resources as pdfminer ships them,
// ARM's optimized-routines), CPython 3.12's license (the algorithms the
// port reproduces: list.sort, float repr and round, unicodedata tables...),
// zlib's (the inflate port), and the license files of the Rust crates the
// WebAssembly module links (`cargo tree` of crates/wasm for
// wasm32-unknown-unknown, normal dependencies). `--check` checks that the
// file lists exactly those crates, and that the module does not contain the
// port of glibc's atan2 (LGPL-2.1-or-later, never shipped: its tables are
// looked for in dist/pdfplumber.wasm).
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const UPSTREAM = join(process.env.PDFPLUMBER_ROOT ?? ROOT, "upstream");
const OUT = join(ROOT, "THIRD_PARTY_NOTICES");
const cargo = existsSync(join(homedir(), ".cargo/bin/cargo")) ? join(homedir(), ".cargo/bin/cargo") : "cargo";

/** The crates of the module: `[name, version, license]`. */
function crates() {
  const out = execFileSync(
    cargo,
    ["tree", "-p", "pdfplumber-wasm", "--target", "wasm32-unknown-unknown", "-e", "normal", "--prefix", "none", "--format", "{p}|{l}"],
    { cwd: ROOT, encoding: "utf8" },
  );
  const seen = new Map();
  for (const line of out.split("\n")) {
    const m = /^(\S+) v(\S+)(?: \([^)]*\))?(?: \(\*\))?\|(.*)$/.exec(line.trim());
    if (!m || m[2] === "0.0.0") continue;
    seen.set(`${m[1]} ${m[2]}`, [m[1], m[2], m[3].replace(/ \(\*\)$/, "")]);
  }
  return [...seen.values()].sort((a, b) => a[0].localeCompare(b[0]));
}

const heading = (title) => `${"=".repeat(78)}\n${title}\n${"=".repeat(78)}\n\n`;

const ZLIB = `  (C) 1995-2023 Jean-loup Gailly and Mark Adler

  This software is provided 'as-is', without any express or implied
  warranty.  In no event will the authors be held liable for any damages
  arising from the use of this software.

  Permission is granted to anyone to use this software for any purpose,
  including commercial applications, and to alter it and redistribute it
  freely, subject to the following restrictions:

  1. The origin of this software must not be misrepresented; you must not
     claim that you wrote the original software. If you use this software
     in a product, an acknowledgment in the product documentation would be
     appreciated but is not required.
  2. Altered source versions must be plainly marked as such, and must not be
     misrepresented as being the original software.
  3. This notice may not be removed or altered from any source distribution.
`;

function registryDir(name, version) {
  const base = join(homedir(), ".cargo/registry/src");
  for (const index of readdirSync(base)) {
    const dir = join(base, index, `${name}-${version}`);
    if (existsSync(dir)) return dir;
  }
  throw new Error(`no source of ${name} ${version} in ${base}`);
}

function write() {
  const read = (rel) => readFileSync(join(UPSTREAM, rel), "utf8").trimEnd() + "\n";
  const cmaps = read("pdfminer-six/cmaprsrc/README.txt");
  const adobe = cmaps.slice(cmaps.indexOf("%%Copyright: ---"));
  const psf = readFileSync("/usr/lib/python3.12/LICENSE.txt", "utf8");
  const psfAgreement = psf.slice(psf.indexOf("PYTHON SOFTWARE FOUNDATION LICENSE VERSION 2"), psf.indexOf("BEOPEN.COM LICENSE AGREEMENT")).trimEnd() + "\n";
  const arm = read("arm-optimized-routines/LICENSE");
  const armMit = arm.slice(arm.indexOf("MIT License"), arm.indexOf("Apache-2.0 WITH LLVM-exception\n---")).trimEnd() + "\n";
  let text = `pdfplumber-js: third-party notices

The WebAssembly module (dist/pdfplumber.wasm) is a port of pdfminer.six
and pdfplumber to Rust; it reproduces algorithms of CPython 3.12 and
zlib 1.3, includes ARM's optimized-routines pow tables, and links the Rust
crates listed below. The CMap files (dist/cmaps) are pdfminer.six's CMap
data, made from Adobe's CMap resources. Their notices follow.

`;
  text += heading("pdfminer.six (MIT License)") + read("pdfminer-six/LICENSE") + "\n";
  text += heading("pdfplumber (MIT License)") + read("pdfplumber-python/LICENSE.txt") + "\n";
  text += heading("Adobe CMap resources (dist/cmaps, through pdfminer.six's cmap data)") + adobe + "\n";
  text += heading("ARM optimized-routines (pow; MIT OR Apache-2.0 WITH LLVM-exception, used under MIT)") + armMit + "\n";
  text +=
    heading("CPython 3.12 (algorithms reproduced: Python Software Foundation License Version 2)") +
    "Copyright (c) 2001-2023 Python Software Foundation. All Rights Reserved.\n\n" +
    "Summary of the changes: the port reimplements in Rust, to give the same\n" +
    "results, parts of CPython 3.12 (list.sort, float repr and round, int and\n" +
    "float conversions, str methods, unicodedata's tables, json and csv\n" +
    "formatting, the recursion limits); no CPython source is shipped.\n\n" +
    psfAgreement +
    "\n";
  text += heading("zlib 1.3 (inflate, ported to Rust: zlib License)") + ZLIB + "\n";
  for (const [name, version, license] of crates()) {
    const dir = registryDir(name, version);
    const file = ["LICENSE-MIT", "LICENSE", "LICENSE.txt", "LICENSE.md"].map((f) => join(dir, f)).find((f) => existsSync(f));
    if (!file) throw new Error(`no license file in ${dir}`);
    text += heading(`Rust crate ${name} ${version} (${license}; used under MIT)`) + readFileSync(file, "utf8").trimEnd() + "\n\n";
  }
  writeFileSync(OUT, text);
  console.log(`wrote ${OUT}`);
}

/** The glibc atan2 tables (crates/pycompat/src/atan2_tables.rs) found in
 * the module, as little-endian f64 bytes. */
function atan2Constants(wasm) {
  const src = readFileSync(join(ROOT, "crates/pycompat/src/atan2_tables.rs"), "utf8");
  const values = [...src.matchAll(/0x([0-9a-f]{16})/g)].map((m) => m[1]).filter((v, i, a) => a.indexOf(v) === i);
  const hex = Buffer.from(wasm).toString("hex");
  let found = 0;
  for (const v of values) {
    const le = v.match(/../g).reverse().join("");
    if (hex.includes(le)) found++;
  }
  return { found, total: values.length };
}

function check() {
  const text = readFileSync(OUT, "utf8");
  const listed = [...text.matchAll(/^Rust crate (\S+) (\S+) /gm)].map((m) => `${m[1]} ${m[2]}`).sort();
  const linked = crates().map(([n, v]) => `${n} ${v}`).sort();
  if (listed.join("\n") !== linked.join("\n")) {
    console.error(`THIRD_PARTY_NOTICES lists ${listed.join(", ")}\nthe module links ${linked.join(", ")}\nrun node scripts/third-party-notices.mjs`);
    process.exit(1);
  }
  for (const title of ["pdfminer.six", "pdfplumber (MIT", "Adobe CMap", "ARM optimized-routines", "CPython 3.12", "zlib 1.3"]) {
    if (!text.includes(title)) {
      console.error(`THIRD_PARTY_NOTICES has no notice of ${title}`);
      process.exit(1);
    }
  }
  const wasm = join(ROOT, "dist/pdfplumber.wasm");
  if (existsSync(wasm)) {
    const { found, total } = atan2Constants(readFileSync(wasm));
    // (a few of its constants are common ones: pi/2, 1/16...)
    if (found > total / 4) {
      console.error(`dist/pdfplumber.wasm holds ${found} of the ${total} constants of glibc's atan2 (LGPL): its notice is missing`);
      process.exit(1);
    }
    console.log(`THIRD_PARTY_NOTICES: ${linked.length} crates; glibc's atan2: ${found}/${total} constants in the module (not linked)`);
  }
}

if (process.argv.includes("--check")) check();
else write();
