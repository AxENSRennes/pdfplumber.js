"""Exception classes of pdfminer and pdfplumber, for pycompat::ExcKind.

Writes ``$PDFPLUMBER_ROOT/.cache/unit/exceptions/classes.jsonl``: one line per
``Exception`` subclass defined in a pdfminer or pdfplumber module (nested
classes included), ``{"in": [module, qualname], "out": [name, [[base module,
base qualname], ...]]}`` in typed JSON. The Rust test
(crates/pdftrace/tests/exceptions_unit.rs) checks that ExcKind lists every
class with the same name and bases.

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python oracle/unit/exceptions.py
"""

import hashlib
import importlib
import json
import os
import pkgutil
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))

# Modules that are out of scope and pull optional dependencies.
SKIP = {"pdfplumber.display", "pdfplumber.cli", "pdfminer.high_level", "pdfminer.image"}


def classes_in(obj, seen):
    for value in list(vars(obj).values()):
        if isinstance(value, type) and value not in seen:
            seen.add(value)
            yield value
            yield from classes_in(value, seen)


def main():
    if sys.version_info[:2] != (3, 12):
        sys.exit(f"the oracle is CPython 3.12, not {sys.version.split()[0]}")
    found = {}
    for package_name in ("pdfminer", "pdfplumber"):
        package = importlib.import_module(package_name)
        names = [package_name] + [m.name for m in pkgutil.walk_packages(package.__path__, package_name + ".")]
        for name in names:
            if name in SKIP:
                continue
            module = importlib.import_module(name)
            for cls in classes_in(module, set()):
                if issubclass(cls, Exception) and cls.__module__.split(".")[0] in ("pdfminer", "pdfplumber"):
                    found[(cls.__module__, cls.__qualname__)] = cls
    root = os.environ.get("PDFPLUMBER_ROOT") or REPO
    outdir = os.path.join(root, ".cache", "unit", "exceptions")
    os.makedirs(outdir, exist_ok=True)
    with open(os.path.join(outdir, "classes.jsonl"), "w") as f:
        for (module, qualname), cls in sorted(found.items()):
            bases = [[b.__module__, b.__qualname__] for b in cls.__bases__]
            f.write(json.dumps({"in": [module, qualname], "out": [cls.__name__, bases]}) + "\n")
    with open(os.path.abspath(__file__), "rb") as f:
        digest = hashlib.sha256(f.read()).hexdigest()
    with open(os.path.join(outdir, "meta.json"), "w") as f:
        json.dump({"generator_sha256": digest, "python": sys.version.split()[0], "classes": len(found)}, f)
    print(f"wrote {len(found)} classes to {outdir}")


if __name__ == "__main__":
    main()
