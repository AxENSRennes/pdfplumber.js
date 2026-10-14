# Agent Instructions

- The goal is a robust pdfplumber port with browser-capable JavaScript parity: a Rust core, ported module by module from pdfminer.six and pdfplumber, compiled to Wasm behind a coarse-grained JS API. Passing the current tests is not the goal; behaving like Python pdfplumber is.
- Read these before working on the port:
  - `docs/port-strategy.md`: why the old pdf.js-based port was dropped, the Rust/Wasm decision, and the Python numeric semantics to emulate.
  - `docs/architecture.md`: repository layout, porting rules (one Rust module per Python module, upstream `file:line` references, no special cases), value model, error model, JS boundary.
  - `docs/verification.md`: canonical trace format, stages, tools (`oracle/verify.py`, `pdftrace`), corpus sets, adversarial corpora, mutation testing, and the definition of done for a module.
- Python pdfplumber/pdfminer (`.venv`, Python 3.12, sources in `upstream/`) is the oracle. Never encode expected behavior by hand when Python can produce it; create test PDFs when needed.
- A difference with Python is fixed at its cause, found by following the stage traces. Never add a font name, file fingerprint or unexplained threshold to make a case pass.
- The previous pdf.js-based port is tagged `legacy-pdfjs`; read it only as a source of edge-case ideas, never as a reference for behavior.
