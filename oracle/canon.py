"""Canonical text serialization of Python values for stage traces.

This is the Python half of the contract in ``docs/verification.md``
("Canonical Trace Format"); ``crates/pdftrace/src/canon.rs`` is the Rust
half. Both are cross-tested on the generated cases of
``oracle/unit/canon.py``.

Rules (``canon(v)``):

=====================  ==============================================================
Value                  Text
=====================  ==============================================================
None, True, False      ``None``, ``True``, ``False``
int (any size)         decimal (also beyond CPython's 4300-digit ``str()`` limit)
float                  ``repr()``: ``1.0``, ``1e-05``, ``1e+16``, ``-0.0``, ``inf``,
                       ``-inf``, ``nan`` (every NaN, whatever its sign or payload)
str                    ``json.dumps(s, ensure_ascii=True)``; non-BMP characters become
                       a surrogate pair of escapes, lone surrogates one escape
bytes (<= 256)         ``x"`` + lowercase hex + ``"``
bytes (> 256)          ``x#<len>:<first 16 hex digits of sha256>``
tuple                  ``(a, b)``; one element ``(a)``; empty ``()``
list                   ``[a, b]``; empty ``[]``
dict                   ``{k: v, ...}`` in insertion order, keys through ``canon``
PSLiteral              ``/`` + canon(name) (name is str or bytes): ``/"F1"``, ``/x"ff"``
PSKeyword              ``K`` + canon(name bytes): ``Kx"4254"``
PDFObjRef              ``R`` + canon(objid): ``R12``
PDFStream              ``Stream(objid, genno, attrs, raw)`` with
                       ``raw = canon(stream.rawdata)``
exception              ``!`` + class name + ``(`` + json(str(e)) + ``)``
Named(name, fields)    ``name`` + canon(fields dict): ``LTChar{"x0": 1.0}``
=====================  ==============================================================

Details that the Rust side must reproduce:

- ``bool`` is tested before ``int`` (``True`` is an ``int`` in Python).
- ``PDFStream.rawdata`` is ``None`` once the stream has been decoded
  (``pdftypes.py`` ``PDFStream.decode`` sets ``self.rawdata = None``), and the
  canon then shows ``None`` for ``raw``. This is deliberate: it exposes the
  decode cache state, which the port must reproduce (``get_rawdata()`` is
  public API). A stream canon therefore depends on whether, earlier in the same
  run, something called ``get_data()`` on that very object; every stage
  specifies its traversal order so that this is replicable. ``raw`` is the
  stored ``rawdata``: still encrypted for an encrypted document.
- Exception class names are ``type(e).__name__`` for builtins, pdfminer,
  pdfplumber and oracle classes, and ``<module>.<name>`` otherwise
  (``zlib.error``, ``struct.error``, ``binascii.Error``), see ``exception_name``.
- In an exception message, CPython default object reprs carry a memory address
  (``<pdfminer.pdfparser.PDFParser object at 0x7f3a...>``); every
  `` at 0x<hex>>`` is replaced by `` at 0x?>`` so that traces are reproducible.
- ``ExcValue`` (an exception decoded from typed JSON) is written like the
  exception it stands for; it is how unit cases carry exceptions.

``canon`` is iterative: nesting depth is unbounded (a pathological PDF can
nest arrays thousands deep) and the oracle's own recursion never interferes
with the recursion limit seen by pdfminer. A cyclic structure or an
unsupported type raises ``CanonError``, which derives from ``BaseException``
on purpose: a harness bug must never be caught by a stage's
``except Exception`` and end up in a trace as if pdfminer had raised it.
"""

from __future__ import annotations

import hashlib
import json
import re
from typing import Any

from pdfminer.pdftypes import PDFObjRef, PDFStream
from pdfminer.psparser import PSKeyword, PSLiteral

__all__ = [
    "BYTES_INLINE_MAX",
    "CanonError",
    "ExcValue",
    "Named",
    "canon",
    "canon_exception",
    "exception_message",
    "exception_name",
    "int_to_decimal",
]

#: Byte strings up to this length are written in full (``x"..."``).
BYTES_INLINE_MAX = 256

_ADDRESS = re.compile(r" at 0x[0-9a-fA-F]+>")
_SEP = ", "


class CanonError(BaseException):
    """A value the canonical format cannot represent (a harness bug)."""


class Named:
    """A stage-specific object written as ``Name{...}`` (``LTChar{...}``).

    ``fields`` is a dict (insertion order is kept) or an iterable of pairs.
    """

    __slots__ = ("name", "fields")

    def __init__(self, name: str, fields: Any) -> None:
        if not isinstance(name, str) or not name:
            raise CanonError(f"Named: name must be a non-empty str, not {name!r}")
        self.name = name
        self.fields = fields if isinstance(fields, dict) else dict(fields)

    def __eq__(self, other: object) -> bool:
        return isinstance(other, Named) and (self.name, self.fields) == (other.name, other.fields)

    def __repr__(self) -> str:
        return f"Named({self.name!r}, {self.fields!r})"


class ExcValue:
    """An exception known only by class name and message (from typed JSON)."""

    __slots__ = ("name", "message")

    def __init__(self, name: str, message: str) -> None:
        self.name = name
        self.message = message

    def __eq__(self, other: object) -> bool:
        return isinstance(other, ExcValue) and (self.name, self.message) == (other.name, other.message)

    def __repr__(self) -> str:
        return f"ExcValue({self.name!r}, {self.message!r})"


_SHORT_NAME_MODULES = ("builtins", "pdfminer", "pdfplumber", "oracle")


def exception_name(e: BaseException) -> str:
    """Class name used in traces: bare for builtins/pdfminer/pdfplumber/oracle."""
    cls = type(e)
    module = cls.__module__ or ""
    if module.split(".")[0] in _SHORT_NAME_MODULES:
        return cls.__name__
    return f"{module}.{cls.__name__}"


def exception_message(e: BaseException) -> str:
    """``str(e)``, as the trace and typed JSON record it (before normalization)."""
    return str(e)


def _normalize_message(message: str) -> str:
    return _ADDRESS.sub(" at 0x?>", message)


def _exception_text(name: str, message: str) -> str:
    return "!" + name + "(" + json.dumps(_normalize_message(message), ensure_ascii=True) + ")"


def canon_exception(e: BaseException | ExcValue) -> str:
    """The trace line for an exception: ``!Name("message")``."""
    if isinstance(e, ExcValue):
        return _exception_text(e.name, e.message)
    return _exception_text(exception_name(e), exception_message(e))


def int_to_decimal(n: int) -> str:
    """Decimal text of any int, also past CPython's ``int_max_str_digits``."""
    try:
        return str(n)
    except ValueError:  # more digits than sys.get_int_max_str_digits() allows
        pass
    sign = "-" if n < 0 else ""
    n = -n if n < 0 else n
    base = 10**18
    chunks = []
    while n:
        n, r = divmod(n, base)
        chunks.append(r)
    head = "%d" % chunks[-1]
    return sign + head + "".join("%018d" % c for c in reversed(chunks[:-1]))


def _bytes_text(b: bytes) -> str:
    if len(b) <= BYTES_INLINE_MAX:
        return 'x"' + b.hex() + '"'
    return "x#" + str(len(b)) + ":" + hashlib.sha256(b).hexdigest()[:16]


def _scalar(v: Any) -> str | None:
    """Text of a leaf value, or None if ``v`` is a container."""
    t = type(v)
    if t is int:
        return int_to_decimal(v)
    if t is float:
        return repr(v)
    if t is str:
        return json.dumps(v, ensure_ascii=True)
    if t is bytes:
        return _bytes_text(v)
    if v is None:
        return "None"
    if t is bool:
        return "True" if v else "False"
    if t is PSLiteral:
        name = v.name
        if type(name) is str:
            return "/" + json.dumps(name, ensure_ascii=True)
        if type(name) is bytes:
            return "/" + _bytes_text(name)
        raise CanonError(f"canon: PSLiteral name of type {type(name).__name__}")
    if t is PSKeyword:
        if type(v.name) is not bytes:
            raise CanonError(f"canon: PSKeyword name of type {type(v.name).__name__}")
        return "K" + _bytes_text(v.name)
    if isinstance(v, (BaseException, ExcValue)):
        return canon_exception(v)
    return None


def canon(value: Any) -> str:
    """Canonical text of ``value`` (see the module docstring)."""
    leaf = _scalar(value)
    if leaf is not None:
        return leaf
    out: list[str] = []
    # Work items: ("v", value) to serialize, ("s", text) to emit, ("x", id) to
    # leave a container (cycle detection).
    stack: list[tuple[str, Any]] = [("v", value)]
    active: set[int] = set()
    while stack:
        kind, item = stack.pop()
        if kind == "s":
            out.append(item)
            continue
        if kind == "x":
            active.discard(item)
            continue
        leaf = _scalar(item)
        if leaf is not None:
            out.append(leaf)
            continue
        t = type(item)
        if t is list or t is tuple or t is dict or t is Named:
            key = id(item)
            if key in active:
                raise CanonError(f"canon: cyclic {t.__name__}")
            active.add(key)
            stack.append(("x", key))
            if t is list:
                _push_sequence(stack, "[", item, "]")
            elif t is tuple:
                _push_sequence(stack, "(", item, ")")
            elif t is dict:
                _push_mapping(stack, item)
            else:
                out.append(item.name)
                _push_mapping(stack, item.fields)
            continue
        if t is PDFObjRef:
            stack.append(("v", item.objid))
            out.append("R")
            continue
        if t is PDFStream:
            parts = (item.objid, item.genno, item.attrs, item.rawdata)
            _push_sequence(stack, "Stream(", parts, ")")
            continue
        raise CanonError(f"canon: unsupported type {t.__module__}.{t.__qualname__}")
    return "".join(out)


def _push_sequence(stack: list[tuple[str, Any]], open_: str, items: Any, close: str) -> None:
    stack.append(("s", close))
    n = len(items)
    for i in range(n - 1, -1, -1):
        stack.append(("v", items[i]))
        if i:
            stack.append(("s", _SEP))
    stack.append(("s", open_))


def _push_mapping(stack: list[tuple[str, Any]], mapping: dict[Any, Any]) -> None:
    if type(mapping) is not dict:
        raise CanonError(f"canon: Named fields must be a dict, not {type(mapping).__name__}")
    stack.append(("s", "}"))
    pairs = list(mapping.items())
    for i in range(len(pairs) - 1, -1, -1):
        k, v = pairs[i]
        stack.append(("v", v))
        stack.append(("s", ": "))
        stack.append(("v", k))
        if i:
            stack.append(("s", _SEP))
    stack.append(("s", "{"))
