"""Self-test of oracle/verify.py with a fake pdftrace (oracle/tests/fake_pdftrace.py).

    PYTHONPATH=. .venv/bin/python -m unittest oracle.tests.test_verify

Uses a temporary cache directory (``PDFPLUMBER_CACHE``) and the micro PDFs of
this checkout (``corpus/micro``), so it needs no downloaded corpus.
"""

from __future__ import annotations

import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from oracle.verify import first_difference

REPO = Path(__file__).resolve().parents[2]
MICRO = REPO / "corpus" / "micro"
FAKE = Path(__file__).resolve().parent / "fake_pdftrace.py"
FILES = [MICRO / n for n in ("text-operators.pdf", "form-xobject-matrix.pdf", "vector-objects.pdf", "encrypted-password.pdf")]


class VerifyWithFakePdftrace(unittest.TestCase):
    tmp: Path
    pdftrace: Path

    @classmethod
    def setUpClass(cls) -> None:
        cls.tmp = Path(tempfile.mkdtemp(prefix="oracle-verify-test-"))
        cls.pdftrace = cls.tmp / "pdftrace"
        cls.pdftrace.write_text(f'#!/bin/sh\nexec "{sys.executable}" "{FAKE}" "$@"\n')
        cls.pdftrace.chmod(cls.pdftrace.stat().st_mode | stat.S_IXUSR)

    @classmethod
    def tearDownClass(cls) -> None:
        shutil.rmtree(cls.tmp, ignore_errors=True)

    def verify(self, *extra: str, mode: str = "copy", target: str = "", cache: str = "cache", files: list[Path] = FILES, stage: str = "psparser", line: int = 2) -> tuple[int, dict]:
        env = dict(os.environ)
        env.update(
            PDFPLUMBER_ROOT=str(REPO),
            PDFPLUMBER_CACHE=str(self.tmp / cache),
            PYTHONPATH=str(REPO),
            FAKE_PDFTRACE_MODE=mode,
            FAKE_PDFTRACE_TARGET=target,
            FAKE_PDFTRACE_LINE=str(line),
        )
        cmd = [sys.executable, "-m", "oracle.verify", "--stage", stage, "--json", "--pdftrace", str(self.pdftrace)]
        cmd += [*extra, "--files", *map(str, files)]
        proc = subprocess.run(cmd, env=env, capture_output=True, cwd=REPO)
        try:
            summary = json.loads(proc.stdout) if proc.stdout.strip() else {}
        except ValueError:
            self.fail(f"not JSON: {proc.stdout[:500]!r}\nstderr: {proc.stderr[-2000:]!r}")
        return proc.returncode, summary

    def test_identical(self) -> None:
        code, summary = self.verify()
        self.assertEqual(code, 0, summary)
        self.assertEqual(summary["counts"]["identical"], len(FILES))
        self.assertEqual(summary["counts"]["total"], len(FILES))
        self.assertEqual(summary["files"], [])

    def test_password_from_manifest(self) -> None:
        # corpus/micro/manifest.json gives encrypted-password.pdf its password:
        # the trace must reach the page content, not stop at PDFPasswordIncorrect.
        cache = self.tmp / "cache"
        self.verify()
        traces = [p.read_text() for p in cache.glob("oracle/psparser/*/*.pw-*.txt")]
        self.assertEqual(len(traces), 1)
        self.assertIn('Kx"546a"', traces[0])  # Tj
        self.assertTrue(traces[0].endswith("!end\n"))

    def test_perturbed_line(self) -> None:
        code, summary = self.verify("--batch-size", "2", mode="perturb", target="vector-objects", files=FILES)
        self.assertEqual(code, 1)
        self.assertEqual(summary["counts"]["different"], 1)
        self.assertEqual(summary["counts"]["identical"], len(FILES) - 1)
        (rec,) = summary["files"]
        self.assertTrue(rec["path"].endswith("vector-objects.pdf"))
        d = rec["difference"]
        self.assertEqual(d["line"], 2)
        self.assertEqual(d["first_context_line"], 1)
        self.assertTrue(d["rust"][1].endswith(" !perturbed"))
        self.assertEqual(d["rust"][1], d["oracle"][1] + " !perturbed")
        self.assertEqual(d["column"], len(d["oracle"][1]))
        self.assertEqual(d["oracle"][0], d["rust"][0])

    def test_perturbed_line_context(self) -> None:
        code, summary = self.verify(mode="perturb", target="text-operators", files=FILES[:1], line=6)
        self.assertEqual(code, 1)
        d = summary["files"][0]["difference"]
        self.assertEqual(d["line"], 6)
        self.assertEqual(d["first_context_line"], 3)
        self.assertEqual(len(d["oracle"]), 7)
        self.assertEqual(d["oracle"][:3], d["rust"][:3])
        self.assertEqual(d["oracle"][4:], d["rust"][4:])

    def test_truncated(self) -> None:
        code, summary = self.verify(mode="truncate", target="form-xobject", files=FILES)
        self.assertEqual(code, 1)
        (rec,) = summary["files"]
        d = rec["difference"]
        self.assertEqual(d["oracle_lines"], d["rust_lines"] + 1)
        self.assertEqual(d["line"], d["oracle_lines"])
        self.assertEqual(d["oracle"][-1 if d["oracle"][-1] else -2], "!end")

    def test_crash_is_isolated(self) -> None:
        code, summary = self.verify("--batch-size", "4", mode="crash", target="form-xobject", files=FILES)
        self.assertEqual(code, 1)
        self.assertEqual(summary["counts"]["rust-crashed"], 1)
        self.assertEqual(summary["counts"]["identical"], len(FILES) - 1)
        (rec,) = summary["files"]
        self.assertEqual(rec["status"], "rust-crashed")
        self.assertIn("exit 101", rec["reason"])

    def test_unimplemented_stage(self) -> None:
        code, summary = self.verify(mode="unimplemented")
        self.assertEqual(code, 2)
        self.assertEqual(summary, {})

    def test_missing_binary(self) -> None:
        env_missing = self.tmp / "nope" / "pdftrace"
        code, _ = self.verify("--pdftrace", str(env_missing))
        self.assertEqual(code, 2)

    def test_oracle_timeout_is_excluded(self) -> None:
        # 1 ms cannot even start Python: every oracle trace becomes !Timeout,
        # is cached as such, and is counted apart rather than compared.
        code, summary = self.verify("--timeout", "0.001", cache="cache-timeout", files=FILES[:2])
        self.assertEqual(code, 0, summary)
        self.assertEqual(summary["counts"]["oracle-timeout"], 2)
        self.assertEqual(summary["counts"]["identical"], 0)
        cached = list((self.tmp / "cache-timeout").glob("oracle/psparser/*/*.txt"))
        self.assertEqual(sorted(p.read_text() for p in cached), ["!Timeout\n", "!Timeout\n"])
        # --retry-timeouts rebuilds them with a normal time limit.
        code, summary = self.verify("--retry-timeouts", cache="cache-timeout", files=FILES[:2])
        self.assertEqual(code, 0, summary)
        self.assertEqual(summary["counts"]["identical"], 2)

    def test_other_stages(self) -> None:
        for stage in ("objects", "streams", "pages"):
            code, summary = self.verify(stage=stage, files=FILES[:2])
            self.assertEqual(code, 0, (stage, summary))
            self.assertEqual(summary["counts"]["identical"], 2, stage)


class FirstDifference(unittest.TestCase):
    def test_middle(self) -> None:
        a = b"".join(b"l%d\n" % i for i in range(10))
        b = a.replace(b"l5\n", b"l5x\n")
        d = first_difference(a, b)
        self.assertEqual((d["line"], d["column"]), (6, 2))
        self.assertEqual(d["first_context_line"], 3)
        self.assertEqual(d["oracle"], ["l2", "l3", "l4", "l5", "l6", "l7", "l8"])
        self.assertEqual(d["rust"][3], "l5x")

    def test_rust_shorter(self) -> None:
        d = first_difference(b"a\nb\nc\n", b"a\nb\n")
        self.assertEqual(d["line"], 3)
        self.assertEqual(d["rust"], ["a", "b", ""])
        self.assertEqual(d["oracle"], ["a", "b", "c", ""])

    def test_missing_final_newline(self) -> None:
        d = first_difference(b"a\n!end\n", b"a\n!end")
        self.assertEqual(d["line"], 3)

    def test_long_line_is_clipped_around_the_difference(self) -> None:
        a = b"x" * 5000 + b"A" + b"y" * 100 + b"\n"
        b = b"x" * 5000 + b"B" + b"y" * 100 + b"\n"
        d = first_difference(a, b)
        self.assertEqual(d["column"], 5000)
        self.assertIn("A", d["oracle"][0])
        self.assertIn("B", d["rust"][0])
        self.assertLess(len(d["oracle"][0]), 400)


if __name__ == "__main__":
    unittest.main()
