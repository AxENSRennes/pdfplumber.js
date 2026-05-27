# Agent Instructions

- The goal is a robust pdfplumber port with browser-capable JavaScript parity, not just passing the current tests or committing to a specific PDF engine; when useful, read relevant upstream source code and use focused terminal experiments to understand behavior and guide the port.
- Use pdfplumber/pdfminer as the oracle for tests whenever possible; create test PDFs when needed, and do not encode expected behavior by hand unless no Python-backed validation is possible.
