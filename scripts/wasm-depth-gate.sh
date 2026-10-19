#!/usr/bin/env bash
# The deepest recursions Python accepts, run by the production WebAssembly
# profile (`dist`: LTO, one codegen unit, panic=abort), whose larger frames
# could exhaust the module's shadow stack where the `release` replays of
# scripts/check.sh do not (docs/architecture.md, "Profondeur de récursion";
# the stack size is set by crates/wasm/build.rs and crates/pdftrace/build.rs).
#
# Builds `pdftrace` natively (`release`) and for wasm32-wasip1 (`dist`), runs
# both on the depth windows of the adversarial corpora (chains of nested Form
# XObjects around Python's first failing level, `adv-interp`; page trees,
# `/Prev` and `/Length` chains, `adv-pages`, `adv-objects`) and requires the
# same traces. A stack overflow traps (`memory access out of bounds`) and
# fails.
#
# For `interp`, `fonts`, `pages` and `objects`, the native traces are those
# `oracle.verify` checks against Python. Not by construction for `layout`:
# both builds run here without `--oracle-dir`, so with `CreationOrder`,
# where `oracle.verify` hands `pdftrace` Python's `id()` ranking (the gate
# needs no oracle cache). They are the same traces only because these
# windows have no page whose analysis depends on that ranking: measured in
# October 2026 at the wave 3 finalization, the 188 `adv-interp` depth and
# C-limit files give identical native `layout` traces with and without
# `--oracle-dir` (docs/verification.md, "Outils"). A new depth file whose
# layout has `id()` ties would still be gated for the stack (wasm = native),
# not for Python parity.
#
#   scripts/wasm-depth-gate.sh        # run by scripts/check.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PDFPLUMBER_ROOT="${PDFPLUMBER_ROOT:-$ROOT}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export PATH="$HOME/.cargo/bin:$PATH"
CACHE="${PDFPLUMBER_CACHE:-$PDFPLUMBER_ROOT/.cache}"
PY="$PDFPLUMBER_ROOT/.venv/bin/python"
cd "$ROOT"

# the provider of the predefined CMaps: the directory the interp unit data names
if [[ -z "${PDFPLUMBER_CMAP_DIR:-}" ]]; then
  PDFPLUMBER_CMAP_DIR="$CACHE/$(PYTHONPATH="$ROOT" "$PY" -c 'from oracle.codegen import cmaps; from oracle.corpus import cache_dir; print(cmaps.ensure().relative_to(cache_dir()).as_posix())')"
  export PDFPLUMBER_CMAP_DIR
fi

cargo build --release -p pdftrace --bin pdftrace
cargo build --profile dist --target wasm32-wasip1 -p pdftrace --bin pdftrace
native="$ROOT/target/release/pdftrace"
wasm="$ROOT/target/wasm32-wasip1/dist/pdftrace.wasm"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
status=0
gate() {
  local stage=$1 set=$2
  shift 2
  local files=("$@")
  if [[ ${#files[@]} -eq 0 ]]; then
    echo "wasm-depth-gate: no depth file in $set (python -m oracle.unit regenerates it)" >&2
    status=1
    return
  fi
  # one file per run, on stdout (`--out` names its temporary files with
  # the process id, which WASI does not have)
  local f bad=0
  for f in "${files[@]}"; do
    "$native" --stage "$stage" "$f" >"$work/native.txt"
    if ! node --no-warnings "$ROOT/scripts/wasi-runner.mjs" "$wasm" --stage "$stage" "$f" \
      >"$work/wasm.txt" 2>"$work/err"; then
      echo "wasm-depth-gate: $stage on $(basename "$f"): the dist wasm failed" >&2
      head -c 2000 "$work/err" >&2
      bad=$((bad + 1))
    elif ! cmp -s "$work/native.txt" "$work/wasm.txt"; then
      echo "wasm-depth-gate: $stage on $(basename "$f"): the dist wasm's trace differs from the native one" >&2
      bad=$((bad + 1))
    fi
  done
  if [[ $bad -eq 0 ]]; then
    echo "wasm-depth-gate: $stage on $set: ${#files[@]} files, dist wasm = native"
  else
    status=1
  fi
}

shopt -s nullglob
interp=("$CACHE"/corpus/adv-interp/*-depth-*.pdf "$CACHE"/corpus/adv-interp/*-climit-*.pdf)
for stage in layout interp fonts; do
  gate "$stage" adv-interp "${interp[@]}"
done
gate pages adv-pages "$CACHE"/corpus/adv-pages/*depth*.pdf
gate objects adv-objects "$CACHE"/corpus/adv-objects/*depth*.pdf
exit "$status"
