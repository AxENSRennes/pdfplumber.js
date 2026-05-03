# Agent Instructions

- Use `wsl_venv` for Python work in this repository.
- Do not create or rely on a repo-local `.venv`; `npm run python:setup` is configured to create and update `wsl_venv`.
- When running Python reference scripts directly, prefer `wsl_venv/bin/python`, for example:

```bash
wsl_venv/bin/python scripts/generate-goldens.py
```

