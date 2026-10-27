"""Tests of the classification of oracle/api_parity.py.

    PYTHONPATH=. .venv/bin/python -m unittest oracle.tests.test_api_parity

The traces are written in the line formats of oracle/stages/plumber_doc.py
and plumber_text.py; each rule is checked on the case it accepts and on the
nearest cases it must refuse.
"""

from __future__ import annotations

import unittest

from oracle.api_parity import align_filters, classify

REC = '!RecursionError("maximum recursion depth exceeded")'
FILTERED = [
    "page 1 \"<Page:1>\"",
    'derived [["filter", "small"], ["crop", [0.0, 0.0, 1.0, 0.5], {}]]',
    'dbox (0.0, 0.0, 200.0, 100.0) 200.0 100.0 "<Page:1>"',
    f"dobjects {REC}",
    "dannots 0 4f53cda18c2baa0c",
    'derived [["crop", [0.0, 0.0, 0.5, 0.5], {}]]',
    'dbox (0.0, 0.0, 100.0, 100.0) 100.0 100.0 "<Page:1>"',
    "!end",
]
REJECTED = FILTERED[:2] + [f"derive {REC}"] + FILTERED[5:]


def b(lines: list[str]) -> bytes:
    return ("\n".join(lines) + "\n").encode()


class AlignFilters(unittest.TestCase):
    def test_a_rejected_filter_is_aligned(self) -> None:
        out = align_filters(FILTERED, REJECTED)
        self.assertIsNotNone(out)
        assert out is not None
        self.assertEqual(out, (REJECTED, 1))

    def test_two_gaps(self) -> None:
        self.assertEqual(align_filters(FILTERED + FILTERED, REJECTED + REJECTED), (REJECTED + REJECTED, 2))

    def test_another_exception_is_not_aligned(self) -> None:
        other = [x.replace("RecursionError", "ValueError") if x.startswith("derive ") else x for x in REJECTED]
        self.assertIsNone(align_filters(FILTERED, other))

    def test_a_derivation_without_filter_is_not_aligned(self) -> None:
        oracle = [x.replace('["filter", "small"], ', "") for x in FILTERED]
        api = [x.replace('["filter", "small"], ', "") for x in REJECTED]
        self.assertIsNone(align_filters(oracle, api))

    def test_objects_that_do_not_raise_are_not_aligned(self) -> None:
        oracle = [('dobjects [["char", 1, "00"]]' if x.startswith("dobjects ") else x) for x in FILTERED]
        self.assertIsNone(align_filters(oracle, REJECTED))

    def test_a_difference_after_the_gap_is_not_aligned(self) -> None:
        api = REJECTED[:-2] + ['dbox (0.0, 0.0, 100.0, 50.0) 100.0 50.0 "<Page:1>"', "!end"]
        self.assertIsNone(align_filters(FILTERED, api))


class Classify(unittest.TestCase):
    def test_identical(self) -> None:
        self.assertEqual(classify("a", b(["x"]), b(["x"]), b(["y"]), set(), "plumber_text")[0], "identical")

    def test_no_api_output_is_a_defect(self) -> None:
        self.assertEqual(classify("a", b(["x"]), None, b(["x"]), set(), "plumber_text")[0], "defect")

    def test_core_needs_the_api_to_be_the_rust_trace(self) -> None:
        self.assertEqual(classify("a", b(["x"]), b(["y"]), b(["y"]), set(), "plumber_text")[0], "core")
        self.assertEqual(classify("a", b(["x"]), b(["y"]), b(["z"]), set(), "plumber_text")[0], "defect")
        # the Rust trace is Python's: the API's difference is its own
        self.assertEqual(classify("a", b(["x"]), b(["y"]), b(["x"]), set(), "plumber_text")[0], "defect")

    def test_filter_only_for_plumber_doc(self) -> None:
        self.assertEqual(classify("a", b(FILTERED), b(REJECTED), b(FILTERED), set(), "plumber_doc")[0], "filter")
        self.assertEqual(classify("a", b(FILTERED), b(REJECTED), b(FILTERED), set(), "plumber_text")[0], "defect")

    def test_window(self) -> None:
        stem = "108-depth-figures-327"
        oracle = b(["page 1", f"chars !PdfminerException(\"m\") cause {REC}", "!end"])
        api = b(["page 1", 'op 0 ["extract_text", {}]', 'out 0 "x"', "!end"])
        self.assertEqual(classify(stem, oracle, api, oracle, {stem}, "plumber_text")[0], "window")
        # the other way round: the API meets the limit first
        self.assertEqual(classify(stem, api, oracle, api, {stem}, "plumber_text")[0], "window")
        # no session for the window
        self.assertEqual(classify(stem, oracle, api, oracle, set(), "plumber_text")[0], "defect")
        # a name that is not a window's
        self.assertEqual(classify("108-figures-327", oracle, api, oracle, {"108-figures-327"}, "plumber_text")[0], "defect")
        # a first difference that is not a RecursionError
        other = b(["page 1", 'chars !ValueError("m")', "!end"])
        self.assertEqual(classify(stem, other, api, other, {stem}, "plumber_text")[0], "defect")
        # a difference before the RecursionError
        early = b(["page 2", f"chars {REC}", "!end"])
        self.assertEqual(classify(stem, early, api, early, {stem}, "plumber_text")[0], "defect")


if __name__ == "__main__":
    unittest.main()
