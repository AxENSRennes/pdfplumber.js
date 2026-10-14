"""Differential cases for the file-based parsers of pdfminer/psparser.py.

PDFContentParser is covered by oracle/unit/psparser.py. This generator drives
``PSBaseParser`` and ``PSStackParser`` over a ``BytesIO``, the way
pdfminer's PDFParser and CMapParser use them: ``nexttoken`` to the end,
``nextline`` to the end, ``revreadlines``, ``seek`` to arbitrary positions,
and ``nextobject`` (which never returns for a bare ``PSStackParser``, whose
``flush`` does nothing: the objects pile up on the stack) followed by
``pop``, ``popall`` and ``add_results`` as subclasses use them. Data are
generated around the 4096-byte read blocks and line endings.

Writes ``<cache>/unit/psparser_base/cases.jsonl`` (typed JSON lines
``{"in": [data, ops], "out": [results...]}``, see ``oracle/unit/_common.py``)
and ``meta.json``. The
Rust replay is crates/pdftrace/tests/psparser_base_unit.rs.

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.psparser_base
"""

import io
import random

from pdfminer.psparser import PSBaseParser, PSStackParser

from ._common import require_oracle_python, write_cases, write_meta


FRAGMENTS = [
    b"1 0 obj", b"<< /Type /Page /Kids [3 0 R] >>", b"endobj", b"stream", b"endstream", b"xref", b"0 5",
    b"0000000000 65535 f", b"trailer", b"startxref", b"%%EOF", b"%PDF-1.7", b"(a\\\r\nb)", b"(x\\101)", b"<4142>",
    b"/N#41", b"[1 2.5 -.5 true]", b"{ 1 }", b"-", b".", b"%comment", b"\x00", b"\xff", b"abc", b"(unclosed",
    b"<</A (b)>>", b"<</A>>", b"]", b">>", b"}",
]
EOLS = [b"\n", b"\r", b"\r\n", b" ", b"", b"\n\r", b"\r\r\n"]


def gen_data(rng):
    target = rng.choice([0, 1, 50, 4095, 4096, 4097, 8191, 8192, 9000])
    parts, size = [], 0
    while size < target:
        if rng.random() < 0.05:
            gap = 4096 - size % 4096 - rng.randint(0, 3)
            frag = b"%" + b"x" * max(gap - 2, 0) + b"\n" if gap > 2 else b" "
        else:
            frag = rng.choice(FRAGMENTS) + rng.choice(EOLS)
        parts.append(frag)
        size += len(frag)
    data = b"".join(parts)[: target + rng.randint(0, 3)] if target else b""
    return data


def run(data, ops):
    """Applies ops to a fresh parser; each op returns a result."""
    fp = io.BytesIO(data)
    parser = PSStackParser(fp)
    out = []
    for op in ops:
        kind = op[0]
        try:
            if kind == "seek":
                parser.seek(op[1])
                out.append(None)
            elif kind == "nexttoken":
                out.append(parser.nexttoken())
            elif kind == "tokens":
                toks = []
                try:
                    while True:
                        toks.append(parser.nexttoken())
                except Exception as e:  # the exception ends the list
                    toks.append(e)
                out.append(toks)
            elif kind == "nextline":
                out.append(parser.nextline())
            elif kind == "lines":
                lines = []
                try:
                    while True:
                        lines.append(parser.nextline())
                except Exception as e:  # the exception ends the list
                    lines.append(e)
                out.append(lines)
            elif kind == "revreadlines":
                out.append(list(parser.revreadlines()))
            elif kind == "nextobject":
                out.append(parser.nextobject())
            elif kind == "pop":
                out.append(parser.pop(op[1]))
            elif kind == "popall":
                out.append(parser.popall())
            elif kind == "add_results":
                # what PDFContentParser.flush and PDFParser.do_keyword do
                parser.add_results(*parser.pop(op[1]))
                out.append(list(parser.results))
            elif kind == "stack":
                out.append([parser.curtype, list(parser.curstack), len(parser.context)])
            else:
                raise ValueError(kind)
        except Exception as e:  # the exception is the result
            out.append(e)
    return out


def gen_ops(rng, n):
    ops = []
    for _ in range(rng.randint(1, 6)):
        r = rng.random()
        if r < 0.15:
            ops.append(["seek", rng.choice([0, rng.randint(0, n + 2), max(n - 5, 0), 4095, 4096, 4097])])
        elif r < 0.3:
            ops.append(["tokens"])
        elif r < 0.45:
            ops.append(["lines"])
        elif r < 0.55:
            ops.append(["revreadlines"])
        elif r < 0.65:
            ops.append(["nextline"])
        elif r < 0.75:
            ops.append(["nexttoken"])
        elif r < 0.85:
            ops.append(["nextobject"])
            ops.append(["stack"])
        elif r < 0.9:
            ops.append(["pop", rng.choice([0, 1, 2, 5])])
        elif r < 0.95:
            ops.append(["popall"])
        else:
            ops.append(["add_results", rng.choice([0, 1, 3])])
    return ops


def main():
    require_oracle_python()
    assert PSBaseParser.BUFSIZ == 4096
    rng = random.Random("20261014:psparser_base")

    def cases():
        for _ in range(3000):
            data = gen_data(rng)
            ops = gen_ops(rng, len(data))
            yield [data, ops], run(data, ops)

    n = write_cases("psparser_base", "cases", cases())
    write_meta("psparser_base", cases=n)
    print(f"wrote {n} cases")

if __name__ == "__main__":
    main()
