# Agent Instructions

- The goal is a robust pdfplumber port with browser-capable JavaScript parity, not just passing the current tests or committing to a specific PDF engine; when useful, read relevant upstream source code and use focused terminal experiments to understand behavior and guide the port.
- Use pdfplumber/pdfminer as the oracle for tests whenever possible; create test PDFs when needed, and do not encode expected behavior by hand unless no Python-backed validation is possible.
- Read `docs/port-strategy.md` before working on the port: it records the diagnosis of the current `src/`, the Rust/Wasm core decision, the mandatory verification method (per-stage Python traces, generated adversarial corpora, mutation tests, no special cases), and the Python numeric semantics to emulate.
