#!/usr/bin/env bash
# The `Ok(..)` pass of cargo mutants for functions returning `PyResult<T>`.
#
# cargo-mutants (27.1) recognises a `Result` only by the name of its type, so
# for the alias `pycompat::PyResult<T>` it generates `PyResult::new(..)` /
# `PyResult::from(..)` body replacements, which never compile ("unviable"),
# and no `Ok(..)` ones: the replacement of a whole body by a constant result
# is never tried. This script copies the checkout's tracked and untracked
# (non-ignored) files to a scratch directory, rewrites `PyResult<T>` as
# `Result<T, pycompat::PyErr>` in the files given with -f/--file, and runs
# cargo mutants there on the `Ok(..)` mutants only. The usual pass, on the
# checkout itself, covers every other mutant (docs/verification.md).
#
# Every argument goes to cargo mutants; run it through scripts/heavy.sh:
#
#   scripts/heavy.sh scripts/mutants-ok.sh -f crates/pdfminer/src/arcfour.rs \
#     --profile mutants -j 1 --timeout 60 -- --package=pdftrace --test=crypto_unit
#
# MUTANTS_OK_DIR (default: a new temporary directory) holds the copy (ws/)
# and, unless -o/--output is given, the results (mutants.out/).
set -euo pipefail

SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PDFPLUMBER_ROOT="${PDFPLUMBER_ROOT:-$SRC}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export PATH="$HOME/.cargo/bin:$PATH"
DIR="${MUTANTS_OK_DIR:-$(mktemp -d -t mutants-ok.XXXXXX)}"
WS="$DIR/ws"

files=()
has_output=0
args=("$@")
for ((i = 0; i < ${#args[@]}; i++)); do
  a="${args[i]}"
  case "$a" in
    --) break ;;
    -f | --file) files+=("${args[i + 1]}"); i=$((i + 1)) ;;
    --file=*) files+=("${a#--file=}") ;;
    -o | --output | --output=*) has_output=1 ;;
  esac
done
if [[ ${#files[@]} -eq 0 ]]; then
  echo "mutants-ok.sh: give the files to mutate with -f FILE" >&2
  exit 2
fi

rm -rf "$WS"
mkdir -p "$WS"
# --no-recursion: a submodule (a gitlink) is copied as an empty directory.
git -C "$SRC" ls-files -z --cached --others --exclude-standard \
  | (cd "$SRC" && tar --null --no-recursion -T - -cf -) \
  | tar -xf - -C "$WS"

for f in "${files[@]}"; do
  # PyResult<T> -> Result<T, pycompat::PyErr>, T with up to two levels of <>.
  perl -0pi -e 's/(?<![A-Za-z0-9_:])(?:pycompat::)?PyResult<((?:[^<>]|<(?:[^<>]|<[^<>]*>)*>)*)>/Result<$1, pycompat::PyErr>/g' "$WS/$f"
  if grep -n 'PyResult<' "$WS/$f"; then
    echo "mutants-ok.sh: $f: PyResult<..> left after the rewrite (nested deeper than two levels?)" >&2
    exit 2
  fi
done

out=()
[[ $has_output -eq 1 ]] || out=(-o "$DIR")
echo "mutants-ok.sh: copy in $WS, results in ${out[1]:-the given --output}/mutants.out" >&2
cd "$WS"
exec cargo mutants --re 'with Ok\(' "${out[@]}" "$@"
