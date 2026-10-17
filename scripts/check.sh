#!/usr/bin/env bash
# Local CI: formatting, lints, upstream references, generated tables, Rust
# tests on the unit oracle data (natively and compiled to WebAssembly, run by
# Node's WASI), the JavaScript package (Wasm build, Node tests, size budget;
# the browsers are scripts/check-browsers.sh) and the oracle self-tests. Regenerates the unit oracle data first when it is
# missing or stale (`python -m oracle.unit`).
#
#   scripts/check.sh            # from any directory
#
# PDFPLUMBER_ROOT (default: this checkout) is where .venv, corpus/ and .cache/
# are; in a git worktree, point it at the main checkout. Heavy steps run under
# a 5 GB memory cap when systemd-run is available (docs/verification.md).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PDFPLUMBER_ROOT="${PDFPLUMBER_ROOT:-$ROOT}"
export PYTHONPATH="$ROOT${PYTHONPATH:+:$PYTHONPATH}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export PATH="$HOME/.cargo/bin:$PATH"
PY="$PDFPLUMBER_ROOT/.venv/bin/python"
cd "$ROOT"

if [[ ! -x "$PY" ]]; then
  echo "check.sh: no oracle interpreter at $PY (run scripts/setup-dev.sh, or set PDFPLUMBER_ROOT)" >&2
  exit 2
fi

capped() {
  if command -v systemd-run >/dev/null 2>&1 && systemd-run --user --scope --quiet true >/dev/null 2>&1; then
    systemd-run --user --scope --quiet -p MemoryMax=5G -p MemorySwapMax=0 -p OOMPolicy=continue "$@"
  else
    "$@"
  fi
}

step() { printf '\n==> %s\n' "$*"; }

step "rustfmt"
cargo fmt --all --check

step "clippy"
cargo clippy --workspace --all-targets -- -D warnings

step "upstream file:line references (python -m oracle.upstream_refs)"
"$PY" -m oracle.upstream_refs

step "generated tables are up to date"
"$PY" oracle/codegen/libm_tables.py --check
"$PY" oracle/codegen/atan2_tables.py --check
"$PY" oracle/codegen/unicodedata_tables.py --check
"$PY" oracle/codegen/saslprep_tables.py --check
"$PY" oracle/codegen/pdfminer_tables.py --check
"$PY" oracle/codegen/pdffont_tables.py --check
"$PY" oracle/codegen/ccitt_tables.py --check
"$PY" oracle/codegen/py_symbols.py --check

step "unit oracle data (python -m oracle.unit)"
capped "$PY" -m oracle.unit
"$PY" -m oracle.codegen.cmaps --check

step "cargo test --release"
capped cargo test --release --workspace

step "cargo test on wasm32-wasip1 (Node WASI): the same unit replays compiled to WebAssembly"
CARGO_TARGET_WASM32_WASIP1_RUNNER="node --no-warnings $ROOT/scripts/wasi-runner.mjs" \
  capped cargo test --release --target wasm32-wasip1 -p pycompat -p pdfminer -p pdfplumber -p pdftrace -p pdfplumber-wasm

step "JavaScript package: CMap names, build (dist/, without wasm-opt), types, Node tests, size budget"
node scripts/build-cmaps.mjs --check-names
node scripts/build-wasm.mjs --no-opt
node scripts/build-cmaps.mjs
node scripts/build-js.mjs
node node_modules/typescript/bin/tsc -p js/tsconfig.test.json
capped node node_modules/vitest/vitest.mjs run --config js/vitest.config.ts
node scripts/size.mjs --budget

step "oracle self-tests"
"$PY" -m unittest discover -s oracle/tests -t .

printf '\ncheck.sh: all checks passed\n'
