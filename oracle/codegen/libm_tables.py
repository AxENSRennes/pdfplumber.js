"""Generate crates/pycompat/src/libm_tables.rs: the data of the oracle's pow().

CPython's ``float ** float`` calls the C library's ``pow``. On the oracle
(x86-64 glibc 2.39) that is the ARM optimized-routines algorithm, and
``crates/pycompat/src/libm.rs`` ports it. Its tables (``__pow_log_data``,
``__exp_data``) are read here from the vendored ARM sources
(``upstream/arm-optimized-routines/math/pow_log_data.c`` and ``exp_data.c``,
MIT, see the README there) with the configuration glibc builds them with
(``N = 128``, ``POW_LOG_POLY_ORDER = 8``, ``EXP_POLY_ORDER = 5``, no wide
polynomials, the plain ``Shift``), and written as IEEE-754 bit patterns, so
the Rust constants are exactly the C ones:

    python oracle/codegen/libm_tables.py [--check]

``--check`` exits 1 if the file is not up to date. The tables are identical
to glibc 2.39's ``e_pow_log_data.c``/``e_exp_data.c`` (checked when they
were vendored, see the README), and ``oracle/unit/pycompat.py`` compares the
port with CPython on many ``**`` cases, which a wrong constant would fail.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SRC = REPO / "upstream" / "arm-optimized-routines" / "math"
OUT = REPO / "crates" / "pycompat" / "src" / "libm_tables.rs"

#: The configuration of the tables glibc 2.39 compiles (sysdeps/ieee754/dbl-64/math_config.h).
MACROS = {
    "N": 128,
    "POW_LOG_TABLE_BITS": 7,
    "POW_LOG_POLY_ORDER": 8,
    "EXP_TABLE_BITS": 7,
    "EXP_POLY_ORDER": 5,
    "EXP_POLY_WIDE": 0,
    "EXP_USE_TOINT_NARROW": 0,
    "EXP2_POLY_ORDER": 5,
    "EXP2_POLY_WIDE": 0,
    "EXP10_POLY_WIDE": 0,
}

_HEX_FLOAT = r"[-+]?0x[0-9a-fA-F]*\.?[0-9a-fA-F]*p[-+]?\d+"


def _condition(expr: str) -> bool:
    expr = re.sub(r"defined\s*\(?\s*(\w+)\s*\)?", lambda m: "1" if m.group(1) in MACROS else "0", expr)
    expr = expr.replace("&&", " and ").replace("||", " or ")
    expr = re.sub(r"!(?!=)", " not ", expr)
    expr = re.sub(r"\b[A-Za-z_]\w*\b", lambda m: m.group(0) if m.group(0) in ("and", "or", "not") else str(MACROS.get(m.group(0), 0)), expr)
    return bool(eval(expr, {"__builtins__": {}}))  # noqa: S307 - integer expressions of the C preprocessor


def preprocess(text: str) -> str:
    """The lines a C preprocessor keeps with ``MACROS`` (``#if``/``#elif``/
    ``#else``/``#endif`` only; other directives are dropped)."""
    out: list[str] = []
    stack: list[tuple[bool, bool]] = []  # (this branch active, a branch was taken)
    text = re.sub(r"/\*.*?\*/", " ", text, flags=re.S)
    for line in text.splitlines():
        stripped = line.strip()
        parent = all(active for active, _ in stack)
        if stripped.startswith("#if"):
            if stripped.startswith("#ifdef"):
                value = stripped.split()[1] in MACROS
            elif stripped.startswith("#ifndef"):
                value = stripped.split()[1] not in MACROS
            else:
                value = _condition(stripped[3:])
            stack.append((value, value))
        elif stripped.startswith("#elif"):
            _, taken = stack.pop()
            value = not taken and _condition(stripped[5:])
            stack.append((value, taken or value))
        elif stripped.startswith("#else"):
            _, taken = stack.pop()
            stack.append((not taken, True))
        elif stripped.startswith("#endif"):
            stack.pop()
        elif stripped.startswith("#"):
            continue
        elif parent:
            out.append(re.sub(r"//.*", "", line))
    return "\n".join(out)


def _value(expr: str) -> float:
    """A constant expression of the tables: a hex float, times or divided by
    an integer or ``N`` (power-of-two scalings, exact)."""
    expr = expr.strip()
    m = re.fullmatch(rf"({_HEX_FLOAT})(?:\s*([*/])\s*([-+]?\w+))?", expr)
    if not m:
        raise ValueError(f"unsupported constant {expr!r}")
    value = float.fromhex(m.group(1))
    if m.group(2):
        operand = MACROS["N"] if m.group(3) == "N" else int(m.group(3))
        value = value * operand if m.group(2) == "*" else value / operand
    return value


def _field(text: str, name: str) -> str:
    m = re.search(rf"\.{name}\s*=\s*", text)
    if not m:
        raise ValueError(f"no field .{name}")
    rest = text[m.end() :]
    if rest.startswith("{"):
        depth = 0
        for i, c in enumerate(rest):
            depth += {"{": 1, "}": -1}.get(c, 0)
            if depth == 0:
                return rest[1:i]
    return rest.split(",", 1)[0]


def _list(body: str) -> list[str]:
    return [x.strip() for x in body.replace("\n", " ").split(",") if x.strip()]


def pow_log_data(path: Path) -> dict[str, object]:
    text = preprocess(path.read_text(encoding="utf-8"))
    tab = []
    for m in re.finditer(r"\bA\(([^)]*)\)", _field(text, "tab")):
        invc, logc, logctail = (_value(x) for x in m.group(1).split(","))
        tab.append((invc, logc, logctail))
    return {
        "ln2hi": _value(_field(text, "ln2hi")),
        "ln2lo": _value(_field(text, "ln2lo")),
        "poly": [_value(x) for x in _list(_field(text, "poly"))],
        "tab": tab,
    }


def exp_data(path: Path) -> dict[str, object]:
    text = preprocess(path.read_text(encoding="utf-8"))
    return {
        "invln2N": _value(_field(text, "invln2N")),
        "shift": _value(_field(text, "shift")),
        "negln2hiN": _value(_field(text, "negln2hiN")),
        "negln2loN": _value(_field(text, "negln2loN")),
        "poly": [_value(x) for x in _list(_field(text, "poly"))],
        "tab": [int(x, 16) for x in _list(_field(text, "tab"))],
    }


def bits(x: float) -> str:
    import struct

    return f"0x{struct.unpack('>Q', struct.pack('>d', x))[0]:016x}"


def render(log: dict[str, object], exp: dict[str, object]) -> str:
    assert len(log["poly"]) == MACROS["POW_LOG_POLY_ORDER"] - 1  # type: ignore[arg-type]
    assert len(log["tab"]) == 1 << MACROS["POW_LOG_TABLE_BITS"]  # type: ignore[arg-type]
    assert len(exp["poly"]) == MACROS["EXP_POLY_ORDER"] - 1  # type: ignore[arg-type]
    assert len(exp["tab"]) == 2 << MACROS["EXP_TABLE_BITS"]  # type: ignore[arg-type]
    lines = [
        "// Generated by oracle/codegen/libm_tables.py from",
        "// upstream/arm-optimized-routines/math/{pow_log_data.c,exp_data.c}; do not edit.",
        "// Values are IEEE-754 bit patterns (see crates/pycompat/src/libm.rs).",
        "",
        "/// `__pow_log_data.ln2hi`",
        f"pub(crate) const POW_LN2HI: u64 = {bits(log['ln2hi'])};",  # type: ignore[arg-type]
        "/// `__pow_log_data.ln2lo`",
        f"pub(crate) const POW_LN2LO: u64 = {bits(log['ln2lo'])};",  # type: ignore[arg-type]
        "/// `__pow_log_data.poly` (`A[0]` to `A[6]`, scaled as in the C source)",
        "pub(crate) const POW_POLY: [u64; 7] = [",
        *(f"    {bits(v)}," for v in log["poly"]),  # type: ignore[union-attr]
        "];",
        "/// `__pow_log_data.tab`: `(invc, logc, logctail)` (the C struct's `pad` is left out)",
        "pub(crate) const POW_TAB: [(u64, u64, u64); 128] = [",
        *(f"    ({bits(a)}, {bits(b)}, {bits(c)})," for a, b, c in log["tab"]),  # type: ignore[union-attr]
        "];",
        "/// `__exp_data.invln2N`",
        f"pub(crate) const EXP_INVLN2N: u64 = {bits(exp['invln2N'])};",  # type: ignore[arg-type]
        "/// `__exp_data.shift`",
        f"pub(crate) const EXP_SHIFT: u64 = {bits(exp['shift'])};",  # type: ignore[arg-type]
        "/// `__exp_data.negln2hiN`",
        f"pub(crate) const EXP_NEGLN2HIN: u64 = {bits(exp['negln2hiN'])};",  # type: ignore[arg-type]
        "/// `__exp_data.negln2loN`",
        f"pub(crate) const EXP_NEGLN2LON: u64 = {bits(exp['negln2loN'])};",  # type: ignore[arg-type]
        "/// `__exp_data.poly` (`C2` to `C5`)",
        "pub(crate) const EXP_POLY: [u64; 4] = [",
        *(f"    {bits(v)}," for v in exp["poly"]),  # type: ignore[union-attr]
        "];",
        "/// `__exp_data.tab`: `tail, sbits` pairs, as in C",
        "pub(crate) const EXP_TAB: [u64; 256] = [",
        *(f"    0x{v:016x}," for v in exp["tab"]),  # type: ignore[union-attr]
        "];",
    ]
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--check", action="store_true", help="exit 1 if the generated file is not up to date")
    args = ap.parse_args(argv)
    text = render(pow_log_data(SRC / "pow_log_data.c"), exp_data(SRC / "exp_data.c"))
    if args.check:
        current = OUT.read_text(encoding="utf-8") if OUT.is_file() else ""
        if current != text:
            print(f"{OUT} is not up to date: run python oracle/codegen/libm_tables.py", file=sys.stderr)
            return 1
        return 0
    OUT.write_text(text, encoding="utf-8")
    print(f"wrote {OUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
