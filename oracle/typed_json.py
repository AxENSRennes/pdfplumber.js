"""Typed JSON: lossless exchange of Python values with the Rust side.

Python half of "Typed JSON" in ``docs/verification.md``;
``crates/pdftrace/src/typed_json.rs`` is the Rust half. Unit cases
(``oracle/unit/*.py``) are JSON Lines files whose values use this encoding.

=====================  ===========================================================
Python value           JSON
=====================  ===========================================================
None, True, False      ``null``, ``true``, ``false``
str                    JSON string; a str holding a surrogate code point
                       (U+D800..U+DFFF, which UTF-8 cannot carry) is
                       ``{"s": [<code point>, ...]}`` instead
int                    ``{"i": "<decimal>"}``
float                  ``{"f": "<16 hex digits of the IEEE-754 bits, big-endian>"}``
bytes                  ``{"b": "<hex>"}``
list                   JSON array
tuple                  ``{"t": [...]}``
dict                   ``{"d": [[key, value], ...]}`` (insertion order)
PSLiteral              ``{"lit": <str or {"b": hex}>}``
PSKeyword              ``{"kw": "<hex>"}``
PDFObjRef              ``{"ref": <objid>}`` (a JSON integer when the objid is an int
                       that fits in an i64, else a typed value)
PDFStream              ``{"stream": {"objid": v, "genno": v, "attrs": {"d": ...},
                       "rawdata": v, "data": v}}`` (typed values; rawdata/data
                       may be null)
exception              ``{"exc": "<class name>", "msg": <str(e)>}`` (class name as
                       in ``canon.exception_name``); decodes to ``canon.ExcValue``
Named                  ``{"named": "<name>", "d": [[key, value], ...]}``
=====================  ===========================================================

Every JSON object has exactly one key except ``exc`` (``exc`` + ``msg``) and
``named`` (``named`` + ``d``). The output of ``dumps`` is pure ASCII
(``ensure_ascii``), compact, with keys in the order above.
"""

from __future__ import annotations

import json
import struct
from typing import Any

from pdfminer.pdftypes import PDFObjRef, PDFStream
from pdfminer.psparser import LIT, KWD, PSKeyword, PSLiteral

from .canon import CanonError, ExcValue, Named, exception_message, exception_name, int_to_decimal

__all__ = ["decode", "dumps", "encode", "loads"]

_I64_MIN, _I64_MAX = -(2**63), 2**63 - 1


def _has_surrogate(s: str) -> bool:
    return any(0xD800 <= ord(c) <= 0xDFFF for c in s)


def _encode_str(s: str) -> Any:
    if _has_surrogate(s):
        return {"s": [ord(c) for c in s]}
    return s


def _encode_bytes(b: bytes) -> dict[str, str]:
    return {"b": b.hex()}


def encode(v: Any) -> Any:
    """Python value -> JSON-compatible structure."""
    t = type(v)
    if v is None or t is bool:
        return v
    if t is str:
        return _encode_str(v)
    if t is int:
        return {"i": int_to_decimal(v)}
    if t is float:
        return {"f": struct.pack(">d", v).hex()}
    if t is bytes:
        return _encode_bytes(v)
    if t is list:
        return [encode(x) for x in v]
    if t is tuple:
        return {"t": [encode(x) for x in v]}
    if t is dict:
        return {"d": [[encode(k), encode(x)] for k, x in v.items()]}
    if t is PSLiteral:
        name = v.name
        return {"lit": _encode_str(name) if type(name) is str else _encode_bytes(name)}
    if t is PSKeyword:
        return {"kw": v.name.hex()}
    if t is PDFObjRef:
        objid = v.objid
        if type(objid) is int and _I64_MIN <= objid <= _I64_MAX:
            return {"ref": objid}
        return {"ref": encode(objid)}
    if t is PDFStream:
        return {
            "stream": {
                "objid": encode(v.objid),
                "genno": encode(v.genno),
                "attrs": encode(v.attrs),
                "rawdata": encode(v.rawdata),
                "data": encode(v.data),
            }
        }
    if t is Named:
        return {"named": v.name, "d": [[encode(k), encode(x)] for k, x in v.fields.items()]}
    if t is ExcValue:
        return {"exc": v.name, "msg": _encode_str(v.message)}
    if isinstance(v, BaseException):
        return {"exc": exception_name(v), "msg": _encode_str(exception_message(v))}
    raise CanonError(f"typed_json: unsupported type {t.__module__}.{t.__qualname__}")


def _decode_str(j: Any) -> str:
    if type(j) is str:
        return j
    if type(j) is dict and set(j) == {"s"}:
        return "".join(chr(c) for c in j["s"])
    raise ValueError(f"typed_json: not a str: {j!r}")


def _decode_bytes(j: Any) -> bytes:
    if type(j) is dict and set(j) == {"b"}:
        return bytes.fromhex(j["b"])
    raise ValueError(f"typed_json: not bytes: {j!r}")


def decode(j: Any) -> Any:
    """JSON-compatible structure -> Python value (inverse of ``encode``)."""
    if j is None or type(j) is bool or type(j) is str:
        return j
    if type(j) is list:
        return [decode(x) for x in j]
    if type(j) is not dict:
        raise ValueError(f"typed_json: unexpected JSON value {j!r}")
    keys = set(j)
    if keys == {"i"}:
        return int(j["i"]) if len(j["i"]) <= 4000 else _parse_big_int(j["i"])
    if keys == {"f"}:
        return struct.unpack(">d", bytes.fromhex(j["f"]))[0]
    if keys == {"s"}:
        return _decode_str(j)
    if keys == {"b"}:
        return _decode_bytes(j)
    if keys == {"t"}:
        return tuple(decode(x) for x in j["t"])
    if keys == {"d"}:
        return {decode(k): decode(v) for k, v in j["d"]}
    if keys == {"lit"}:
        name = j["lit"]
        return LIT(_decode_bytes(name) if type(name) is dict and "b" in name else _decode_str(name))
    if keys == {"kw"}:
        return KWD(bytes.fromhex(j["kw"]))
    if keys == {"ref"}:
        objid = j["ref"]
        return PDFObjRef(None, objid if type(objid) is int else decode(objid))
    if keys == {"stream"}:
        s = j["stream"]
        attrs = decode(s["attrs"])
        stream = PDFStream(attrs, b"")
        stream.rawdata = decode(s["rawdata"])
        stream.data = decode(s["data"])
        stream.objid = decode(s["objid"])
        stream.genno = decode(s["genno"])
        return stream
    if keys == {"exc", "msg"}:
        return ExcValue(j["exc"], _decode_str(j["msg"]))
    if keys == {"named", "d"}:
        return Named(j["named"], {decode(k): decode(v) for k, v in j["d"]})
    raise ValueError(f"typed_json: unknown object {sorted(keys)}")


def _parse_big_int(text: str) -> int:
    """``int(text)`` past CPython's ``int_max_str_digits`` limit."""
    neg = text.startswith("-")
    digits = text[1:] if neg else text
    n = 0
    for i in range(0, len(digits), 1000):
        chunk = digits[i : i + 1000]
        n = n * 10 ** len(chunk) + int(chunk)
    return -n if neg else n


def dumps(v: Any) -> str:
    """Compact ASCII JSON text of ``encode(v)``."""
    return json.dumps(encode(v), ensure_ascii=True, separators=(",", ":"))


def loads(text: str) -> Any:
    return decode(json.loads(text))
