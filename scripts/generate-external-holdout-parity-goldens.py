#!/usr/bin/env python3
from __future__ import annotations

import os
import pathlib
import runpy

ROOT = pathlib.Path(__file__).resolve().parents[1]

os.environ["EXTERNAL_PARITY_MANIFEST"] = "test/fixtures/external-holdout-pdfs/manifest.json"
os.environ["EXTERNAL_PARITY_OUT"] = "test/fixtures/goldens/pdfplumber-external-holdout-parity.json"

runpy.run_path(str(ROOT / "scripts" / "generate-external-parity-goldens.py"), run_name="__main__")
