"""Self-test of the unit data layout (oracle/unit/_common.py, oracle/unit/__main__.py).

    PYTHONPATH=. .venv/bin/python -m unittest oracle.tests.test_unit_common

Generators with different oracle sources (two worktrees sharing one cache)
write to different fingerprint directories, and the staleness check finds
the directory of this checkout among them.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

from oracle.environment import environment
from oracle.unit import __main__ as unit_main
from oracle.unit import _common

REPO = Path(__file__).resolve().parents[2]


class UnitLayout(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix="oracle-unit-test-"))
        patcher = mock.patch.dict(os.environ, {"PDFPLUMBER_CACHE": str(self.tmp)})
        patcher.start()
        self.addCleanup(patcher.stop)
        self.addCleanup(shutil.rmtree, self.tmp, True)

    def test_sources_choose_the_directory(self) -> None:
        with mock.patch.dict(_common._snapshot, {"oracle/unit/canon.py": "a" * 64}, clear=True):
            a = _common.unit_dir("canon")
        with mock.patch.dict(_common._snapshot, {"oracle/unit/canon.py": "b" * 64}, clear=True):
            b = _common.unit_dir("canon")
        self.assertNotEqual(a, b)
        self.assertEqual(a.parent, b.parent)
        self.assertEqual(a.parent, self.tmp / "unit" / "canon")

    def test_unit_dir_needs_the_snapshot(self) -> None:
        with mock.patch.dict(_common._snapshot, {}, clear=True), self.assertRaises(RuntimeError):
            _common.unit_dir("canon")

    def test_stale_reason_finds_the_current_directory(self) -> None:
        sha = hashlib.sha256((REPO / "oracle" / "unit" / "canon.py").read_bytes()).hexdigest()
        base = self.tmp / "unit" / "canon"
        current, other = base / "current", base / "other"
        for d, sources in ((current, {"oracle/unit/canon.py": sha}), (other, {"oracle/unit/canon.py": "0" * 64})):
            d.mkdir(parents=True)
            (d / "meta.json").write_text(json.dumps({"sources": sources, "environment": environment()}))
            time.sleep(0.02)  # `other` is the most recent one
        self.assertIsNone(unit_main.stale_reason("canon"))
        # the current directory is now the most recent: the one the Rust side opens
        newest = max(base.iterdir(), key=lambda d: (d / "meta.json").stat().st_mtime_ns)
        self.assertEqual(newest, current)
        # another environment makes it stale
        (current / "meta.json").write_text(json.dumps({"sources": {"oracle/unit/canon.py": sha}, "environment": {}}))
        self.assertIn("environment", unit_main.stale_reason("canon") or "")


if __name__ == "__main__":
    unittest.main()
