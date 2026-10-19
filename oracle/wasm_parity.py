"""Compare the WebAssembly ``pdftrace`` with the native one, file by file.

    python -m oracle.wasm_parity [--set quick] [--stages interp,fonts,layout]
                                 [--max-pages N] [--limit K] [--timeout S]

Runs ``target/release/pdftrace`` (native) and
``target/wasm32-wasip1/dist/pdftrace.wasm`` (the production profile, under
Node's WASI through ``scripts/wasi-runner.mjs``) on each file of the set and
requires byte-identical traces and exit statuses. Build both first::

    cargo build --release -p pdftrace --bin pdftrace
    cargo build --profile dist --target wasm32-wasip1 -p pdftrace --bin pdftrace

Both runs get exactly what ``oracle.verify`` gives the native binary, so an
identical trace is Python parity for the Wasm build wherever
``oracle.verify`` finds the native one identical:

- the CMap provider is passed explicitly (``--cmap-dir``, the directory of
  ``oracle/codegen/cmaps.py``, written when missing), whatever the
  environment holds: without it both builds would agree on a "no CMap
  provider" trace for the files that load a predefined CMap (3 of the 150
  files of ``quick`` in October 2026);
- for a stage whose module sets ``ORACLE_INPUT`` (``layout``), the oracle
  cache directory of the stage (``--oracle-dir``, Python's ``id()``
  ranking), built first for the missing files as ``oracle.verify`` builds
  it; files whose oracle trace is not ``ok`` are skipped, as verify skips
  them.

One file per process and the Wasm trace written to a file: WASI has no
process id for ``--out``, and Node's WASI writes a pipe non-blocking (a large
trace fails with ``EAGAIN``). A heavy job: run it through
``scripts/heavy.sh``.

Exit status: 0 if every file compared is identical in every stage, 1
otherwise, 2 if a binary is missing.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from oracle.cache import OracleCache
from oracle.codegen import cmaps
from oracle.corpus import load_set
from oracle.verify import oracle_input

ROOT = Path(__file__).resolve().parent.parent


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(prog="python -m oracle.wasm_parity", description=__doc__.split("\n\n")[0])
    p.add_argument("--set", default="quick")
    p.add_argument("--stages", default="interp,fonts,layout")
    p.add_argument("--max-pages", type=int, default=20)
    p.add_argument("--limit", type=int)
    p.add_argument("--timeout", type=float, default=600)
    args = p.parse_args(argv)

    native = ROOT / "target/release/pdftrace"
    wasm = ROOT / "target/wasm32-wasip1/dist/pdftrace.wasm"
    runner = ROOT / "scripts/wasi-runner.mjs"
    for b in (native, wasm):
        if not b.is_file():
            print(f"wasm_parity: no {b} (see python -m oracle.wasm_parity --help)", file=sys.stderr)
            return 2
    cmap_dir = cmaps.ensure()
    print(f"CMap provider: {cmap_dir}", flush=True)
    files = load_set(args.set)
    if args.limit is not None:
        files = files[: args.limit]

    bad = 0
    with tempfile.TemporaryDirectory(prefix="wasm-parity-") as tmp:
        out = Path(tmp) / "wasm.txt"
        for stage in args.stages.split(","):
            extra = ["--max-pages", str(args.max_pages), "--cmap-dir", str(cmap_dir)]
            todo = files
            if oracle_input(stage):
                cache = OracleCache(stage, args.max_pages)
                results = cache.build(files, timeout=args.timeout, progress=False)
                todo = [r.file for r in results if r.status == "ok"]
                extra += ["--oracle-dir", str(cache.dir)]
            started = time.monotonic()
            differ = 0
            for f in todo:
                cmd = ["--stage", stage, *extra] + ([f"--password={f.password}"] if f.password is not None else []) + [str(f.path)]
                a = subprocess.run([str(native), *cmd], capture_output=True, timeout=args.timeout)
                try:
                    with open(out, "wb") as fo:
                        b = subprocess.run(
                            ["node", "--no-warnings", str(runner), str(wasm), *cmd],
                            stdout=fo,
                            stderr=subprocess.PIPE,
                            timeout=args.timeout,
                        )
                except subprocess.TimeoutExpired:
                    print(f"{stage} {f.rel}: the wasm run timed out", flush=True)
                    differ += 1
                    continue
                if b.returncode != a.returncode or out.read_bytes() != a.stdout:
                    err = b.stderr[:300].decode("utf-8", "replace")
                    print(f"{stage} {f.rel}: differs (native status {a.returncode}, wasm {b.returncode}) {err}", flush=True)
                    differ += 1
            skipped = len(files) - len(todo)
            print(
                f"{stage} {args.set}: {len(todo)} files, {differ} differ"
                + (f", {skipped} skipped (oracle not ok)" if skipped else "")
                + f" ({time.monotonic() - started:.0f} s)",
                flush=True,
            )
            bad += differ
    print("dist wasm = native" if bad == 0 else f"{bad} differences", flush=True)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
