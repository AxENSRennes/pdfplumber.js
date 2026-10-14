#!/usr/bin/env bash
# Local CI: formatting, lints, Rust tests on the unit oracle data, Wasm build
# and the oracle self-tests. Regenerates the unit oracle data first when it is
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
    systemd-run --user --scope --quiet -p MemoryMax=5G -p MemorySwapMax=0 "$@"
  else
    "$@"
  fi
}

step() { printf '\n==> %s\n' "$*"; }

step "rustfmt"
cargo fmt --all --check

step "clippy"
cargo clippy --workspace --all-targets -- -D warnings

step "unit oracle data (python -m oracle.unit)"
capped "$PY" -m oracle.unit

step "cargo test --release"
capped cargo test --release --workspace

step "wasm32 build (dist profile)"
cargo build --target wasm32-unknown-unknown --profile dist -p pdfplumber-wasm

step "oracle self-tests"
"$PY" -m unittest discover -s oracle/tests -t .

printf '\ncheck.sh: all checks passed\n'
