"""Generate crates/pdfminer/src/saslprep_tables.rs from the running CPython.

``pdfminer/_saslprep.py`` calls the ``stringprep`` module and
``unicodedata.ucd_3_2_0.normalize("NFKC", ...)``; both read CPython's Unicode
3.2.0 view of its database (``stringprep`` does ``from unicodedata import
ucd_3_2_0 as unicodedata``). This script evaluates them on every code point
and writes what the Rust port needs:

- one range table per ``stringprep.in_table_*`` predicate that ``saslprep``
  uses (``a1``, ``b1``, ``c12``, ``c21_c22``, ``c3`` ... ``c9``, ``d1``,
  ``d2``), computed by calling the predicate itself;
- the data of ``ucd_3_2_0.normalize`` (``Modules/unicodedata.c``
  ``nfd_nfkd``/``nfc_nfkc``): the decompositions ``ucd_3_2_0.decomposition``
  returns (empty for a code point unassigned in Unicode 3.2.0, as
  ``get_decomp_record``), the "normalization corrections" of the 3.2.0
  database (``PreviousDBVersion.normalization``: derived as the code points
  whose ``ucd_3_2_0.normalize("NFD")`` differs from their decomposition),
  the canonical combining classes (``_getrecord_ex``: the current database,
  also for UCD 3.2.0) and the canonical composition pairs (``nfc_first``,
  ``nfc_last``, ``comp_data``: the current database; a pair is the
  canonical two-character decomposition of a code point that NFC leaves
  alone).

Before writing, the script re-runs the normalization from these tables in
Python on every code point and compares it with ``ucd_3_2_0.normalize``.
Run with the oracle's Python (3.12, Unicode 15.0)::

    .venv/bin/python oracle/codegen/saslprep_tables.py [--check]

``--check`` exits 1 if the file is not up to date.
"""

from __future__ import annotations

import argparse
import stringprep
import sys
import unicodedata
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE.parent.parent / "crates" / "pdfminer" / "src" / "saslprep_tables.rs"

UCD = unicodedata.ucd_3_2_0

#: The predicates of pdfminer/_saslprep.py, by table name.
PREDICATES = {
    "A1": stringprep.in_table_a1,
    "B1": stringprep.in_table_b1,
    "C12": stringprep.in_table_c12,
    "C21_C22": stringprep.in_table_c21_c22,
    "C3": stringprep.in_table_c3,
    "C4": stringprep.in_table_c4,
    "C5": stringprep.in_table_c5,
    "C6": stringprep.in_table_c6,
    "C7": stringprep.in_table_c7,
    "C8": stringprep.in_table_c8,
    "C9": stringprep.in_table_c9,
    "D1": stringprep.in_table_d1,
    "D2": stringprep.in_table_d2,
}

# Modules/unicodedata.c (Hangul syllables, decomposed and composed by
# arithmetic, not by tables)
S_BASE, L_BASE, V_BASE, T_BASE = 0xAC00, 0x1100, 0x1161, 0x11A7
L_COUNT, V_COUNT, T_COUNT = 19, 21, 28
N_COUNT = V_COUNT * T_COUNT
S_COUNT = L_COUNT * N_COUNT


def ranges(pred) -> list[tuple[int, int]]:
    out: list[tuple[int, int]] = []
    start = None
    for c in range(0x110000):
        if pred(c):
            if start is None:
                start = c
        elif start is not None:
            out.append((start, c - 1))
            start = None
    if start is not None:
        out.append((start, 0x10FFFF))
    return out


def predicate_ranges() -> dict[str, list[tuple[int, int]]]:
    return {name: ranges(lambda c, p=pred: p(chr(c))) for name, pred in PREDICATES.items()}


def decompositions() -> dict[int, tuple[bool, list[int]]]:
    """``ucd_3_2_0.decomposition``, parsed: (compatibility?, code points)."""
    out = {}
    for c in range(0x110000):
        d = UCD.decomposition(chr(c))
        if not d:
            continue
        parts = d.split()
        compat = parts[0].startswith("<")
        if compat:
            parts = parts[1:]
        out[c] = (compat, [int(p, 16) for p in parts])
    return out


def combining_runs() -> list[tuple[int, int, int]]:
    runs: list[tuple[int, int, int]] = []
    for c in range(0x110000):
        ccc = unicodedata.combining(chr(c))
        if not ccc:
            continue
        if runs and runs[-1][1] == c - 1 and runs[-1][2] == ccc:
            runs[-1] = (runs[-1][0], c, ccc)
        else:
            runs.append((c, c, ccc))
    return runs


def composition_pairs() -> list[tuple[int, int, int]]:
    pairs = []
    for c in range(0x110000):
        d = unicodedata.decomposition(chr(c))
        if not d or d.startswith("<"):
            continue
        parts = d.split()
        if len(parts) == 2 and unicodedata.normalize("NFC", chr(c)) == chr(c):
            pairs.append((int(parts[0], 16), int(parts[1], 16), c))
    pairs.sort()
    return pairs


class Model:
    """``nfd_nfkd``/``nfc_nfkc`` of UCD 3.2.0 on the generated tables (the
    algorithm the Rust port implements), to check the tables."""

    def __init__(self, decomp, corrections, ccc, pairs):
        self.decomp = decomp
        self.corrections = corrections
        self.ccc = ccc
        self.comp = {(a, b): c for a, b, c in pairs}
        self.first = {a for a, _, _ in pairs}
        self.last = {b for _, b, _ in pairs}

    def nfd(self, s: list[int], k: bool, corrections: bool = True) -> list[int]:
        out: list[int] = []
        for ch in s:
            stack = [ch]
            while stack:
                code = stack.pop()
                if S_BASE <= code < S_BASE + S_COUNT:
                    si = code - S_BASE
                    out += [L_BASE + si // N_COUNT, V_BASE + (si % N_COUNT) // T_COUNT]
                    if si % T_COUNT:
                        out.append(T_BASE + si % T_COUNT)
                    continue
                if corrections and code in self.corrections:
                    stack.append(self.corrections[code])
                    continue
                rec = self.decomp.get(code)
                if rec is None or (rec[0] and not k):
                    out.append(code)
                    continue
                stack.extend(reversed(rec[1]))
        # canonical ordering (stable by combining class within runs)
        i = 1
        while i < len(out):
            prev, cur = self.ccc.get(out[i - 1], 0), self.ccc.get(out[i], 0)
            if prev and cur and prev > cur:
                o = i - 1
                while True:
                    out[o], out[o + 1] = out[o + 1], out[o]
                    o -= 1
                    if o < 0 or not self.ccc.get(out[o], 0) or self.ccc.get(out[o], 0) <= cur:
                        break
            i += 1
        return out

    def nfkc(self, s: list[int]) -> list[int]:
        data = self.nfd(s, True)
        out: list[int] = []
        skipped: list[int] = []
        i = 0
        n = len(data)
        while i < n:
            if i in skipped:
                skipped.remove(i)
                i += 1
                continue
            code = data[i]
            if L_BASE <= code < L_BASE + L_COUNT and i + 1 < n and V_BASE <= data[i + 1] < V_BASE + V_COUNT:
                code = S_BASE + ((code - L_BASE) * V_COUNT + data[i + 1] - V_BASE) * T_COUNT
                i += 2
                if i < n and T_BASE < data[i] < T_BASE + T_COUNT:
                    code += data[i] - T_BASE
                    i += 1
                out.append(code)
                continue
            if code not in self.first:
                out.append(code)
                i += 1
                continue
            i1 = i + 1
            comb = 0
            cur = code
            while i1 < n:
                code1 = data[i1]
                comb1 = self.ccc.get(code1, 0)
                if comb:
                    if comb1 == 0:
                        break
                    if comb >= comb1:
                        i1 += 1
                        continue
                composed = self.comp.get((cur, code1)) if code1 in self.last else None
                if composed is None:
                    if comb1 == 0:
                        break
                    comb = comb1
                    i1 += 1
                    continue
                cur = composed
                skipped.append(i1)
                i1 += 1
                if cur not in self.first:
                    break
            out.append(cur)
            i += 1
        return out


def corrections_of(decomp, ccc) -> dict[int, int]:
    model = Model(decomp, {}, ccc, [])
    out = {}
    for c in range(0x110000):
        if 0xD800 <= c <= 0xDFFF:
            continue
        expected = [ord(x) for x in UCD.normalize("NFD", chr(c))]
        if model.nfd([c], False, corrections=False) != expected:
            if len(expected) != 1 or [ord(x) for x in UCD.normalize("NFD", chr(expected[0]))] != expected:
                raise SystemExit(f"U+{c:04X}: NFD {expected} is not a single stable code point")
            out[c] = expected[0]
    return out


def check_model(model: Model) -> None:
    for c in range(0x110000):
        s = chr(c)
        for form, f in (("NFD", lambda x: model.nfd(x, False)), ("NFKD", lambda x: model.nfd(x, True)),
                        ("NFKC", model.nfkc)):
            if f([c]) != [ord(x) for x in UCD.normalize(form, s)]:
                raise SystemExit(f"the generated tables do not reproduce ucd_3_2_0.normalize({form!r}, U+{c:04X})")


def table(name: str, doc: str, ty: str, rows: list[str]) -> str:
    body = "".join(f"    {r},\n" for r in rows)
    return f"/// {doc}\npub static {name}: &[{ty}] = &[\n{body}];\n"


def render() -> str:
    preds = predicate_ranges()
    decomp = decompositions()
    ccc_runs = combining_runs()
    ccc = {c: v for lo, hi, v in ccc_runs for c in range(lo, hi + 1)}
    pairs = composition_pairs()
    corrections = corrections_of(decomp, ccc)
    check_model(Model(decomp, corrections, ccc, pairs))
    parts = [
        "//! Generated by `oracle/codegen/saslprep_tables.py` from CPython "
        f"{sys.version.split()[0]} (`unicodedata` {unicodedata.unidata_version}, `ucd_3_2_0` "
        f"{UCD.unidata_version}). Do not edit.\n",
    ]
    for name, rows in preds.items():
        parts.append(
            table(
                name,
                f"Inclusive code point ranges where `stringprep.in_table_{name.lower()}` is true.",
                "(u32, u32)",
                [f"(0x{lo:04x}, 0x{hi:04x})" for lo, hi in rows],
            )
        )
    parts.append(
        table(
            "DECOMPOSITION",
            "`ucd_3_2_0.decomposition(c)` (empty for code points unassigned in Unicode 3.2.0): "
            "`(c, compatibility, code points)`, sorted by `c`.",
            "(u32, bool, &[u32])",
            [f"(0x{c:04x}, {str(compat).lower()}, &[{', '.join(f'0x{x:04x}' for x in cps)}])"
             for c, (compat, cps) in sorted(decomp.items())],
        )
    )
    parts.append(
        table(
            "NORMALIZATION_CORRECTIONS",
            "`PreviousDBVersion.normalization` of UCD 3.2.0: `(c, replacement)`, sorted.",
            "(u32, u32)",
            [f"(0x{c:04x}, 0x{v:04x})" for c, v in sorted(corrections.items())],
        )
    )
    parts.append(
        table(
            "COMBINING",
            "Runs `(first, last, class)` of the canonical combining classes (`unicodedata.combining`, "
            "the current database).",
            "(u32, u32, u8)",
            [f"(0x{lo:04x}, 0x{hi:04x}, {v})" for lo, hi, v in ccc_runs],
        )
    )
    parts.append(
        table(
            "COMPOSITION",
            "Canonical composition pairs `(first, last, composed)` of the current database, sorted.",
            "(u32, u32, u32)",
            [f"(0x{a:04x}, 0x{b:04x}, 0x{c:04x})" for a, b, c in pairs],
        )
    )
    return "\n".join(parts)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--check", action="store_true", help="exit 1 if the generated file is not up to date")
    args = ap.parse_args(argv)
    if sys.implementation.name != "cpython" or sys.version_info[:2] != (3, 12):
        print("the oracle is CPython 3.12", file=sys.stderr)
        return 2
    text = render()
    if args.check:
        current = OUT.read_text(encoding="utf-8") if OUT.is_file() else ""
        if current != text:
            print(f"{OUT} is not up to date: run python oracle/codegen/saslprep_tables.py", file=sys.stderr)
            return 1
        return 0
    OUT.write_text(text, encoding="utf-8")
    print(f"wrote {OUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
