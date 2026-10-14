"""Tests of oracle/canon.py, oracle/typed_json.py and the stage runner.

    PYTHONPATH=. .venv/bin/python -m unittest oracle.tests.test_canon

The expected texts below are the examples of the canon table in
docs/verification.md; everything else is checked against CPython itself
(``repr``, ``json.dumps``, ``str``).
"""

from __future__ import annotations

import hashlib
import json
import random
import struct
import sys
import threading
import unittest
from pathlib import Path

from pdfminer.pdftypes import PDFObjRef, PDFStream
from pdfminer.psparser import KWD, LIT, PSEOF

from oracle import stages
from oracle.canon import CanonError, ExcValue, Named, canon, int_to_decimal
from oracle.typed_json import dumps, loads

REPO = Path(__file__).resolve().parents[2]


class CanonTable(unittest.TestCase):
    def test_documented_examples(self) -> None:
        stream = PDFStream({"Length": 300}, b"z" * 300)
        stream.set_objid(5, 0)
        cases = [
            (None, "None"),
            (True, "True"),
            (-12, "-12"),
            (1.0, "1.0"),
            (0.1, "0.1"),
            (1e-05, "1e-05"),
            (1e16, "1e+16"),
            (-0.0, "-0.0"),
            (float("inf"), "inf"),
            (float("nan"), "nan"),
            ("café", '"caf\\u00e9"'),
            (b"\r\n", 'x"0d0a"'),
            ((1.0, 2), "(1.0, 2)"),
            ((), "()"),
            ([], "[]"),
            ({"Type": LIT("Page")}, '{"Type": /"Page"}'),
            (LIT("F1"), '/"F1"'),
            (LIT(b"\xff"), '/x"ff"'),
            (KWD(b"BT"), 'Kx"4254"'),
            (PDFObjRef(None, 12), "R12"),
            (stream, 'Stream(5, 0, {"Length": 300}, x#300:' + hashlib.sha256(b"z" * 300).hexdigest()[:16] + ")"),
            (PSEOF("Unexpected EOF"), '!PSEOF("Unexpected EOF")'),
            (Named("LTChar", {"x0": 1.0}), 'LTChar{"x0": 1.0}'),
        ]
        for value, text in cases:
            with self.subTest(value=value):
                self.assertEqual(canon(value), text)

    def test_bytes_boundary(self) -> None:
        b256, b257 = bytes(range(256)), bytes(range(256)) + b"\x00"
        self.assertEqual(canon(b256), 'x"' + b256.hex() + '"')
        self.assertEqual(canon(b257), "x#257:" + hashlib.sha256(b257).hexdigest()[:16])

    def test_floats_are_repr(self) -> None:
        rng = random.Random(7)
        for _ in range(20000):
            x = struct.unpack(">d", rng.getrandbits(64).to_bytes(8, "big"))[0]
            self.assertEqual(canon(x), repr(x))

    def test_strings_are_json_ascii(self) -> None:
        for s in ["", "\x00\x1f\x7f", "\u2028", "\U0001f600", "\ud800", "\udc00\ud800", '"\\/']:
            self.assertEqual(canon(s), json.dumps(s, ensure_ascii=True))

    def test_huge_ints(self) -> None:
        self.assertGreater(len(str(10**4299)), 4299)  # still allowed by str()
        n = 10**5000 + 123
        text = int_to_decimal(n)
        self.assertEqual(len(text), 5001)
        self.assertTrue(text.startswith("1000") and text.endswith("0123"))
        self.assertEqual(int_to_decimal(-n), "-" + text)
        self.assertEqual(sys.get_int_max_str_digits(), 4300)  # never changed globally

    def test_single_element_tuple(self) -> None:
        self.assertEqual(canon((1,)), "(1)")

    def test_bool_before_int(self) -> None:
        self.assertEqual(canon([True, 1, False, 0]), "[True, 1, False, 0]")

    def test_decoded_stream_has_no_raw(self) -> None:
        s = PDFStream({}, b"abc")
        s.get_data()
        self.assertEqual(canon(s), "Stream(None, None, {}, None)")

    def test_exception_names_and_addresses(self) -> None:
        import zlib

        try:
            zlib.decompress(b"nope")
        except zlib.error as e:
            self.assertTrue(canon(e).startswith('!zlib.error("Error -3'))
        e = ValueError(f"bad {object()!r}")
        self.assertRegex(canon(e), r'^!ValueError\("bad <object object at 0x\?>"\)$')
        self.assertEqual(canon(KeyError("Length")), "!KeyError(\"'Length'\")")
        self.assertEqual(canon(ExcValue("PSEOF", "x")), canon(PSEOF("x")))

    def test_deep_nesting_is_not_recursive(self) -> None:
        v: object = 0
        for _ in range(100000):
            v = [v]
        self.assertEqual(canon(v), "[" * 100000 + "0" + "]" * 100000)

    def test_cycles_and_unknown_types_are_harness_errors(self) -> None:
        a: list[object] = []
        a.append(a)
        with self.assertRaises(CanonError):
            canon(a)
        with self.assertRaises(CanonError):
            canon(bytearray(b"x"))
        self.assertFalse(issubclass(CanonError, Exception))
        shared = [1]
        self.assertEqual(canon([shared, shared]), "[[1], [1]]")  # shared, not cyclic


class TypedJson(unittest.TestCase):
    def test_round_trip(self) -> None:
        stream = PDFStream({"Filter": LIT("FlateDecode")}, b"raw")
        stream.set_objid(3, 1)
        values = [
            None, True, 0, -(10**5000), 1.5, -0.0, float("nan"), "a\ud800", "\U0001f600", b"\x00\xff",
            [1, (2, [3])], {"k": {1: (None,)}}, LIT("x"), LIT(b"\xff"), KWD(b"Tj"), PDFObjRef(None, 2**70),
            stream, ValueError("m"), Named("Cell", {"bbox": (0.0, 1.0, 2.0, 3.0)}),
        ]  # fmt: skip
        for v in values:
            with self.subTest(v=v):
                text = dumps(v)
                text.encode("ascii")
                again = loads(text)
                self.assertEqual(canon(again), canon(v))
                self.assertEqual(dumps(again), text)

    def test_int_float_and_tuple_list_stay_distinct(self) -> None:
        self.assertEqual(dumps([1, 1.0, (1,)]), '[{"i":"1"},{"f":"3ff0000000000000"},{"t":[{"i":"1"}]}]')

    def test_surrogates_use_code_points(self) -> None:
        self.assertEqual(dumps("a\udc00"), '{"s":[97,56320]}')
        self.assertEqual(dumps("\U0001f600"), '"\\ud83d\\ude00"')


class StageRunner(unittest.TestCase):
    def test_stage_entry_depth(self) -> None:
        depths: list[int] = []
        target = str(REPO / "oracle" / "stages" / "objects.py")

        def profile(frame, event, arg):  # type: ignore[no-untyped-def]
            if event == "call" and frame.f_code.co_filename == target and frame.f_code.co_name == "run":
                n, f = 0, frame.f_back
                while f is not None:
                    n, f = n + 1, f.f_back
                depths.append(n)

        threading.setprofile(profile)
        try:
            stages.trace_lines("objects", REPO / "corpus" / "micro" / "vector-objects.pdf")
        finally:
            threading.setprofile(None)  # type: ignore[arg-type]
        self.assertEqual(depths[0], stages.STAGE_ENTRY_DEPTH)

    def test_traces_end_with_end_or_exception(self) -> None:
        for name in stages.available_stages():
            lines = stages.trace_lines(name, REPO / "corpus" / "micro" / "broken-recovery.pdf")
            self.assertEqual(lines[-1], "!end", name)
            lines = stages.trace_lines(name, REPO / "corpus" / "micro" / "encrypted-password.pdf")  # no password
            self.assertEqual(lines, ['!PDFPasswordIncorrect("")'], name)


if __name__ == "__main__":
    unittest.main()
