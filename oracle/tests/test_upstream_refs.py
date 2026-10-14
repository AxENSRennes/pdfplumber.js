"""Self-test of oracle/upstream_refs.py (the upstream ``file:line`` checker).

    PYTHONPATH=. .venv/bin/python -m unittest oracle.tests.test_upstream_refs

Reads the upstream submodules under ``PDFPLUMBER_ROOT`` (the main checkout).
"""

from __future__ import annotations

import unittest

from oracle.corpus import corpus_root
from oracle.upstream_refs import check_text, default_paths

ROOT = corpus_root()


def problems(text: str) -> list[str]:
    return [p.message for p in check_text(ROOT, text, "test")]


class UpstreamRefs(unittest.TestCase):
    def test_correct_references(self) -> None:
        self.assertEqual(problems("// pdfminer/pdfinterp.py:276 PDFContentParser.seek"), [])
        self.assertEqual(problems("/// pdfminer/psparser.py:164 `PSBaseParser.BUFSIZ`"), [])
        self.assertEqual(problems("// pdfminer/utils.py:402 choplist"), [])
        self.assertEqual(problems("// pdfminer/psparser.py:166 PSBaseParser.__init__ + :523 PSStackParser.__init__"), [])
        self.assertEqual(problems("/// pdfminer/psparser.py:161 `PSBaseParser` and :522 `PSStackParser`, with the"), [])
        self.assertEqual(problems("// pdfminer/pdfinterp.py:327-329 PDFContentParser.KEYWORD_BI, PDFContentParser.KEYWORD_EI"), [])
        self.assertEqual(problems("// pdfplumber/page.py:191 Page.__init__"), [])

    def test_wrong_references(self) -> None:
        # the references fixed in pdfinterp.rs (they were 2 to 6 lines off)
        self.assertEqual(problems("// pdfminer/pdfinterp.py:278 PDFContentParser.seek"), ["PDFContentParser.seek is at line 276, not 278"])
        self.assertEqual(len(problems("// pdfminer/pdfinterp.py:330-332 PDFContentParser.KEYWORD_BI")), 1)
        # a method named without its class, an unknown name, a missing name
        self.assertEqual(len(problems("// pdfminer/pdfinterp.py:276 seek")), 1)
        self.assertIn("no definition of nope", problems("// pdfminer/psparser.py:161 nope")[0])
        self.assertIn("no name", problems("// pdfminer/pdftypes.py:28-39")[0])
        # outside the file, a file that does not exist, a wrong continuation
        self.assertIn("outside", problems("// pdfminer/utils.py:99999 choplist")[0])
        self.assertIn("no file", problems("// pdfminer/nope.py:1 x")[0])
        self.assertEqual(len(problems("// pdfminer/psparser.py:166 PSBaseParser.__init__ + :524 PSStackParser.__init__")), 1)

    def test_sources_of_the_port(self) -> None:
        found = [str(p) for path in default_paths() for p in check_text(ROOT, path.read_text(encoding="utf-8"), str(path))]
        self.assertEqual(found, [])


if __name__ == "__main__":
    unittest.main()
