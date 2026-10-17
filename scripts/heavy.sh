#!/usr/bin/env bash
# Runs a heavy job (cargo mutants, a pass over a whole corpus set, adversarial
# corpus generation) in one of HEAVY_SLOTS machine-wide slots, inside a cgroup
# capped at HEAVY_MEM without swap. The machine is shared: a mutant that loops
# while allocating once filled 16.5 GB of RAM and froze it.
#
#   scripts/heavy.sh cargo mutants --profile mutants -j 1 --timeout 20 -f crates/...
#   scripts/heavy.sh .venv/bin/python -m oracle.verify --stage objects --set all
#
# OOMPolicy=continue: the OOM killer kills the process that exhausted the cap
# (a mutant's test binary: a failed test, so a caught mutant) and the scope
# goes on; the default policy (stop) would terminate the whole scope, and
# cargo mutants with it ("interrupted").
#
# Waits for a free slot. Slots are lock files under $PDFPLUMBER_ROOT/.cache/locks,
# shared by every checkout and worktree of the repository. With HEAVY_SLOTS=1
# (the rule since the incident of October 2026), the wait is a blocking
# flock, which takes the slot as soon as it is released; with more slots,
# each waiter polls every 5 s.
set -uo pipefail

ROOT=${PDFPLUMBER_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}
SLOTS=${HEAVY_SLOTS:-2}
MEM=${HEAVY_MEM:-5G}
LOCKS="$ROOT/.cache/locks"
mkdir -p "$LOCKS"

if [ $# -eq 0 ]; then
  echo "usage: scripts/heavy.sh COMMAND [ARGS...]" >&2
  exit 2
fi

if [ "$SLOTS" -eq 1 ]; then
  # One slot: a blocking flock, woken when the slot is released (a waiter
  # polling every 5 s can be passed over again and again).
  exec {fd}>"$LOCKS/heavy.0"
  if ! flock -n "$fd"; then
    echo "heavy.sh: the slot is busy, waiting" >&2
    flock "$fd"
    echo "heavy.sh: got slot 0" >&2
  fi
  systemd-run --user --scope --quiet -p MemoryMax="$MEM" -p MemorySwapMax=0 -p OOMPolicy=continue "$@"
  exit $?
fi

waited=0
while true; do
  for i in $(seq 0 $((SLOTS - 1))); do
    exec {fd}>"$LOCKS/heavy.$i"
    if flock -n "$fd"; then
      [ "$waited" -eq 1 ] && echo "heavy.sh: got slot $i" >&2
      systemd-run --user --scope --quiet -p MemoryMax="$MEM" -p MemorySwapMax=0 -p OOMPolicy=continue "$@"
      exit $?
    fi
    exec {fd}>&-
  done
  if [ "$waited" -eq 0 ]; then
    echo "heavy.sh: all $SLOTS slots busy, waiting" >&2
    waited=1
  fi
  sleep 5
done
