"""Differential cases for crates/pdfminer/src/converter.rs (``pdfminer/converter.py``
``PDFLayoutAnalyzer``, ``PDFPageAggregator``) and pdfplumber/page.py:110
``PDFPageAggregatorWithMarkedContent`` (``crates/pdfplumber/src/page.rs``),
with the constructors of the raw layout objects (``crates/pdfminer/src/layout.rs``),
computed by pdfminer without its interpreter.

Writes typed JSON lines (docs/verification.md) to
``<cache>/unit/converter/<fingerprint>/<name>.jsonl`` (``oracle/unit/_common.py``);
the Rust replay is crates/pdftrace/tests/converter_unit.rs (natively and on
wasm32-wasip1).

A case is a sequence of device calls: those pdfminer's interpreter makes to
pdfplumber's device for one page (``process_page``), at nesting level 0 (the
calls the analyzer makes on itself, a subpath's ``paint_path``,
``handle_undefined_char``, ``receive_layout``, ``tag_cur_item``, belong to
the replayed code; ``render_string`` is ``PDFTextDevice``'s and is not
replayed, its ``render_char`` calls are), each with the recursion depth of
its caller. Python replays the sequence on a new
``PDFPageAggregatorWithMarkedContent(rsrcmgr, pageno=pageno, laparams=...)``
once per run (``laparams=None``, pdfplumber's default, then ``LAParams``
configurations), from a frame at the fixed depth ``REPLAY_DEPTH``
(``run_case``, the case's ``base``: the device is created and ``get_result``
called there), each call made from its recorded depth by plain Python calls
(``_call_at``, which take no C unit: every call has the C units left of the
base); the Rust replay does the same (``with_depth``).

Input ``[name, process, fonts, gstates, base, pageno, calls, runs]``:

- ``process``: ``{"re_compiled": bool}``, whether ``re`` has compiled
  ``m[^m]+`` (``PDFLayoutAnalyzer.paint_path``'s pattern) when the run starts
  (``re.purge()`` otherwise); the logger caches are cleared before each run
  (``logging.disable``), once the fonts are built;
- ``fonts``: the font of each ``render_char``, by the SHA-256 of its dump in
  ``<set>_fonts.jsonl`` (lines ``[sha, [spec, table, errors, cipher]]``, the
  cases of ``oracle/unit/fonts.py``: a font dictionary, the objects it reaches
  and the errors of those that do not load, the document's cipher), built by
  ``PDFResourceManager().get_font(None, spec)`` on the decoded dump;
- ``gstates``: the graphic states of the calls (``paint_path``'s live state,
  ``render_char``'s copy), each the list of its ``__dict__`` values
  (``GSTATE_KEYS``), a color space as ``[name, ncomponents]``;
- ``base``: ``{"py": .., "c_left": ..}``, the depth of the replay's frame;
- ``calls``: ``[kind, py, args...]``, ``py`` the Python frames of the
  caller (the call is made from there):

  - ``["set_ctm", py, ctm]``, ``["begin_page", py, mediabox, ctm]``,
    ``["end_page", py]``;
  - ``["begin_figure", py, name, bbox, matrix]`` (``bbox``, ``matrix`` as the
    interpreter passes them: a list, the PDF's, or a tuple),
    ``["end_figure", py, name]``, ``["render_image", py, name, stream]``
    (the stream's attributes, nested streams emptied, its data emptied: the
    layout does not read it); an inline image's name is ``inline{"n": k}``
    (``str(id(obj))`` upstream, a memory address: the Python replay names it
    ``str(id(stream))`` of its own stream, the Rust replay
    ``pdfinterp::inline_image_name(k)``);
  - ``["paint_path", py, gstate, stroke, fill, evenodd, path]``;
  - ``["render_char", py, matrix, font, fontsize, scaling, rise, cid, ncs,
    gstate]`` (``font``, ``gstate`` by index, ``ncs`` ``[name,
    ncomponents]``);
  - ``["begin_tag", py, tag, props]``, ``["end_tag", py]``, ``["do_tag", py,
    tag, props]`` (a dict ``props`` reduced to its ``MCID``, the only key
    read);

- ``runs``: ``[laparams, ids]``, ``laparams`` ``None`` or the ``LAParams``
  keyword arguments, ``ids`` Python's ``id()`` ranking of the boxes and
  groups of that run's analysis (``oracle/stages/layout.py`` ``id_ranks``,
  ``None`` without an analysis), which the Rust replay injects
  (``RecordedIds``) before ``end_page``.

Output: for each run, the canonical text of what it produced, through
``canon_or_hash`` (the text, or ``sha256:<hex>:<length>`` when it is longer
than ``INLINE_LIMIT``; ``--dump SET INDEX RUN`` prints it): ``adv <i>
<advance>`` for each ``render_char``, ``raised <i> <exception>`` for the call
that raised (the replay stops there: the interpreter never catches a
device's exception), then ``result <exception>`` when ``get_result`` raises,
else the lines of the ``layout`` stage (``oracle/stages/layout.py``) for the
result: the ``raw`` lines (``laparams=None``), or the ``analyzed``/``obj``/
``group`` lines (the raw objects numbered on the page as ``end_page``
received it); after an analysis, the ``ids`` line (also after one that
raised: the page is then the device's ``cur_item``).

Sets:

- ``adversarial`` (seconds): every shape of ``paint_path`` up to 4
  operators and random longer ones, subpaths split (``re.finditer``),
  rectangles in every winding and degenerate ones (a corner repeated, zero
  width, the closing ``l`` dropped or kept), points equal through ``-0.0`` or
  not through NaN, coordinates NaN, infinite, huge, ints (the interpreter
  makes floats; ints are what the type allows; a ``bool`` coordinate is not
  representable in ``utils::PathSegment``), under integer, float, rotated and
  degenerate CTMs, graphic states of every color shape and dash; figures
  nested, with every kind of ``/BBox`` and ``/Matrix`` item (ints, floats,
  bools, NaN, sequences, other objects, wrong lengths), images in them and
  outside (an ``AssertionError``), inline ones; chars of fonts of every kind
  (``oracle/unit/fonts.py`` ``Spec``: standard, simple, Type 3, CID
  horizontal and vertical, Type 0) and arguments (int and float sizes, NaN,
  rotated matrices, cids undefined, huge, beyond ``str()``'s digit limit);
  tags with props of every kind (dicts with an ``MCID`` of every type,
  without, names, lists, streams), names ``str``, ``bytes``, UTF-16 and
  beyond Latin-1, nested and unbalanced, tagging chars, paths, figures and
  the previous object when a path adds none; calls the interpreter never
  makes (``get_result`` before ``end_page``, ``end_page`` inside a figure).
- ``depth`` (seconds): for each call kind and each deepest path (the
  constructors of every object, a subpath split with the pattern compiled or
  not, an undefined char with the logger cache cold or warm, a vertical
  char, ``decode_text`` of each kind of tag, ``end_page``'s analysis, an
  exception raised before the deepest frame), the deepest caller depth at
  which the call does not raise ``RecursionError`` (binary search), and the
  cases around it.
- ``corpus`` (``--corpus``, minutes: ``scripts/heavy.sh``): the calls the
  interpreter makes on the first ``MAX_PAGES`` pages of every PDF of the set
  ``all``, recorded by a subclass of pdfplumber's device (``Recorder``) on
  pdfplumber's driving (one ``PDFResourceManager`` per document), their
  depths kept relative to the shallowest; the fonts copied as
  ``oracle/unit/fonts.py`` copies its corpus fonts (a second instance of an
  encrypted document, streams still encrypted) and pruned of the streams
  their construction does not read; runs ``None`` and ``LAParams()`` (and
  ``EXTRA_CONFIG`` on pages of at most ``SMALL`` raw objects); pages of more
  than ``MAX_CALLS`` calls are skipped (counted in ``corpus.json``, with the
  pages whose replayed raw page differs from the interpretation's: fonts the
  dump cannot carry exactly).

Usage: PDFPLUMBER_ROOT=<main checkout> <root>/.venv/bin/python -m oracle.unit.converter [--corpus] [-j N]
"""

from __future__ import annotations

import argparse
import collections
import hashlib
import json
import logging
import math
import multiprocessing
import os
import random
import re
import signal
import struct
import sys
import zlib
from typing import Any

from pdfminer.layout import LAParams, LTImage, LTPage
from pdfminer.pdfcolor import PDFColorSpace
from pdfminer.pdfinterp import PDFGraphicState, PDFPageInterpreter, PDFResourceManager
from pdfminer.pdfpage import PDFPage
from pdfminer.pdftypes import PDFObjRef, PDFStream
from pdfminer.psparser import KWD, LIT
from pdfplumber.page import PDFPageAggregatorWithMarkedContent

from ..canon import Named, canon
from ..codegen import cmaps as codegen_cmaps
from ..corpus import cache_dir, load_set
from ..stages._common import line, open_document
from ..stages.layout import _analyzed, _RawDump, id_ranks
from ..typed_json import decode as typed_decode
from ..typed_json import encode as typed_encode
from ._common import depth_here, oracle_sources, require_oracle_python, unit_dir, write_meta
from .fonts import (
    FontDoc,
    RecordingManager,
    Spec,
    bind,
    copy_graph,
    deep_recursion,
    document_cipher,
    dump_doc,
    load_doc,
    prune,
    roundtrip,
    streams_of,
)

MODULE = "converter"
#: Outputs longer than this are compared through their SHA-256.
INLINE_LIMIT = 20000
FILE_TIMEOUT_S = 600
MAX_PAGES = 20
#: Pages with more device calls are skipped.
MAX_CALLS = 60000
#: Pages of at most this many raw objects also get ``EXTRA_CONFIG``.
SMALL = 1500
EXTRA_CONFIG = {"all_texts": True, "detect_vertical": True, "boxes_flow": -0.5}
#: The Python frames of ``run_case``'s frame (the replay's base).
REPLAY_DEPTH = 60
#: ``PDFLayoutAnalyzer.paint_path``'s pattern.
SUBPATH_PATTERN = r"m[^m]+"
#: The order of ``PDFGraphicState.__dict__``.
GSTATE_KEYS = ["linewidth", "linecap", "linejoin", "miterlimit", "dash", "intent", "flatness", "scolor", "scs",
               "ncolor", "ncs"]
#: The code of the method that names inline images ``str(id(obj))``.
_DO_EI = PDFPageInterpreter.do_EI.__code__


def canon_or_hash(text: str) -> str:
    if len(text) <= INLINE_LIMIT:
        return text
    return f"sha256:{hashlib.sha256(text.encode('ascii')).hexdigest()}:{len(text)}"


def _py_depth(frame) -> int:
    n = 0
    while frame is not None:
        n += 1
        frame = frame.f_back
    return n


def encoded(v: Any) -> str:
    with deep_recursion():
        return json.dumps(typed_encode(v))


def decoded(text: str) -> Any:
    with deep_recursion():
        return typed_decode(json.loads(text))


# -- values of the calls --------------------------------------------------------


def _snapshot_obj(x: Any) -> Any:
    """A copy of a PDF object (lists, dicts, streams, references), its
    streams emptied of their data (attributes kept), iteratively (PDF values
    can nest deeply). A reference is kept by number."""
    if not isinstance(x, (list, dict, PDFStream, PDFObjRef)):
        return x
    root: list[Any] = [None]
    stack: list[tuple[Any, Any, Any]] = [(x, root, 0)]
    while stack:
        v, parent, key = stack.pop()
        if isinstance(v, list):
            out: Any = [None] * len(v)
            stack.extend((c, out, i) for i, c in enumerate(v))
        elif isinstance(v, dict):
            out = dict.fromkeys(v)
            stack.extend((c, out, k) for k, c in v.items())
        elif isinstance(v, PDFStream):
            out = PDFStream(dict.fromkeys(v.attrs), b"")
            out.objid, out.genno = v.objid, v.genno
            stack.extend((c, out.attrs, k) for k, c in v.attrs.items())
        elif isinstance(v, PDFObjRef):
            out = PDFObjRef(None, v.objid)
        else:
            out = v
        parent[key] = out
    return root[0]


def snapshot(x: Any) -> Any:
    """An operand as a case carries it: a PDF object copied
    (``_snapshot_obj``), a tuple of the interpreter item by item."""
    if isinstance(x, tuple):
        return tuple(snapshot(v) for v in x)
    return _snapshot_obj(x)


def props_value(props: Any) -> Any:
    """``begin_tag``'s props as the case carries them: a dict reduced to its
    ``MCID`` (the only key the device reads), anything else copied."""
    if isinstance(props, dict):
        return {"MCID": snapshot(props["MCID"])} if "MCID" in props else {}
    return snapshot(props)


def gstate_fields(gs_vars: dict) -> list:
    out = []
    for k in GSTATE_KEYS:
        v = gs_vars[k]
        out.append([v.name, snapshot(v.ncomponents)] if isinstance(v, PDFColorSpace) else snapshot(v))
    return out


class CaseBuilder:
    """Turns calls (as ``Recorder`` records them) into a case: tables of
    graphic states and fonts, inline image names."""

    def __init__(self, font_key) -> None:
        self.font_key = font_key  # font object -> SHA-256 of its dump
        self.gstates: list[list] = []
        self.gstate_index: dict[str, int] = {}
        self.fonts: list[str] = []
        self.font_index: dict[str, int] = {}
        self.inline: dict[str, int] = {}
        self.calls: list[list] = []

    def gstate(self, gs_vars: dict) -> int:
        fields = gstate_fields(gs_vars)
        key = canon(fields)
        if key not in self.gstate_index:
            self.gstate_index[key] = len(self.gstates)
            self.gstates.append(fields)
        return self.gstate_index[key]

    def font(self, font) -> int:
        sha = self.font_key(font)
        if sha not in self.font_index:
            self.font_index[sha] = len(self.fonts)
            self.fonts.append(sha)
        return self.font_index[sha]

    def name(self, name: str, inline: bool) -> Any:
        if not inline:
            return name
        if name not in self.inline:
            self.inline[name] = len(self.inline)
        return Named("inline", {"n": self.inline[name]})

    def add(self, kind: str, py: int, args: tuple) -> None:
        if kind == "set_ctm":
            self.calls.append([kind, py, snapshot(args[0])])
        elif kind == "begin_page":
            self.calls.append([kind, py, tuple(args[0]), snapshot(args[1])])
        elif kind in ("end_page", "end_tag"):
            self.calls.append([kind, py])
        elif kind == "begin_figure":
            name, inline, bbox, matrix = args
            self.calls.append([kind, py, self.name(name, inline), snapshot(bbox), snapshot(matrix)])
        elif kind == "end_figure":
            name, inline = args
            self.calls.append([kind, py, self.name(name, inline)])
        elif kind == "render_image":
            name, inline, stream = args
            self.calls.append([kind, py, self.name(name, inline), snapshot(stream)])
        elif kind == "paint_path":
            gs, stroke, fill, evenodd, path = args
            self.calls.append([kind, py, self.gstate(gs), stroke, fill, evenodd, [tuple(p) for p in path]])
        elif kind == "render_char":
            matrix, font, fontsize, scaling, rise, cid, ncs, gs = args
            self.calls.append([kind, py, snapshot(matrix), self.font(font), fontsize, scaling, rise, cid,
                               [ncs.name, snapshot(ncs.ncomponents)], self.gstate(gs)])
        elif kind in ("begin_tag", "do_tag"):
            tag, props = args
            self.calls.append([kind, py, tag, props_value(props)])
        else:  # pragma: no cover
            raise AssertionError(kind)


def _packable(c: list) -> bool:
    """Whether a ``render_char`` call can join a ``chars`` call: float
    ``e``, ``f`` and an int cid that fits an ``i64``."""
    matrix, cid = c[2], c[7]
    return (type(matrix[4]) is float and type(matrix[5]) is float and type(cid) is int
            and -2**63 <= cid < 2**63)


def _chars_key(c: list) -> str:
    return canon([c[1], c[2][:4], c[3], c[4], c[5], c[6], c[8], c[9]])


def compact(calls: list) -> list:
    """The calls with each run of ``render_char`` calls that differ only by
    ``e``, ``f`` and the cid (the chars of a string) as one ``["chars", py,
    (a, b, c, d), font, fontsize, scaling, rise, ncs, gstate, packed]``,
    ``packed`` the zlib-compressed little-endian columns of ``e`` (f64),
    ``f`` (f64) and the cids (i64): the corpus's chars take a tenth of their
    typed JSON. A ``paint_path`` call whose coordinates are all floats is
    ``["path", py, gstate, stroke, fill, evenodd, ops, packed]``, ``ops``
    the operators, ``packed`` the compressed coordinates (f64). ``expand``
    restores the calls (the outputs number them)."""
    out: list = []
    run: list = []

    def flush():
        if len(run) == 1:
            out.append(run[0])
        elif run:
            first = run[0]
            n = len(run)
            packed = struct.pack(f"<{n}d", *(c[2][4] for c in run)) + struct.pack(f"<{n}d", *(c[2][5] for c in run)) \
                + struct.pack(f"<{n}q", *(c[7] for c in run))
            out.append(["chars", first[1], tuple(first[2][:4]), first[3], first[4], first[5], first[6], first[8],
                        first[9], zlib.compress(packed, 9)])
        run.clear()

    for c in calls:
        if c[0] == "render_char" and _packable(c) and (not run or _chars_key(run[0]) == _chars_key(c)):
            run.append(c)
            continue
        flush()
        if c[0] == "render_char" and _packable(c):
            run.append(c)
        elif c[0] == "paint_path" and all(seg[0] in _NCOORDS and len(seg) == 1 + _NCOORDS[seg[0]]
                                          and all(type(x) is float for x in seg[1:]) for seg in c[6]):
            coords = [x for seg in c[6] for x in seg[1:]]
            ops = "".join(seg[0] for seg in c[6])
            out.append(["path", *c[1:6], ops, zlib.compress(struct.pack(f"<{len(coords)}d", *coords), 9)])
        else:
            out.append(c)
    flush()
    return out


#: The coordinates of each path operator.
_NCOORDS = {"m": 2, "l": 2, "c": 6, "v": 4, "y": 4, "h": 0}


def expand(calls: list) -> list:
    """The calls of a case, ``chars`` calls expanded (``compact``)."""
    out: list = []
    for c in calls:
        if c[0] == "path":
            _, py, gs, stroke, fill, evenodd, ops, packed = c
            data = zlib.decompress(packed)
            coords = struct.unpack(f"<{len(data) // 8}d", data)
            path, k = [], 0
            for op in ops:
                n = _NCOORDS[op]
                path.append((op, *coords[k:k + n]))
                k += n
            out.append(["paint_path", py, gs, stroke, fill, evenodd, path])
            continue
        if c[0] != "chars":
            out.append(c)
            continue
        _, py, abcd, font, fontsize, scaling, rise, ncs, gs, packed = c
        data = zlib.decompress(packed)
        n = len(data) // 24
        es = struct.unpack_from(f"<{n}d", data, 0)
        fs = struct.unpack_from(f"<{n}d", data, 8 * n)
        cids = struct.unpack_from(f"<{n}q", data, 16 * n)
        for e, f, cid in zip(es, fs, cids, strict=True):
            out.append(["render_char", py, (*abcd, e, f), font, fontsize, scaling, rise, cid, ncs, gs])
    return out


#: Calls whose typed JSON is longer are written compressed.
PACK_CALLS = 4000


def pack_calls(case: list) -> list:
    """The case as written: its calls, when long, as the zlib-compressed
    text of their typed JSON (``bytes``)."""
    text = encoded(case[6])
    if len(text) <= PACK_CALLS:
        return case
    return [*case[:6], zlib.compress(text.encode("ascii"), 9), case[7]]


def unpack_calls(calls: Any) -> list:
    """The calls of a written case (``pack_calls``)."""
    if isinstance(calls, bytes):
        return decoded(zlib.decompress(calls).decode("ascii"))
    return calls


def new_case(name: str, builder: CaseBuilder, pageno: int, configs: list, re_compiled: bool = True) -> list:
    """A case whose calls' depths are relative (``place_depths``) and whose
    base and ids ``finish`` fills in."""
    return [name, {"re_compiled": re_compiled}, builder.fonts, builder.gstates, None, pageno, compact(builder.calls),
            [[c, None] for c in configs]]


def place_depths(case: list) -> list:
    """The calls' depths made absolute: the shallowest is the replay's
    first level (``REPLAY_DEPTH + 1``), the others keep their offsets."""
    calls = case[6]
    if calls:
        low = min(c[1] for c in calls)
        for c in calls:
            c[1] = REPLAY_DEPTH + 1 + c[1] - low
    return case


# -- the Python replay ------------------------------------------------------------


class _Page:
    """What ``begin_page`` reads of a ``PDFPage`` (``end_page`` reads
    nothing)."""

    def __init__(self, mediabox) -> None:
        self.mediabox = mediabox


def _call_at(extra: int, f, args):
    """``f(*args)`` called from the frame ``extra + 1`` frames below this
    one's caller, as a plain call with positional arguments (the
    interpreter calls the device's methods so; ``f(*args)`` would be a call
    from C, ``CALL_FUNCTION_EX``). The recursion takes no C unit."""
    if extra:
        return _call_at(extra - 1, f, args)
    n = len(args)
    if n == 0:
        return f()
    if n == 1:
        return f(args[0])
    if n == 2:
        return f(args[0], args[1])
    if n == 3:
        return f(args[0], args[1], args[2])
    if n == 5:
        return f(args[0], args[1], args[2], args[3], args[4])
    if n == 8:
        return f(args[0], args[1], args[2], args[3], args[4], args[5], args[6], args[7])
    raise AssertionError(n)  # pragma: no cover


#: The fonts built from their dumps, by SHA-256 (per process).
_FONTS: dict[str, Any] = {}


def build_font(sha: str, dump: list) -> Any:
    """The font of a dump (or the exception of its construction), built
    once per process: a font has no state its queries change."""
    font = _FONTS.get(sha)
    if font is None:
        spec_j, table_j, errors_j, cipher_j = dump
        doc = load_doc(table_j, errors_j, cipher_j)
        try:
            font = PDFResourceManager().get_font(None, bind(spec_j, doc))
        except Exception as e:  # the case cannot be replayed
            font = e
        _FONTS[sha] = font
    return font


def set_process(process: dict) -> None:
    """The state of the process a run starts from: ``re``'s caches (only
    ``m[^m]+`` matters) and the logger caches (cleared)."""
    re.purge()
    if process["re_compiled"]:
        re.compile(SUBPATH_PATTERN)
    logging.disable(logging.CRITICAL)


class Replay:
    """The Python objects of a case, decoded from its typed JSON."""

    def __init__(self, case: list, fonts_table: dict[str, list]) -> None:
        _name, self.process, font_shas, gstates, _base, self.pageno, calls, _runs = case
        calls = unpack_calls(calls)
        self.fonts = [build_font(sha, fonts_table[sha]) for sha in font_shas]
        self.colorspaces: dict[str, PDFColorSpace] = {}
        self.gstates = [self._gstate(fields) for fields in gstates]
        self.inline_streams: dict[int, PDFStream] = {}
        for c in expand(calls):
            if c[0] == "render_image" and isinstance(c[2], Named):
                self.inline_streams.setdefault(c[2].fields["n"], c[3])
        self.calls = [self._call(c) for c in expand(calls)]

    def _colorspace(self, v: list) -> PDFColorSpace:
        key = canon(v)
        cs = self.colorspaces.get(key)
        if cs is None:
            cs = self.colorspaces[key] = PDFColorSpace(v[0], v[1])
        return cs

    def _gstate(self, fields: list) -> PDFGraphicState:
        gs = PDFGraphicState()
        for k, v in zip(GSTATE_KEYS, fields, strict=True):
            setattr(gs, k, self._colorspace(v) if k in ("scs", "ncs") else v)
        return gs

    def _name(self, name: Any) -> str:
        """An inline image's name: ``str(id(obj))`` of its stream, as
        ``do_EI`` names it."""
        if not isinstance(name, Named):
            return name
        n = name.fields["n"]
        if n not in self.inline_streams:
            self.inline_streams[n] = PDFStream({}, b"")
        return str(id(self.inline_streams[n]))

    def _call(self, c: list) -> tuple[str, int, tuple]:
        kind, py = c[0], c[1]
        if kind == "set_ctm":
            return kind, py, (c[2],)
        if kind == "begin_page":
            return kind, py, (_Page(c[2]), c[3])
        if kind == "end_page":
            return kind, py, (_Page((0.0, 0.0, 0.0, 0.0)),)
        if kind == "end_tag":
            return kind, py, ()
        if kind == "begin_figure":
            return kind, py, (self._name(c[2]), c[3], c[4])
        if kind == "end_figure":
            return kind, py, (self._name(c[2]),)
        if kind == "render_image":
            stream = self.inline_streams[c[2].fields["n"]] if isinstance(c[2], Named) else c[3]
            return kind, py, (self._name(c[2]), stream)
        if kind == "paint_path":
            return kind, py, (self.gstates[c[2]], c[3], c[4], c[5], c[6])
        if kind == "render_char":
            _, _, matrix, font, fontsize, scaling, rise, cid, ncs, gs = c
            return kind, py, (matrix, self.fonts[font], fontsize, scaling, rise, cid, self._colorspace(ncs),
                              self.gstates[gs])
        if kind in ("begin_tag", "do_tag"):
            return kind, py, (c[2], c[3])
        raise AssertionError(kind)  # pragma: no cover


_RSRCMGR = PDFResourceManager()


def run_case(replay: Replay, laparams: dict | None) -> tuple[str, list | None, dict]:
    """One run, from ``REPLAY_DEPTH``: the output text, the ids ranking of
    its analysis and the depth of this frame (the case's ``base``)."""
    set_process(replay.process)
    base = depth_here(1)
    if base["py"] != REPLAY_DEPTH:  # pragma: no cover
        raise AssertionError(f"run_case at depth {base['py']}")
    lp = None if laparams is None else LAParams(**laparams)
    device = PDFPageAggregatorWithMarkedContent(_RSRCMGR, pageno=replay.pageno, laparams=lp)
    out: list[str] = []
    raw = None
    for i, (kind, py, args) in enumerate(replay.calls):
        if kind == "end_page" and lp is not None and not device._stack \
                and isinstance(getattr(device, "cur_item", None), LTPage):
            raw = _RawDump(device.cur_item)
        try:
            result = _call_at(py - REPLAY_DEPTH - 1, getattr(device, kind), args)
        except Exception as e:
            out.append(line("raised", i, e))
            break
        if kind == "render_char":
            out.append(line("adv", i, result))
    ids = None
    try:
        page = device.get_result()
    except Exception as e:
        out.append(line("result", e))
        page = None
    if lp is None:
        if page is not None:
            out.extend(_RawDump(page).write(page))
    elif raw is not None:
        if page is not None:
            out.extend(_analyzed(page, raw))
        else:
            page = device.cur_item
        ids = id_ranks(page, raw.index)
        out.append(line("ids", ids))
    return "\n".join(out) + "\n", ids, base


def run_at_base(replay: Replay, laparams: dict | None) -> tuple[str, list | None, dict]:
    """``run_case`` called from below enough plain frames for its own frame
    to be at ``REPLAY_DEPTH``."""
    return _pad(REPLAY_DEPTH - _py_depth(sys._getframe()) - 2, replay, laparams)


def _pad(n: int, replay: Replay, laparams: dict | None):
    if n:
        return _pad(n - 1, replay, laparams)
    return run_case(replay, laparams)


def finish(case: list, fonts_table: dict[str, list]) -> tuple[list, list[str], list[str]] | None:
    """``(input, outputs, texts)`` of a case, its runs' ids and its base
    filled in by the Python replay of the case as Rust decodes it, or None
    when one of its fonts cannot be built from its dump."""
    case = decoded(encoded(case))
    replay = Replay(case, fonts_table)
    if any(isinstance(f, Exception) for f in replay.fonts):
        return None
    runs, texts, bases = [], [], []
    for laparams, _ in case[7]:
        text, ids, base = run_at_base(replay, laparams)
        runs.append([laparams, ids])
        texts.append(text)
        bases.append(base)
    if any(b != bases[0] for b in bases):  # pragma: no cover
        raise AssertionError(f"the runs of a case start from different depths: {bases}")
    return [*case[:4], bases[0], *case[5:7], runs], [canon_or_hash(t) for t in texts], texts


def replay_text(case: list, fonts_table: dict[str, list], run: int) -> str:
    """The output text of one run of a finished case."""
    return run_at_base(Replay(case, fonts_table), case[7][run][0])[0]


# -- recording the interpreter's calls ----------------------------------------------


class Recorder(PDFPageAggregatorWithMarkedContent):
    """pdfplumber's device, recording the calls the interpreter makes to it
    (not those the analyzer makes on itself: ``level``) with the depth of
    their caller. Each override is one frame more than pdfplumber runs,
    below the recorded depth: the replay makes the call from that depth
    without it."""

    def __init__(self, *args, **kwargs) -> None:
        super().__init__(*args, **kwargs)
        self.calls: list[tuple[str, int, tuple]] = []
        self.level = 0

    def _enter(self, kind: str, args: tuple) -> None:
        if not self.level:
            self.calls.append((kind, _py_depth(sys._getframe(2)), args))

    def set_ctm(self, ctm):
        self._enter("set_ctm", (ctm,))
        self.level += 1
        try:
            super().set_ctm(ctm)
        finally:
            self.level -= 1

    def begin_page(self, page, ctm):
        self._enter("begin_page", (page.mediabox, ctm))
        self.level += 1
        try:
            super().begin_page(page, ctm)
        finally:
            self.level -= 1

    def end_page(self, page):
        self._enter("end_page", ())
        self.level += 1
        try:
            super().end_page(page)
        finally:
            self.level -= 1

    def begin_figure(self, name, bbox, matrix):
        self._enter("begin_figure", (name, sys._getframe(1).f_code is _DO_EI, bbox, matrix))
        self.level += 1
        try:
            super().begin_figure(name, bbox, matrix)
        finally:
            self.level -= 1

    def end_figure(self, name):
        self._enter("end_figure", (name, sys._getframe(1).f_code is _DO_EI))
        self.level += 1
        try:
            super().end_figure(name)
        finally:
            self.level -= 1

    def render_image(self, name, stream):
        self._enter("render_image", (name, sys._getframe(1).f_code is _DO_EI, stream))
        self.level += 1
        try:
            super().render_image(name, stream)
        finally:
            self.level -= 1

    def paint_path(self, graphicstate, stroke, fill, evenodd, path):
        self._enter("paint_path", (dict(vars(graphicstate)), stroke, fill, evenodd, list(path)))
        self.level += 1
        try:
            super().paint_path(graphicstate, stroke, fill, evenodd, path)
        finally:
            self.level -= 1

    def render_char(self, matrix, font, fontsize, scaling, rise, cid, ncs, graphicstate):
        self._enter("render_char", (matrix, font, fontsize, scaling, rise, cid, ncs, dict(vars(graphicstate))))
        self.level += 1
        try:
            return super().render_char(matrix, font, fontsize, scaling, rise, cid, ncs, graphicstate)
        finally:
            self.level -= 1

    def begin_tag(self, tag, props=None):
        self._enter("begin_tag", (tag, props))
        self.level += 1
        try:
            super().begin_tag(tag, props)
        finally:
            self.level -= 1

    def end_tag(self):
        self._enter("end_tag", ())
        self.level += 1
        try:
            super().end_tag()
        finally:
            self.level -= 1

    def do_tag(self, tag, props=None):
        self._enter("do_tag", (tag, props))
        self.level += 1
        try:
            super().do_tag(tag, props)
        finally:
            self.level -= 1


def font_dump(spec, real_doc, cipher) -> list:
    """``[spec, table, errors, cipher]`` of a document's font, as typed JSON
    decodes it, the streams its construction does not read emptied
    (``oracle/unit/fonts.py`` ``corpus_font_case``)."""
    doc = FontDoc()
    doc.cipher = cipher
    spec_copy = copy_graph(spec, real_doc, doc)
    table, errors = dump_doc(doc)
    spec_j, table_j, errors_j, cipher_j = roundtrip(spec_copy, table, errors, cipher)
    probe_doc = load_doc(table_j, errors_j, cipher_j)
    probe_spec = bind(spec_j, probe_doc)
    try:
        PDFResourceManager().get_font(None, probe_spec)
    except Exception:  # the dump is kept whole
        pass
    read = {k for k, s in enumerate(streams_of(probe_spec, probe_doc)) if s.data is not None and s.rawdata is None}
    pruned_doc = load_doc(table_j, errors_j, cipher_j)
    pruned_spec = bind(spec_j, pruned_doc)
    prune(pruned_spec, pruned_doc, read)
    table_p, errors_p = dump_doc(pruned_doc)
    return list(roundtrip(pruned_spec, table_p, errors_p, cipher))


class FileFonts:
    """The dumps of the fonts of one document, by font object (the spec the
    interpreter gave ``PDFResourceManager.get_font``, ``RecordingManager``),
    copied from a second instance of an encrypted document."""

    def __init__(self, rsrcmgr: RecordingManager, doc, fresh, cipher) -> None:
        self.rsrcmgr = rsrcmgr
        self.doc = fresh if cipher is not None else doc
        self.cipher = cipher
        self.dumps: dict[int, str] = {}
        self.table: dict[str, list] = {}

    def key(self, font) -> str:
        sha = self.dumps.get(id(font))
        if sha is None:
            spec = next(s for s, f in self.rsrcmgr.fonts if f is font)
            dump = font_dump(spec, self.doc, self.cipher)
            sha = hashlib.sha256(encoded(dump).encode()).hexdigest()
            self.dumps[id(font)] = sha
            self.table[sha] = dump
        return sha


def emptied_dump(page) -> list[str]:
    """The ``raw`` lines of the interpretation's page with its images'
    streams emptied as the cases carry them (their data swapped out for
    the dump, then back: the objects keep their identity, which names
    inline images)."""
    saved = []
    stack = [obj.stream for obj, _ in _RawDump(page).objects if isinstance(obj, LTImage)]
    while stack:
        v = stack.pop()
        if isinstance(v, PDFStream):
            saved.append((v, v.rawdata, v.data))
            v.rawdata, v.data = b"", None
            stack.extend(v.attrs.values())
        elif isinstance(v, list):
            stack.extend(v)
        elif isinstance(v, dict):
            stack.extend(v.values())
    try:
        return _RawDump(page).write(page)
    finally:
        for s, rawdata, data in reversed(saved):
            s.rawdata, s.data = rawdata, data


class Timeout(Exception):
    pass


def on_alarm(signum, frame):
    raise Timeout()


def collect_file(args):
    """The cases of one document's pages, its font dumps and statistics."""
    path, rel, password = args
    signal.signal(signal.SIGALRM, on_alarm)
    signal.alarm(FILE_TIMEOUT_S)
    stats: collections.Counter = collections.Counter()
    out: list[tuple[str, str]] = []
    fonts_out: dict[str, str] = {}
    try:
        with open_document(path, password) as doc, open_document(path, password) as fresh:
            cipher = document_cipher(doc, password)
            rsrcmgr = RecordingManager()
            fonts = FileFonts(rsrcmgr, doc, fresh, cipher)
            for pageno, page in enumerate(PDFPage.create_pages(doc), start=1):
                if pageno > MAX_PAGES:
                    break
                stats["pages"] += 1
                recorder = Recorder(rsrcmgr, pageno=pageno, laparams=None)
                interpreter = PDFPageInterpreter(rsrcmgr, recorder)
                try:
                    interpreter.process_page(page)
                except Exception:  # recorded: the calls up to the one that raised
                    pass
                if len(recorder.calls) > MAX_CALLS:
                    stats["skipped:calls"] += 1
                    continue
                result = recorder.result
                nobjs = len(_RawDump(result).objects) if result is not None else 0
                try:
                    builder = CaseBuilder(fonts.key)
                    for kind, py, call_args in recorder.calls:
                        builder.add(kind, py, call_args)
                except Timeout:
                    raise
                except Exception as e:  # a harness limitation, counted
                    stats[f"skipped:{type(e).__name__}"] += 1
                    continue
                configs = [None, {}] + ([EXTRA_CONFIG] if nobjs <= SMALL else [])
                case = place_depths(new_case(f"{rel}#{pageno}", builder, pageno, configs))
                done = finish(case, fonts.table)
                if done is None:
                    stats["skipped:font"] += 1
                    continue
                args_c, outs, texts = done
                if result is not None:
                    replayed = [t for t in texts[0].splitlines() if not t.startswith("adv ")]
                    stats["replay_differs"] += replayed != emptied_dump(result)
                out.append((encoded(pack_calls(args_c)), encoded(outs)))
                for sha in args_c[2]:
                    if sha not in fonts_out:
                        fonts_out[sha] = encoded([sha, fonts.table[sha]])
                stats["cases"] += 1
    except Timeout:
        stats["timeout"] += 1
    except Exception as e:  # an unreadable document
        stats[f"file:{type(e).__name__}"] += 1
    finally:
        signal.alarm(0)
    return rel, out, fonts_out, stats


def write_jsonl(name: str, lines) -> int:
    path = unit_dir(MODULE) / f"{name}.jsonl"
    tmp = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    n = 0
    with open(tmp, "w", encoding="ascii") as f:
        for args, out in lines:
            f.write('{"in":' + args + ',"out":' + out + "}\n")
            n += 1
    tmp.replace(path)
    return n


def corpus_set(jobs: int) -> dict:
    files = load_set("all")
    tasks = [(str(f.path), f.rel, f.password) for f in files]
    fonts: dict[str, str] = {}
    stats: collections.Counter = collections.Counter()

    def cases():
        with multiprocessing.Pool(jobs, maxtasksperchild=10) as pool:
            for _rel, out, file_fonts, file_stats in pool.imap(collect_file, tasks, chunksize=1):
                fonts.update(file_fonts)
                stats.update(file_stats)
                yield from out

    n = write_jsonl("corpus", cases())
    write_jsonl("corpus_fonts", ((text, "null") for _, text in sorted(fonts.items())))
    print(f"corpus: {n} pages, {len(fonts)} fonts from {len(files)} files; {dict(stats)}", file=sys.stderr)
    return {"cases": n, "fonts": len(fonts), "stats": dict(stats)}


# -- generated sequences --------------------------------------------------------------


class FontRef:
    """A generated font: the SHA-256 of its dump (the builders' font key)."""

    def __init__(self, sha: str) -> None:
        self.sha = sha


def font_ref(spec, doc: FontDoc, table: dict[str, list]) -> FontRef:
    """The dump of a generated font dictionary, put in ``table``."""
    t, e = dump_doc(doc)
    dump = list(roundtrip(spec, t, e, doc.cipher))
    sha = hashlib.sha256(encoded(dump).encode()).hexdigest()
    table[sha] = dump
    return FontRef(sha)


def generated_fonts(rng, table: dict[str, list]) -> dict[str, list[FontRef]]:
    """Fonts of every kind, built from ``Spec`` (only those that build), by
    kind."""
    kinds: dict[str, list[FontRef]] = {}

    def add(kind, make):
        found = kinds.setdefault(kind, [])
        for _ in range(60):
            doc = FontDoc()
            spec = make(Spec(rng, doc))
            ref = font_ref(spec, doc, table)
            if not isinstance(build_font(ref.sha, table[ref.sha]), Exception):
                found.append(ref)
            if len(found) >= 8:
                break

    add("standard", lambda s: {"Type": LIT("Font"), "Subtype": LIT("Type1"),
                               "BaseFont": LIT(rng.choice(["Helvetica", "Times-Roman", "Courier", "Symbol"]))})
    add("simple", lambda s: s.simple(LIT(rng.choice(["Type1", "TrueType"]))))
    add("type3", lambda s: s.type3())
    add("cid", lambda s: s.cid(encoding="Identity-H"))
    add("vertical", lambda s: s.cid(encoding="Identity-V"))
    add("type0", lambda s: s.type0())
    add("any", lambda s: s.any())
    return {k: v for k, v in kinds.items() if v}


#: The SHA-256 of the generated fonts whose chars ``good_font`` builds.
_GOOD: dict[str, bool] = {}


def good_font(ref: FontRef) -> bool:
    """Whether ``render_char`` of the codes 65 and 0x4E00 does not raise
    (the mixed cases use such fonts, so that they go on to their end)."""
    if ref.sha not in _GOOD:
        device = PDFPageAggregatorWithMarkedContent(_RSRCMGR, pageno=1, laparams=None)
        device.begin_page(_Page((0.0, 0.0, 612.0, 792.0)), (1, 0, 0, 1, 0, 0))
        ok = True
        for cid in (65, 0x4E00):
            try:
                device.render_char((10.0, 0.0, 0.0, 10.0, 5.0, 5.0), _FONTS[ref.sha], 1.0, 1.0, 0, cid,
                                   PDFColorSpace("DeviceGray", 1), PDFGraphicState())
            except Exception:
                ok = False
        _GOOD[ref.sha] = ok
    return _GOOD[ref.sha]


#: Coordinates that raise nothing (ints, which the interpreter never
#: passes, become floats; huge floats overflow to inf).
SPECIALS = [math.nan, math.inf, -math.inf, -0.0, 0.0, 1e308, -1e308, 5e-324, 2**63, 10**30, 0, 1, -1, 3.5]
#: An int too large for ``float()`` (``OverflowError``).
HUGE = 10**400
GRID = [0.0, 1.0, 2.0, -0.0, 10.5, 1.0, 2.0]
CTMS = [(1, 0, 0, 1, 0, 0), (1.0, 0.0, 0.0, 1.0, 0.0, 0.0), (0, -1, 1, 0, -0.0, 612.0), (-1, 0, 0, -1, 612.0, 792.0),
        (0, 1, -1, 0, 792.0, 0), (2.0, 0.5, -0.5, 2.0, 10.0, 20.0), (0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
        (1.0, 0, 0, -1.0, 0, 792.0), (math.nan, 0, 0, 1, 0, 0), (1e308, 0.0, 0.0, 1e308, 0.0, 0.0),
        (0.1, 0.2, 0.3, 0.4, 0.5, 0.6)]
MEDIABOXES = [(0.0, 0.0, 612.0, 792.0), (-100.5, 50.0, 300.0, -20.0), (0.0, 0.0, 0.0, 0.0), (math.nan, 0.0, 1.0, 1.0),
              (0.0, 0.0, math.inf, 1e308)]
COLORSPACES = [["DeviceGray", 1], ["DeviceRGB", 3], ["DeviceCMYK", 4], ["Pattern", 1], ["ICCBased", PDFObjRef(None, 7)],
               ["ICCBased", 3], ["DeviceN", 2], ["Indexed", 1]]
COLORS = [0, 0.5, math.nan, (1.0, 0.0, 0.0), (0.0, 0.1, 0.2, 0.3), "P1", (0.5, "P2"), ((1.0, 0.0, 0.0), "P3"),
          ((0.0, 0.1, 0.2, 0.3), "P4")]
DASHES = [None, ([], 0), ([3, 2], 1.5), (LIT("x"), None), ([1], 0), ([0.5, math.nan], -1)]
STYLE_VALUES = [None, 0, 1, 2.5, LIT("x"), b"x", [1, 2], True]
OPS = "mlcvyh"
NCOORDS = {"m": 2, "l": 2, "c": 6, "v": 4, "y": 4, "h": 0}
#: Items of an unchecked /Matrix or /BBox (oracle/unit/layout.py RAW_NUMS and others).
RAW_ITEMS = [0, 1, -1, 2, 7, 1000, 2**62, 2**63, 10**30, 0.0, -0.0, 0.5, 72.0, 1e308, math.inf, -math.inf, math.nan,
             True, False]
RAW_ODD = [b"", b"ab", [], [1], [LIT("a"), 2.5], None, {"a": 1}, LIT("x"), KWD(b"q"), PDFObjRef(None, 3)]
IMAGE_ATTRS = [{"W": 5, "H": 7}, {"Width": 5.5, "Height": LIT("x"), "W": 1}, {"IM": True, "BPC": 8},
               {"ImageMask": False, "BitsPerComponent": 1, "CS": LIT("DeviceGray")},
               {"ColorSpace": [LIT("Indexed"), LIT("DeviceRGB"), 255, b"x"]}, {"CS": [], "ColorSpace": 3},
               {"W": PDFObjRef(None, 4), "H": 2, "SMask": PDFStream({"W": 1}, b"xyz")}, {}]
TAGS = [LIT("P"), LIT("Span"), LIT("Artifact"), LIT(""), LIT("caf\xe9"), LIT("ĉx"), LIT(b"\xfe\xff\x00A\x00B"),
        LIT(b"\xe9t\xe9"), LIT(b"\xfe\xff\x00"), LIT(b"")]
#: ``MCID`` values: any PDF object (a ``str`` is not one).
MCIDS = [None, 0, 1, 7, -3, 2**70, True, 1.5, math.nan, b"x", LIT("Artifact"), [1, 2], {"MCID": 3},
         PDFObjRef(None, 9), PDFStream({"a": 1}, b"")]
PROPS = [None, {}, {"Foo": 1}, LIT("MC0"), [1, 2], PDFStream({"MCID": 1}, b""), 3] + [{"MCID": v} for v in MCIDS]
CIDS = [0, 1, 32, 65, 97, 255, 256, 0x4E00, 0xD800, 0x10FFFF, 0x110000, -1, 2**64, 10**4299, 10**4300]


class Gen:
    """Builds generated calls, ``py`` relative (0: the replay's first
    level; ``place_depths`` makes them absolute)."""

    def __init__(self, rng, fonts: dict[str, list[FontRef]]) -> None:
        self.rng = rng
        self.fonts = fonts
        self.builder = CaseBuilder(lambda f: f.sha)
        self.inline = 0

    def call(self, kind, *args, py=0):
        self.builder.add(kind, py, args)

    def gstate(self) -> dict:
        rng = self.rng
        gs = dict(vars(PDFGraphicState()))
        gs["linewidth"] = rng.choice([0, 0.5, 2.0, math.nan, 1e308, -1.0])
        for k in ("linecap", "linejoin", "miterlimit", "intent", "flatness"):
            gs[k] = rng.choice(STYLE_VALUES)
        gs["dash"] = rng.choice(DASHES)
        gs["scolor"], gs["ncolor"] = rng.choice(COLORS), rng.choice(COLORS)
        gs["scs"] = PDFColorSpace(*rng.choice(COLORSPACES))
        gs["ncs"] = PDFColorSpace(*rng.choice(COLORSPACES))
        return gs

    def coord(self, special=0.1):
        rng = self.rng
        if rng.random() < special:
            return rng.choice(SPECIALS)
        return rng.choice(GRID) if rng.random() < 0.6 else round(rng.uniform(-50, 600), 2)

    def path(self, shape, special=0.1):
        return [(op, *(self.coord(special) for _ in range(NCOORDS[op]))) for op in shape]

    def paint(self, path, py=0):
        rng = self.rng
        self.call("paint_path", self.gstate(), rng.random() < 0.6, rng.random() < 0.5, rng.random() < 0.3, path, py=py)

    def begin(self, ctm=None, mediabox=None):
        rng = self.rng
        ctm = ctm if ctm is not None else rng.choice(CTMS[:8])
        self.call("begin_page", mediabox or rng.choice(MEDIABOXES[:2]), ctm)
        self.call("set_ctm", ctm)

    def char(self, kind=None, cid=None, py=0, **kw):
        rng = self.rng
        if kind is None:
            good = [f for fonts in self.fonts.values() for f in fonts if good_font(f)]
            font = rng.choice(good)
        else:
            font = rng.choice(self.fonts[kind])
        matrix = kw.get("matrix") or (rng.choice([10.0, 12.0, -10.0, 0.0]), rng.choice([0.0, 0, 0.5]),
                                      rng.choice([0.0, 0, -0.5]), rng.choice([10.0, 12.0, -1.0]),
                                      self.coord(0.05), self.coord(0.05))
        if cid is None:
            cid = rng.choice(CIDS[:-1]) if rng.random() < 0.2 else rng.randrange(0, 300)
        args = (matrix, font, kw.get("fontsize", rng.choice([1.0, 1.0, 0, 10.0, -12.0, math.nan])),
                kw.get("scaling", rng.choice([1.0, 1.0, 0.5, -1.0, 0.0])), kw.get("rise", rng.choice([0, 0.0, 2.5])),
                cid, PDFColorSpace(*rng.choice(COLORSPACES)), self.gstate())
        self.call("render_char", *args, py=py)

    def figure_items(self, n):
        rng = self.rng
        out = []
        for _ in range(n):
            r = rng.random()
            out.append(rng.choice(RAW_ITEMS) if r < 0.8 else (rng.choice(RAW_ODD) if r < 0.95 else rng.uniform(-5, 5)))
        return out

    def begin_figure(self, name=None, py=0, bbox=None, matrix=None, odd=0.1, tame=False):
        # a tame figure has a moderate box (an analyzed one with chars: see
        # the mixed cases of adversarial_cases)
        n = 4 if tame else 6
        rng = self.rng
        if bbox is None:
            bbox = rng.choice([[0, 0, 100, 100], [0.0, 0.0, 10.5, 20.0], (0, 0, 1, 1), [-5, 3, 0, 0],
                               [True, False, 2**63, 1e308], [math.nan, -0.0, math.inf, 7]][:n]) \
                if rng.random() >= odd else self.figure_items(rng.choice([4, 4, 4, 3, 5, 0]))
        if matrix is None:
            matrix = rng.choice([[1, 0, 0, 1, 0, 0], (1, 0, 0, 1, 0, 0), [2.0, 0.5, -0.5, 2.0, 10.0, 20.0],
                                 [0, 1, -1, 0, 50.0, 0], [True, 0, 0, True, 2**62, -3],
                                 [math.nan, 0, 0, math.inf, -0.0, 1e308]][:n]) \
                if rng.random() >= odd else self.figure_items(rng.choice([6, 6, 6, 5, 7]))
            if isinstance(matrix, list) and rng.random() < 0.2:
                matrix = tuple(matrix)
        self.call("begin_figure", name or rng.choice(["Fm0", "X", "Im1"]), False, bbox, matrix, py=py)

    def image(self, py=0):
        rng = self.rng
        self.call("render_image", rng.choice(["Im0", "Im1"]), False, PDFStream(dict(rng.choice(IMAGE_ATTRS)), b""),
                  py=py)

    def inline_image(self, py=0):
        name = f"inline image {self.inline}"
        self.inline += 1
        stream = PDFStream(dict(self.rng.choice(IMAGE_ATTRS)), b"")
        self.call("begin_figure", name, True, (0, 0, 1, 1), (1, 0, 0, 1, 0, 0), py=py)
        self.call("render_image", name, True, stream, py=py)
        self.call("end_figure", name, True, py=py)

    def tag(self, py=0):
        rng = self.rng
        r = rng.random()
        if r < 0.5:
            self.call("begin_tag", rng.choice(TAGS), rng.choice(PROPS), py=py)
        elif r < 0.8:
            self.call("end_tag", py=py)
        else:
            self.call("do_tag", rng.choice(TAGS), rng.choice(PROPS), py=py)

    def case(self, name, configs=(None, {}), pageno=1, re_compiled=True):
        return new_case(name, self.builder, pageno, list(configs), re_compiled)


def all_shapes(max_len: int) -> list[str]:
    shapes = [""]
    frontier = [""]
    for _ in range(max_len):
        frontier = [s + op for s in frontier for op in OPS]
        shapes.extend(frontier)
    return shapes


def rect_paths(rng) -> list[list[tuple]]:
    """Rectangles in every winding, closed by ``h``, by a fourth ``l``, by
    both (the redundant ``l`` dropped), with a wrong closing ``l``, and
    degenerate ones; and ``do_re``'s arithmetic."""
    out = []
    for x0, y0, x1, y1 in ((0.0, 0.0, 5.0, 3.0), (1.0, 1.0, 1.0, 4.0), (2.0, 2.0, 2.0, 2.0), (0.0, -0.0, 3.0, 0.0),
                           (math.nan, 0.0, 1.0, 1.0), (0.0, 0.0, math.inf, 1.0), (1e308, 1e308, -1e308, -1e308)):
        corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
        for start in range(4):
            for direction in (1, -1):
                pts = [corners[(start + direction * k) % 4] for k in range(4)]
                m = [("m", *pts[0])] + [("l", *p) for p in pts[1:]]
                out.append(m + [("h",)])
                out.append(m + [("l", *pts[0])])
                out.append(m + [("l", *pts[0]), ("h",)])
                out.append(m + [("l", *pts[1]), ("h",)])
        w, h = rng.choice([5.0, -5.0, 0.0, -0.0]), rng.choice([3.0, -3.0, 0.0, math.nan])
        out.append([("m", x0, y0), ("l", x0 + w, y0), ("l", x0 + w, y0 + h), ("l", x0, y0 + h), ("h",)])
    return out


def adversarial_cases(rng, table: dict[str, list]):
    """The generated sequences (their runs filled in by ``finish``)."""
    fonts = generated_fonts(rng, table)
    # every shape up to 4 operators, under each CTM
    shapes = all_shapes(4)
    rng.shuffle(shapes)
    for k in range(0, len(shapes), 40):
        g = Gen(rng, fonts)
        g.begin(ctm=CTMS[(k // 40) % len(CTMS)])
        for shape in shapes[k:k + 40]:
            g.paint(g.path(shape, special=0.05))
        g.call("end_page")
        yield g.case(f"shapes {k}")
    # longer random shapes, special coordinates
    for k in range(30):
        g = Gen(rng, fonts)
        g.begin(ctm=rng.choice(CTMS))
        for _ in range(30):
            shape = "".join(rng.choice("mmllcvyh") for _ in range(rng.randint(5, 14)))
            g.paint(g.path(shape, special=0.25))
        g.call("end_page")
        yield g.case(f"long shapes {k}", re_compiled=k % 2 == 0)
    # rectangles
    for ctm in CTMS:
        g = Gen(rng, fonts)
        g.begin(ctm=ctm)
        for p in rect_paths(rng):
            g.paint(p)
        g.call("end_page")
        yield g.case(f"rects {canon(ctm)}")
    # figures, images, chars, tags, mixed
    for k in range(150):
        g = Gen(rng, fonts)
        # an analyzed page has a finite box: the analysis's Plane covers
        # it with a grid (an infinite or huge page would loop and allocate
        # without end, in Python as in the port)
        mediabox = rng.choice(MEDIABOXES)
        analyzed = math.isfinite(sum(mediabox)) and max(map(abs, mediabox)) < 1e6
        g.begin(ctm=rng.choice(CTMS[:-3] if analyzed else CTMS), mediabox=mediabox)
        open_figures = 0
        for _ in range(rng.randint(5, 60)):
            r = rng.random()
            if r < 0.3:
                g.char()
            elif r < 0.45:
                g.paint(g.path(rng.choice(["ml", "mlh", "mlllh", "mllll", "mlc", "mlml", "lm", "mvyh"]), 0.1))
            elif r < 0.55 and open_figures < 4:
                g.begin_figure(odd=0.0 if analyzed else 0.1, tame=analyzed)
                open_figures += 1
            elif r < 0.65 and open_figures:
                g.call("end_figure", "Fm0", False)
                open_figures -= 1
            elif r < 0.7 and open_figures:
                g.image()
            elif r < 0.75:
                g.inline_image()
            elif r < 0.9:
                g.tag()
            else:
                g.call("set_ctm", rng.choice(CTMS[:-3] if analyzed else CTMS))
        for _ in range(open_figures):
            g.call("end_figure", "Fm0", False)
        g.call("end_page")
        yield g.case(f"mixed {k}", configs=(None, {}, EXTRA_CONFIG) if analyzed else (None,),
                     pageno=rng.choice([1, 2, 0, -5, 2**70]))
    # chars of every font kind and cid
    for kind in fonts:
        g = Gen(rng, fonts)
        g.begin()
        for cid in CIDS:
            g.char(kind, cid)
            g.tag()
        for _ in range(40):
            g.char(kind)
        g.call("end_page")
        yield g.case(f"chars {kind}")
    # tags of every kind before every kind of object
    g = Gen(rng, fonts)
    g.begin()
    for tag in TAGS:
        for props in PROPS:
            g.call("begin_tag", tag, props)
            rng.choice([g.char, lambda: g.paint(g.path("ml")), g.inline_image, lambda: g.paint(g.path("l"))])()
            if rng.random() < 0.5:
                g.call("end_tag")
            g.call("do_tag", tag, props)
    g.call("end_page")
    yield g.case("tags")
    # calls the interpreter never makes
    for k, calls in enumerate([
        [("render_image", "Im0", False, PDFStream({}, b""))],
        [("end_figure", "Fm0", False)],
        [("begin_figure", "Fm0", False, [0, 0, 1, 1], [1, 0, 0, 1, 0, 0]), ("end_page",)],
        [("begin_figure", "Fm0", False, [0, 0, 1, 1], [1, 0, 0, 1, 0, 0])],
        [],
    ]):
        g = Gen(rng, fonts)
        g.begin()
        g.char()
        for c in calls:
            g.call(*c)
        yield g.case(f"out of grammar {k}")
    # figures of every odd /BBox and /Matrix: each case ends with one
    for k in range(120):
        g = Gen(rng, fonts)
        g.begin()
        g.char()
        g.begin_figure(odd=1.0)
        g.char()
        g.call("end_figure", "Fm0", False)
        g.call("end_page")
        yield g.case(f"odd figure {k}")
    # a coordinate or a CTM item too large for float()
    for k in range(6):
        g = Gen(rng, fonts)
        g.begin(ctm=(1, 0, 0, 1, HUGE, 0) if k % 2 else None)
        path = g.path(rng.choice(["ml", "mlllh", "mlc"]), special=0)
        if not k % 2:
            path[rng.randrange(len(path))] = ("m", HUGE, 0.0) if k < 3 else ("l", 1, HUGE)
            path[0] = ("m", *path[0][1:]) if path[0][0] == "m" else path[0]
        g.paint(path)
        g.call("end_page")
        yield g.case(f"huge {k}")
    # every font, even those whose chars raise
    for kind, refs in fonts.items():
        for ref in refs:
            g = Gen(rng, {kind: [ref]})
            g.begin()
            for cid in (65, 0, 0x4E00, 32):
                g.char(kind, cid)
            g.call("end_page")
            yield g.case(f"font {kind} {ref.sha[:8]}")


# -- recursion boundaries ----------------------------------------------------------------


def depth_probes(fonts: dict[str, list[FontRef]]):
    """``(name, make, config, re_compiled)``: ``make(g, py)`` writes the
    setup calls (at the replay's first level) and the probe call at
    ``py`` frames below it."""

    def page(g):
        g.begin(ctm=(1.0, 0.0, 0.0, 1.0, 0.0, 0.0))

    def in_figure(g):
        page(g)
        g.begin_figure(bbox=[0, 0, 10, 10], matrix=[1, 0, 0, 1, 0, 0])

    def probe(setup, call, end=True):
        def make(g, py):
            setup(g)
            call(g, py)
            if end:
                g.call("end_page")
        return make

    def first_page(g, py):
        g.call("begin_page", (0.0, 0.0, 612.0, 792.0), (1, 0, 0, 1, 0, 0), py=py)
        g.call("end_page")

    yield "begin_page", first_page, None, True
    yield "set_ctm", probe(page, lambda g, py: g.call("set_ctm", (1, 0, 0, 1, 0, 0), py=py)), None, True
    yield "end_page", probe(page, lambda g, py: g.call("end_page", py=py), end=False), None, True

    # the analysis of a page without chars (its frames down to the
    # objects' analyze): the grouping of chars into lines, boxes and groups
    # does not count its callees (crates/pdfminer/src/layout.rs,
    # "Recursion"), which only a caller deep in the stack would reach, not
    # pdfminer's interpreter (process_page calls end_page)
    def end_page_analysis(g, py, figure):
        page(g)
        g.paint(g.path("mlc", special=0))
        if figure:
            g.begin_figure(bbox=[0, 0, 10, 10], matrix=[1, 0, 0, 1, 0, 0])
            g.paint(g.path("ml", special=0))
            g.call("end_figure", "Fm0", False)
        g.call("end_page", py=py)

    for figure in (False, True):
        for config in ({}, {"all_texts": True}):
            yield f"end_page analysis {figure} {canon(config)}", \
                lambda g, py, f=figure: end_page_analysis(g, py, f), config, True
    for bbox, matrix in (([0, 0, 10, 10], [1, 0, 0, 1, 0, 0]), ([0, 0, 10], [1, 0, 0, 1, 0, 0]),
                         ([0, 0, 10, 10], [1, 0, 0, 1, 0]), ([0, 0, LIT("x"), 10], [1, 0, 0, 1, 0, 0]),
                         ([0, 0, 10, 10], [LIT("x"), 0, 0, 1, 0, 0])):
        def figure(g, py, b=bbox, m=matrix):
            g.begin_figure(bbox=b, matrix=m, py=py)
            g.call("end_figure", "Fm0", False)
        yield f"begin_figure {canon(bbox)} {canon(matrix)}", probe(page, figure), None, True
    yield "end_figure", probe(in_figure, lambda g, py: g.call("end_figure", "Fm0", False, py=py)), None, True

    def image(g, py):
        g.image(py=py)
        g.call("end_figure", "Fm0", False)

    yield "render_image", probe(in_figure, image), None, True
    yield "render_image outside", probe(page, lambda g, py: g.image(py=py)), None, True
    for shape in ("", "l", "ml", "mlh", "mlllh", "mllll", "mlc", "mlmlh", "mmlh", "mlllhml"):
        for compiled in ((True, False) if shape.count("m") > 1 else (True,)):
            yield f"paint_path {shape!r} {compiled}", probe(
                page, lambda g, py, s=shape: g.paint(g.path(s, special=0), py=py)), None, compiled
    for kind in fonts:
        for cid in (65, 0x4E00, 10**4300):
            label = cid if cid < 10**10 else "huge"
            yield f"render_char {kind} {label}", probe(
                page, lambda g, py, k=kind, c=cid: g.char(k, c, py=py, fontsize=1.0, scaling=1.0, rise=0)), \
                None, True

            def warm(g, py, k=kind, c=cid):
                g.char(k, c, fontsize=1.0, scaling=1.0, rise=0)
                g.char(k, c, py=py, fontsize=1.0, scaling=1.0, rise=0)

            yield f"render_char warm {kind} {label}", probe(page, warm), None, True
    for tag in TAGS:
        for props in (None, {"MCID": 3}):
            yield f"begin_tag {canon(tag)} {canon(props)}", probe(
                page, lambda g, py, t=tag, p=props: g.call("begin_tag", t, p, py=py)), None, True
    yield "end_tag", probe(page, lambda g, py: g.call("end_tag", py=py)), None, True
    yield "do_tag", probe(page, lambda g, py: g.call("do_tag", LIT("P"), None, py=py)), None, True


def depth_cases(rng, table: dict[str, list]):
    fonts = generated_fonts(rng, table)
    top = 1000 - REPLAY_DEPTH - 1

    def build(make, py, config, re_compiled):
        g = Gen(random.Random(0), fonts)
        make(g, py)
        return place_depths(g.case("probe", configs=[config], re_compiled=re_compiled))

    for name, make, config, re_compiled in depth_probes(fonts):

        def recursion(py):
            case = decoded(encoded(build(make, py, config, re_compiled)))
            return "!RecursionError" in replay_text(case, table, 0)

        if recursion(0):  # pragma: no cover
            raise AssertionError(f"{name}: RecursionError at the replay's first level")
        lo, hi = 0, top
        while lo < hi:
            mid = (lo + hi + 1) // 2
            if recursion(mid):
                hi = mid - 1
            else:
                lo = mid
        for py in range(max(0, lo - 2), min(top, lo + 3) + 1):
            case = build(make, py, config, re_compiled)
            case[0] = f"depth {name} +{py}"
            yield case


# -- main ------------------------------------------------------------------------------------


def generated_set(name: str, cases_of, rng) -> int:
    table: dict[str, list] = {}
    lines = []
    fonts_used: set[str] = set()
    for case in cases_of(rng, table):
        done = finish(place_depths(case), table)
        if done is None:
            continue
        args, outs, _ = done
        fonts_used.update(args[2])
        lines.append((encoded(pack_calls(args)), encoded(outs)))
    write_jsonl(f"{name}_fonts", [(encoded([sha, table[sha]]), "null") for sha in sorted(fonts_used)])
    return write_jsonl(name, lines)


def write_marker(name: str, **extra) -> None:
    with open(unit_dir(MODULE) / f"{name}.json", "w", encoding="ascii") as f:
        json.dump({"sources": oracle_sources(), **extra}, f)


def dump_case(name: str, index: int, run: int) -> None:
    """Prints Python's full output text of one run of a case."""
    table = {}
    for text in (unit_dir(MODULE) / f"{name}_fonts.jsonl").read_text().splitlines():
        sha, dump = decoded(text[len('{"in":'):text.rindex(',"out":')])
        table[sha] = dump
    with open(unit_dir(MODULE) / f"{name}.jsonl", encoding="ascii") as f:
        for i, text in enumerate(f):
            if i == index:
                case = decoded(text[len('{"in":'):text.rindex(',"out":')])
                break
    print(replay_text(case, table, run), end="")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--corpus", action="store_true", help="the calls of the real corpus (minutes: scripts/heavy.sh)")
    ap.add_argument("-j", "--jobs", type=int, default=3)
    ap.add_argument("--dump", nargs=3, metavar=("SET", "INDEX", "RUN"))
    args = ap.parse_args()
    require_oracle_python()
    sys.setrecursionlimit(1000)
    if args.dump:
        dump_case(args.dump[0], int(args.dump[1]), int(args.dump[2]))
        return
    if args.corpus:
        info = corpus_set(args.jobs)
        write_marker("corpus", **info)
        print(f"wrote {info}")
        return
    cmap_dir = codegen_cmaps.ensure()
    counts = {name: generated_set(name, gen, random.Random(f"20261018:{name}"))
              for name, gen in (("adversarial", adversarial_cases), ("depth", depth_cases))}
    write_meta(MODULE, counts=counts, cmap_dir=cmap_dir.relative_to(cache_dir()).as_posix())
    print(f"wrote {counts}")


if __name__ == "__main__":
    main()
