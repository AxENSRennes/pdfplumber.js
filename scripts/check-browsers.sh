#!/usr/bin/env bash
# Browser CI: builds the package as it ships (wasm-opt included), then runs
# the shared runtime tests (js/test/shared/tests.mjs) in Chromium, Firefox
# and WebKit through Playwright, with the core in the page and in a worker
# (docs/js-runtime.md, "Tests").
#
#   scripts/check-browsers.sh [--no-build] [PLAYWRIGHT ARGS...]   # e.g. --project=webkit
#
# PDFPLUMBER_ROOT (default: this checkout) gives .venv, corpus/ and .cache/.
# On a machine without WebKit's system libraries (Ubuntu without root),
# scripts/webkit-local-libs.sh fetches them under .cache/webkit-libs and the
# WebKit project runs through its launcher.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PDFPLUMBER_ROOT="${PDFPLUMBER_ROOT:-$ROOT}"
export PYTHONPATH="$ROOT${PYTHONPATH:+:$PYTHONPATH}"
export PATH="$HOME/.cargo/bin:$PATH"
PY="$PDFPLUMBER_ROOT/.venv/bin/python"
cd "$ROOT"

step() { printf '\n==> %s\n' "$*"; }

if [[ "${1:-}" == "--no-build" ]]; then
  shift
else
  step "build (wasm-opt, CMaps, JS)"
  node scripts/build.mjs
fi

step "the packed package bundled by esbuild and Vite (README recipes: dist/bundler-tests, bundlers.spec.ts)"
node scripts/check-package.mjs --bundles

step "unit oracle data of the JS runtime (python -m oracle.unit js_runtime)"
"$PY" -m oracle.unit js_runtime

if [[ -z "${PDFPLUMBER_WEBKIT:-}" ]] && command -v apt-get >/dev/null 2>&1; then
  if ! node -e "require('playwright').webkit.launch().then(b => b.close()).catch(() => process.exit(1))" 2>/dev/null; then
    step "WebKit lacks system libraries: local copies (scripts/webkit-local-libs.sh)"
    export PDFPLUMBER_WEBKIT="$(scripts/webkit-local-libs.sh)"
    export PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS=1
  fi
fi

step "Playwright: Chromium, Firefox, WebKit"
npx playwright test --config js/playwright.config.ts "$@"

printf '\ncheck-browsers.sh: all browser checks passed\n'
