#!/usr/bin/env bash
# Local CI: formatting, lints, upstream references, generated tables, Rust
# tests on the unit oracle data (natively and compiled to WebAssembly, run by
# Node's WASI), the deepest recursions on the production Wasm profile
# (scripts/wasm-depth-gate.sh), the JavaScript package (Wasm build, Node
# tests and the sessions of the public API on the sources and the dist
# build, size budget; the browsers are scripts/check-browsers.sh), the
# four plumber_* stages through the public API on micro and adv-plumber
# (oracle.api_parity: each trace that is not Python's classified by its
# content), and the oracle self-tests. Regenerates the unit oracle data first when it is
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
"$PY" oracle/codegen/plumber_ops.py --check

step "unit oracle data (python -m oracle.unit)"
capped "$PY" -m oracle.unit
"$PY" -m oracle.codegen.cmaps --check

step "cargo test --release"
capped cargo test --release --workspace

step "cargo test on wasm32-wasip1 (Node WASI): the same unit replays compiled to WebAssembly"
CARGO_TARGET_WASM32_WASIP1_RUNNER="node --no-warnings $ROOT/scripts/wasi-runner.mjs" \
  capped cargo test --release --target wasm32-wasip1 -p pycompat -p pdfminer -p pdfplumber -p pdftrace -p pdfplumber-wasm

step "cargo test on wasm32-wasip1: the tests that need a process of their own (ignored above), one process each"
# No fresh thread per test under WebAssembly: these leave CMaps in the
# provider and CMapDB's caches (crates/wasm/tests/js_runtime_unit.rs).
for t in a_void_call_leaves_no_state_and_its_rerun_matches_pdfminer a_void_call_puts_the_process_state_back \
  a_void_call_rebuilds_its_document_by_replaying_its_history page_operations_match_pdfplumber_through_the_boundary \
  the_history_goes_once_every_cmap_is_handed_over the_first_void_call_of_a_document_asks_for_every_cmap_its_fonts_name \
  the_scan_reads_every_page_of_the_document; do
  out=$(CARGO_TARGET_WASM32_WASIP1_RUNNER="node --no-warnings $ROOT/scripts/wasi-runner.mjs" \
    capped cargo test --release --target wasm32-wasip1 -p pdfplumber-wasm --test js_runtime_unit -- --ignored --exact "$t" 2>&1) \
    || { printf '%s\n' "$out"; exit 1; }
  printf '%s\n' "$out" | grep -E "^test |test result"
  # the filter must have selected the test: one ran, none left ignored
  printf '%s\n' "$out" | grep -q "test result: ok. 1 passed; 0 failed; 0 ignored" \
    || { echo "check.sh: $t did not run alone under WebAssembly" >&2; exit 1; }
done

step "the deepest recursions Python accepts on the production Wasm profile (dist), against native"
capped scripts/wasm-depth-gate.sh

step "JavaScript package: CMap names, build (dist/, without wasm-opt), types, Node tests, size budget"
node scripts/build-cmaps.mjs --check-names
node scripts/build-wasm.mjs --no-opt
node scripts/build-cmaps.mjs
node scripts/build-js.mjs
node node_modules/typescript/bin/tsc -p js/tsconfig.test.json
capped node node_modules/vitest/vitest.mjs run --config js/vitest.config.ts
node scripts/size.mjs --budget

step "the npm package as a consumer gets it: files, notices, links, declarations, import and require"
node scripts/third-party-notices.mjs --check
capped node scripts/check-package.mjs

step "the four plumber_* stages through the public JavaScript API on micro and adv-plumber, against Python and pdftrace (oracle.api_parity; dist/pdfplumber-oracle.wasm with wasm-opt, as the package ships its module)"
node scripts/build-wasm.mjs --oracle
cargo build --release -p pdftrace
for stage in plumber_objects plumber_text plumber_tables plumber_doc; do
  for set in micro adv-plumber; do
    capped "$PY" -m oracle.api_parity --stage "$stage" --set "$set" -j 2
  done
done

step "oracle self-tests"
"$PY" -m unittest discover -s oracle/tests -t .

printf '\ncheck.sh: all checks passed\n'
