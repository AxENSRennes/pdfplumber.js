#!/usr/bin/env bash
# Sets up a development checkout (macOS or Linux). Safe to re-run.
#
# Needs: git, uv, Node 26 (.node-version), rustup. Does not use sudo; on a fresh
# Linux box, browser system libraries may need `sudo npx playwright install-deps`.
#
#   scripts/setup-dev.sh                 # deps, toolchains, browsers, external PDFs, wasm builds
#   scripts/setup-dev.sh --experiments   # also regenerate the experiment corpora and verify the ports
set -euo pipefail
cd "$(dirname "$0")/.."

step() { printf '\n==> %s\n' "$*"; }

step "Submodules (mupdf.js is no longer needed)"
git submodule update --init pdfplumber-python pdfminer-six pdfjs

step "Python oracle (.python-version: pdfplumber output depends on the Python minor version)"
uv sync
.venv/bin/python -c 'import sys; assert sys.version_info[:2] == (3, 12), sys.version; print(sys.version)'

step "Node $(cat .node-version)"
node_major="$(node -p 'process.versions.node.split(".")[0]')"
if [ "$node_major" != "$(cat .node-version)" ]; then
  echo "Node $(node --version) found, expected $(cat .node-version).x" >&2
  exit 1
fi
npm ci

step "Rust toolchain (rust-toolchain.toml)"
rustup show active-toolchain || rustup toolchain install
rustup target add wasm32-unknown-unknown

step "Playwright browsers"
npx playwright install chromium webkit firefox

step "External PDFs (gitignored; downloaded and SHA-256 checked from the manifests)"
if ! npm run --silent fixtures:download:external-all; then
  echo "Some downloads failed: copy test/fixtures/external-*pdfs/ from a machine that has them." >&2
fi

step "Experiment wasm builds"
for exp in psparser-bench tables-bench; do
  (cd "experiments/$exp/rust" && cargo build --release --target wasm32-unknown-unknown -q)
done
cp experiments/psparser-bench/rust/target/wasm32-unknown-unknown/release/psparser.wasm experiments/psparser-bench/rust/
cp experiments/tables-bench/rust/target/wasm32-unknown-unknown/release/tables.wasm experiments/tables-bench/rust/

if [ "${1:-}" = "--experiments" ]; then
  step "Experiment corpora and Python oracles (a few minutes)"
  .venv/bin/python experiments/psparser-bench/dump_corpus.py 2>/dev/null
  .venv/bin/python experiments/psparser-bench/dump_corpus.py --fuzz 4000
  .venv/bin/python experiments/tables-bench/dump_tables.py 2>/dev/null
  .venv/bin/python experiments/tables-bench/dump_tables.py --fuzz 3000

  step "Ports vs oracles"
  for impl in ts wasm; do
    for corpus in corpus fuzz; do
      npx tsx experiments/psparser-bench/ts/verify.ts "$impl" --corpus "$corpus"
      npx tsx experiments/tables-bench/ts/verify.ts "$impl" --corpus "$corpus"
    done
  done
fi

step "Done"
