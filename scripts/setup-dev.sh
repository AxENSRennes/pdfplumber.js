#!/usr/bin/env bash
# Sets up a development checkout (macOS or Linux). Safe to re-run.
#
# Needs: git, uv, Node 26 (.node-version), rustup. Does not use sudo; on a fresh
# Linux box, browser system libraries may need `sudo npx playwright install-deps`.
set -euo pipefail
cd "$(dirname "$0")/.."

step() { printf '\n==> %s\n' "$*"; }

step "Submodules (sources ported and oracle code, plus the pdf.js test corpus)"
git submodule update --init upstream/pdfplumber-python upstream/pdfminer-six upstream/pdfjs

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

step "External PDFs (gitignored; downloaded and SHA-256 checked from the manifests)"
if ! node scripts/download-corpus.mjs; then
  echo "Some downloads failed: copy corpus/external*/ from a machine that has them." >&2
fi

step "Done"
