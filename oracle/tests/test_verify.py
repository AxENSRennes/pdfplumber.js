"""Self-test of oracle/verify.py with a fake pdftrace (oracle/tests/fake_pdftrace.py).

    PYTHONPATH=. .venv/bin/python -m unittest oracle.tests.test_verify

Uses a temporary cache directory (``PDFPLUMBER_CACHE``) and the micro PDFs of
this checkout (``corpus/micro``), so it needs no downloaded corpus.
"""

from __future__ import annotations

import hashlib
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

    def verify(self, *extra: str, mode: str = "copy", target: str = "", cache: str = "cache", files: list[Path] | None = FILES, stage: str = "psparser", line: int = 2, sleep: float = 30) -> tuple[int, dict]:
        env = dict(os.environ)
        env.update(
            PDFPLUMBER_ROOT=str(REPO),
            PDFPLUMBER_CACHE=str(self.tmp / cache),
            PYTHONPATH=str(REPO),
            FAKE_PDFTRACE_MODE=mode,
            FAKE_PDFTRACE_TARGET=target,
            FAKE_PDFTRACE_LINE=str(line),
            FAKE_PDFTRACE_SLEEP=str(sleep),
        )
        cmd = [sys.executable, "-m", "oracle.verify", "--stage", stage, "--json", "--pdftrace", str(self.pdftrace)]
        cmd += [*extra, *(["--files", *map(str, files)] if files is not None else [])]
        proc = subprocess.run(cmd, env=env, capture_output=True, cwd=REPO)
        self.stderr = proc.stderr.decode("utf-8", "replace")
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

    def test_unlisted_stage_stops_before_the_oracle(self) -> None:
        # pdftrace --list-stages says "not implemented": verify stops before
        # building any oracle trace, with a clear message.
        code, summary = self.verify(mode="unlisted", cache="unlisted")
        self.assertEqual(code, 2)
        self.assertEqual(summary, {})
        self.assertIn("does not implement stage 'psparser'", self.stderr)
        self.assertFalse((self.tmp / "unlisted" / "oracle").exists())

    def test_missing_binary(self) -> None:
        env_missing = self.tmp / "nope" / "pdftrace"
        code, _ = self.verify("--pdftrace", str(env_missing))
        self.assertEqual(code, 2)

    def test_oracle_timeout_fails_unless_allowed(self) -> None:
        # 1 ms cannot even start Python: every oracle trace becomes a timeout,
        # cached with its limit, and never counts as passing.
        code, summary = self.verify("--timeout", "0.001", cache="cache-timeout", files=FILES[:2])
        self.assertEqual(code, 1, summary)
        self.assertEqual(summary["counts"]["oracle-timeout"], 2)
        self.assertEqual(summary["counts"]["identical"], 0)
        self.assertIn("no file was compared", summary["problems"])
        cached = list((self.tmp / "cache-timeout").glob("oracle/psparser/*/*.txt"))
        self.assertEqual(sorted(p.read_text() for p in cached), ["!Timeout 0.001\n", "!Timeout 0.001\n"])
        # --allow-timeouts lists them without failing, but nothing was compared
        code, summary = self.verify("--timeout", "0.001", "--allow-timeouts", cache="cache-timeout", files=FILES[:2])
        self.assertEqual(code, 1, summary)
        self.assertEqual(summary["problems"], ["no file was compared"])
        # a later run with a larger limit rebuilds them by itself
        code, summary = self.verify(cache="cache-timeout", files=FILES[:2])
        self.assertEqual(code, 0, summary)
        self.assertEqual(summary["counts"]["identical"], 2)

    def test_old_timeout_entries_are_rebuilt(self) -> None:
        # a "!Timeout" without its limit (older caches) is rebuilt too
        self.verify(cache="cache-old-timeout", files=FILES[:1])
        (cached,) = (self.tmp / "cache-old-timeout").glob("oracle/psparser/*/*.txt")
        cached.write_text("!Timeout\n")
        code, summary = self.verify(cache="cache-old-timeout", files=FILES[:1])
        self.assertEqual(code, 0, summary)
        self.assertEqual(summary["counts"]["identical"], 1)

    def test_retry_timeouts(self) -> None:
        self.verify("--timeout", "0.001", cache="cache-retry", files=FILES[:1])
        code, summary = self.verify("--retry-timeouts", "--timeout", "60", cache="cache-retry", files=FILES[:1])
        self.assertEqual(code, 0, summary)
        self.assertEqual(summary["counts"]["identical"], 1)

    def test_nothing_to_compare_fails(self) -> None:
        code, summary = self.verify("--limit", "0")
        self.assertEqual(code, 1, summary)
        self.assertEqual(summary["counts"]["total"], 0)
        self.assertEqual(summary["problems"], ["no file was compared"])

    def test_unknown_or_missing_sets_are_errors(self) -> None:
        for name in ("adv-nosuchstage", "adv-psparser", "nosuchset"):
            code, _ = self.verify("--set", name, files=None, cache="cache-sets")
            self.assertEqual(code, 2, name)
            self.assertIn(name, self.stderr)

    def test_rust_hang_is_a_timeout(self) -> None:
        # the hanging file is killed after --timeout without completing; the
        # files the batch had finished keep their traces
        code, summary = self.verify("--timeout", "3", mode="hang", target="vector-objects", files=FILES[:3])
        self.assertEqual(code, 1, summary)
        self.assertEqual(summary["counts"]["rust-timeout"], 1)
        self.assertEqual(summary["counts"]["identical"], 2)
        (rec,) = summary["files"]
        self.assertTrue(rec["path"].endswith("vector-objects.pdf"))

    def test_rust_slow_file_is_a_timeout(self) -> None:
        # a right trace that took longer than the limit does not pass
        code, summary = self.verify("--timeout", "60", mode="slow", target="text-operators", sleep=61, files=FILES[:2])
        self.assertEqual(code, 1, summary)
        self.assertEqual(summary["counts"]["rust-timeout"], 1)
        self.assertIn("61.0 s", summary["files"][0]["reason"])

    def test_password_starting_with_a_dash(self) -> None:
        # passwords reach both CLIs as one --password=P argument
        cache = self.tmp / "cache-dash"
        manifest = cache / "corpus" / "dash-manifest.json"
        manifest.parent.mkdir(parents=True, exist_ok=True)
        sha = hashlib.sha256(FILES[0].read_bytes()).hexdigest()
        manifest.write_text(json.dumps([{"sha256": sha, "password": "-x"}]))
        code, summary = self.verify(cache="cache-dash", files=FILES[:1])
        self.assertEqual(code, 0, summary)
        self.assertEqual(summary["counts"]["identical"], 1)
        self.assertEqual(len(list(cache.glob("oracle/psparser/*/*.pw-*.txt"))), 1)

    def test_other_stages(self) -> None:
        for stage in ("objects", "streams", "pages", "fonts", "interp"):
            code, summary = self.verify(stage=stage, files=FILES[:2])
            self.assertEqual(code, 0, (stage, summary))
            self.assertEqual(summary["counts"]["identical"], 2, stage)

    def test_stage_reading_the_oracle(self) -> None:
        # the layout stage gets --oracle-dir: the fake reads each oracle
        # trace there (a password-protected file's name included)
        code, summary = self.verify(stage="layout")
        self.assertEqual(code, 0, (summary, self.stderr))
        self.assertEqual(summary["counts"]["identical"], len(FILES))


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
