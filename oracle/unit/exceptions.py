"""Exception classes of pdfminer and pdfplumber, for pycompat::ExcKind.

Writes ``<cache>/unit/exceptions/classes.jsonl`` (see
``oracle/unit/_common.py``): one line per
``Exception`` subclass defined in a pdfminer or pdfplumber module (nested
classes included), ``{"in": [module, qualname], "out": [name, [[base module,
base qualname], ...]]}`` in typed JSON. The Rust test
(crates/pdftrace/tests/exceptions_unit.rs) checks that ExcKind lists every
class with the same name and bases.

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.exceptions
"""

import importlib
import pkgutil

from ._common import require_oracle_python, write_cases, write_meta

# Modules that are out of scope and pull optional dependencies.
SKIP = {"pdfplumber.display", "pdfplumber.cli", "pdfminer.high_level", "pdfminer.image"}


def classes_in(obj, seen):
    for value in list(vars(obj).values()):
        if isinstance(value, type) and value not in seen:
            seen.add(value)
            yield value
            yield from classes_in(value, seen)


def main():
    require_oracle_python()
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
    cases = (
        ([module, qualname], [cls.__name__, [[b.__module__, b.__qualname__] for b in cls.__bases__]])
        for (module, qualname), cls in sorted(found.items())
    )
    n = write_cases("exceptions", "classes", cases)
    write_meta("exceptions", classes=n)
    print(f"wrote {n} classes")

if __name__ == "__main__":
    main()
