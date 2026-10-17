"""A small PDF writer for the adversarial generators.

It writes exactly the bytes it is told to, including broken ones: objects
are serialized from Python values (``Name``, ``Ref``, ``Raw`` for verbatim
syntax), the file is assembled section by section, and the cross-reference
sections (tables or streams, with any ``/W``, ``/Index`` and predictor) are
computed from the offsets actually written, then possibly falsified by the
caller. Encryption (RC4, AES-128, AES-256) applies the standard security
handler's algorithms to strings and stream data, so that pdfminer can open
the result with the right password.

Everything is deterministic: no clock, no randomness except the ``random``
instances the generators pass in.
"""

from __future__ import annotations

import hashlib
import struct
import zlib
from dataclasses import dataclass, field
from typing import Any, Callable

from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
from pdfminer.arcfour import Arcfour
from pdfminer.pdfdocument import PDFStandardSecurityHandlerV4, PDFStandardSecurityHandlerV5

PAD = b"(\xbfN^Nu\x8aAd\x00NV\xff\xfa\x01\x08..\x00\xb6\xd0h>\x80/\x0c\xa9\xfedSiz"


class Name(str):
    """A PDF name (``/Name``)."""


@dataclass(frozen=True)
class Ref:
    num: int
    gen: int = 0


class Raw(bytes):
    """Bytes written as they are, in place of a value."""


@dataclass
class Stream:
    dict: dict
    data: bytes


def _name(n: str) -> bytes:
    out = b"/"
    for c in n.encode("utf-8"):
        if c < 0x21 or c > 0x7E or c in b"#/%()<>[]{}":
            out += b"#%02x" % c
        else:
            out += bytes([c])
    return out


def _string(b: bytes) -> bytes:
    out = b"("
    for c in b:
        if c in b"()\\":
            out += b"\\" + bytes([c])
        elif c == 0x0D:
            out += b"\\r"
        else:
            out += bytes([c])
    return out + b")"


def _number(x: float) -> bytes:
    if x != x or x in (float("inf"), float("-inf")):
        raise ValueError("no PDF syntax for a non-finite float")
    text = repr(x)
    if "e" in text or "E" in text:
        text = f"{x:.20f}".rstrip("0")
    return text.encode()


def ser(v: Any, enc: Callable[[bytes], bytes] | None = None) -> bytes:
    """The PDF syntax of a value; ``enc`` encrypts strings (written in hex)."""
    if isinstance(v, Raw):
        return bytes(v)
    if v is None:
        return b"null"
    if v is True:
        return b"true"
    if v is False:
        return b"false"
    if isinstance(v, int):
        return str(v).encode()
    if isinstance(v, float):
        return _number(v)
    if isinstance(v, Name):
        return _name(v)
    if isinstance(v, (bytes, bytearray)):
        if enc is not None:
            return b"<" + enc(bytes(v)).hex().encode() + b">"
        return _string(bytes(v))
    if isinstance(v, Ref):
        return b"%d %d R" % (v.num, v.gen)
    if isinstance(v, (list, tuple)):
        return b"[" + b" ".join(ser(x, enc) for x in v) + b"]"
    if isinstance(v, dict):
        return b"<<" + b"".join(_name(k) + b" " + ser(x, enc) + b" " for k, x in v.items()) + b">>"
    raise TypeError(f"cannot serialize {v!r}")


# ---------------------------------------------------------------------------------- encryption


def _rc4(key: bytes, data: bytes) -> bytes:
    return Arcfour(key).encrypt(data)


def _aes_cbc(key: bytes, iv: bytes, data: bytes) -> bytes:
    pad = 16 - len(data) % 16
    data = data + bytes([pad]) * pad
    enc = Cipher(algorithms.AES(key), modes.CBC(iv)).encryptor()
    return iv + enc.update(data) + enc.finalize()


def _bare(cls: type, **attrs: Any) -> Any:
    h = cls.__new__(cls)
    h.__dict__.update(attrs)
    return h


def _make_o(owner: bytes, user: bytes, r: int, length: int) -> bytes:
    """Algorithm 3.3 (the /O entry)."""
    h = hashlib.md5((owner + PAD)[:32]).digest()
    if r >= 3:
        for _ in range(50):
            h = hashlib.md5(h).digest()
    n = 5 if r == 2 else length // 8
    key = h[:n]
    o = _rc4(key, (user + PAD)[:32])
    if r >= 3:
        for i in range(1, 20):
            o = _rc4(bytes(c ^ i for c in key), o)
    return o


@dataclass
class Encryption:
    """The standard security handler of a document: its ``/Encrypt``
    dictionary and the cipher of each object's strings and streams."""

    param: dict
    file_key: bytes
    method: str  # "rc4", "aes128", "aes256", "identity"
    rng: Any
    stream_method: str | None = None  # defaults to method

    def object_key(self, num: int, gen: int, aes: bool) -> bytes:
        key = self.file_key + struct.pack("<L", num)[:3] + struct.pack("<L", gen)[:2]
        if aes:
            key += b"sAlT"
        return hashlib.md5(key).digest()[: min(len(key), 16)]

    def cipher(self, num: int, gen: int, stream: bool) -> Callable[[bytes], bytes]:
        method = self.stream_method if (stream and self.stream_method) else self.method

        def encrypt(data: bytes) -> bytes:
            if method == "identity":
                return data
            if method == "rc4":
                return _rc4(self.object_key(num, gen, False), data)
            iv = bytes(self.rng.randrange(256) for _ in range(16))
            if method == "aes128":
                return _aes_cbc(self.object_key(num, gen, True), iv, data)
            return _aes_cbc(self.file_key, iv, data)

        return encrypt


def standard_encryption(
    rng: Any,
    *,
    r: int,
    user: str,
    owner: str,
    docid: bytes,
    p: int = -4,
    length: int = 128,
    cfm: str | None = None,
    encrypt_metadata: bool | None = None,
) -> Encryption:
    """A consistent ``/Encrypt`` for revision ``r`` (2, 3: RC4; 4: crypt
    filter ``cfm``: ``V2`` (RC4), ``AESV2`` or ``None`` for Identity; 5, 6:
    AES-256) with these passwords (latin-1 for r <= 4)."""
    if r in (2, 3, 4):
        v = {2: 1, 3: 2, 4: 4}[r]
        if r == 2:
            length = 40
        o = _make_o(owner.encode("latin1"), user.encode("latin1"), r, length)
        param: dict = {"Filter": Name("Standard"), "V": v, "R": r, "O": o, "U": b"", "P": p}
        if r != 2:
            param["Length"] = length
        method = "rc4"
        if r == 4:
            if cfm is None:
                param["StmF"] = Name("Identity")
                param["StrF"] = Name("Identity")
                method = "identity"
            else:
                param["CF"] = {"StdCF": {"CFM": Name(cfm), "AuthEvent": Name("DocOpen"), "Length": 16}}
                param["StmF"] = Name("StdCF")
                param["StrF"] = Name("StdCF")
                method = "aes128" if cfm == "AESV2" else "rc4"
            if encrypt_metadata is not None:
                param["EncryptMetadata"] = encrypt_metadata
        h = _bare(
            PDFStandardSecurityHandlerV4,
            r=r,
            o=o,
            p=p if p >= 0 else p + 2**32,
            docid=[docid],
            length=length,
            encrypt_metadata=bool(param.get("EncryptMetadata", True)),
        )
        file_key = h.compute_encryption_key(user.encode("latin1"))
        param["U"] = h.compute_u(file_key)
        return Encryption(param, file_key, method, rng)
    h = _bare(PDFStandardSecurityHandlerV5, r=r)
    file_key = bytes(rng.randrange(256) for _ in range(32))
    uv, uk, ov, ok = (bytes(rng.randrange(256) for _ in range(8)) for _ in range(4))
    ub, ob = h._normalize_password(user), h._normalize_password(owner)

    def aes_nopad(key: bytes, data: bytes) -> bytes:
        enc = Cipher(algorithms.AES(key), modes.CBC(b"\0" * 16)).encryptor()
        return enc.update(data) + enc.finalize()

    u = h._password_hash(ub, uv) + uv + uk
    ue = aes_nopad(h._password_hash(ub, uk), file_key)
    o = h._password_hash(ob, ov, u) + ov + ok
    oe = aes_nopad(h._password_hash(ob, ok, u), file_key)
    param = {
        "Filter": Name("Standard"), "V": 5, "R": r, "O": o, "U": u, "OE": oe, "UE": ue, "P": p, "Length": 256,
        "CF": {"StdCF": {"CFM": Name("AESV3"), "AuthEvent": Name("DocOpen"), "Length": 32}},
        "StmF": Name("StdCF"), "StrF": Name("StdCF"),
    }
    if encrypt_metadata is not None:
        param["EncryptMetadata"] = encrypt_metadata
    return Encryption(param, file_key, "aes256", rng)


# ---------------------------------------------------------------------------------- documents


@dataclass
class Obj:
    num: int
    value: Any
    gen: int = 0
    stream: bytes | None = None
    raw: bytes | None = None  # the whole "n g obj ... endobj" text, verbatim
    encrypt: bool = True


@dataclass
class Doc:
    """Objects, then written by ``write``."""

    objects: list[Obj] = field(default_factory=list)
    header: bytes = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n"
    eol: bytes = b"\n"

    def add(self, num: int, value: Any, *, gen: int = 0, stream: bytes | None = None, encrypt: bool = True) -> Ref:
        self.objects.append(Obj(num, value, gen, stream, encrypt=encrypt))
        return Ref(num, gen)

    def add_raw(self, num: int, raw: bytes, gen: int = 0) -> Ref:
        self.objects.append(Obj(num, None, gen, raw=raw))
        return Ref(num, gen)

    def body(self, obj: Obj, enc: Encryption | None) -> bytes:
        if obj.raw is not None:
            return obj.raw
        eol = self.eol
        senc = enc.cipher(obj.num, obj.gen, False) if (enc and obj.encrypt) else None
        value = obj.value
        if obj.stream is not None:
            data = obj.stream
            if enc and obj.encrypt:
                data = enc.cipher(obj.num, obj.gen, True)(data)
            value = dict(value)
            value.setdefault("Length", len(data))
            text = ser(value, senc) + eol + b"stream" + eol + data + eol + b"endstream"
        else:
            text = ser(value, senc)
        return b"%d %d obj" % (obj.num, obj.gen) + eol + text + eol + b"endobj" + eol

    def write_objects(self, enc: Encryption | None = None, start: int = 0) -> tuple[bytes, dict[int, tuple[int, int]]]:
        """The bodies of the objects and their offsets (from ``start``)."""
        out = b""
        offsets: dict[int, tuple[int, int]] = {}
        for obj in self.objects:
            offsets[obj.num] = (start + len(out), obj.gen)
            out += self.body(obj, enc)
        return out, offsets


def xref_table(offsets: dict[int, tuple[int, int]], size: int | None = None, eol: bytes = b"\n",
               sections: list[tuple[int, int]] | None = None) -> bytes:
    """A classic ``xref`` section (entries for 0..size-1, free where no
    object): ``xref`` line, subsection headers, 20-byte entries."""
    size = size if size is not None else max(offsets, default=0) + 1
    sections = sections or [(0, size)]
    # each entry is 20 bytes: a two-byte end of line
    entry_eol = eol if len(eol) == 2 else b" " + eol
    out = b"xref" + eol
    for start, count in sections:
        out += b"%d %d" % (start, count) + eol
        for num in range(start, start + count):
            if num in offsets:
                pos, gen = offsets[num]
                out += b"%010d %05d n" % (pos, gen) + entry_eol
            else:
                out += b"0000000000 65535 f" + entry_eol
    return out


def trailer(d: dict, eol: bytes = b"\n") -> bytes:
    return b"trailer" + eol + ser(d) + eol


def startxref(pos: int | bytes, eol: bytes = b"\n") -> bytes:
    value = pos if isinstance(pos, bytes) else str(pos).encode()
    return b"startxref" + eol + value + eol + b"%%EOF" + eol


def classic(doc: Doc, trailer_dict: dict, enc: Encryption | None = None, *, size: int | None = None) -> bytes:
    """A complete file with one xref table."""
    head = doc.header
    body, offsets = doc.write_objects(enc, len(head))
    xpos = len(head) + len(body)
    td = dict(trailer_dict)
    td.setdefault("Size", size if size is not None else max(offsets, default=0) + 1)
    return head + body + xref_table(offsets, td["Size"], doc.eol) + trailer(td, doc.eol) + startxref(xpos, doc.eol)


def png_up(rows: list[bytes]) -> bytes:
    """PNG "Up" predictor (type 2) applied to equal-length rows."""
    out = b""
    prev = bytes(len(rows[0])) if rows else b""
    for row in rows:
        out += b"\x02" + bytes((a - b) % 256 for a, b in zip(row, prev))
        prev = row
    return out


def xref_stream_data(entries: list[tuple[int, int, int]], w: tuple[int, int, int]) -> bytes:
    """The rows of an xref stream: each field big-endian on its width."""
    out = b""
    for fields in entries:
        for value, width in zip(fields, w):
            out += value.to_bytes(width, "big") if width else b""
    return out


def objstm(objects: list[tuple[Any, Any]], enc: Callable[[bytes], bytes] | None = None) -> tuple[bytes, int]:
    """The data of an object stream holding ``(id, value)`` pairs, and its
    ``/First``."""
    header = b""
    body = b""
    for objid, value in objects:
        header += ser(objid) + b" " + str(len(body)).encode() + b" "
        body += ser(value, enc) + b"\n"
    return header + body, len(header)


def flate(data: bytes) -> bytes:
    return zlib.compress(data, 9)
