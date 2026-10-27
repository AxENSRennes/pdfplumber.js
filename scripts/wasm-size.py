"""Where the bytes of a WebAssembly module go (docs/js-runtime.md, "Size"):
sections, function bodies by crate, by module and by generic function
(instances counted), the largest functions, and, given the linker's map,
the data section by crate with its largest constants. twiggy, cargo-bloat
and wabt are not installed on the machine; this reads the module itself.

Function names come from the module's "name" section: the shipped module
has none (wasm-opt --strip-debug), so profile a module that keeps them,
optimized like the shipped one (`-g`); data needs the map of wasm-ld:

    node scripts/build-wasm.mjs        # cargo dist build + the shipped module
    node node_modules/binaryen/bin/wasm-opt -O3 --always-inline-max-function-size=0 \\
      --flexible-inline-max-function-size=0 --one-caller-inline-max-function-size=0 \\
      -g --strip-dwarf --strip-producers --all-features \\
      target/wasm32-unknown-unknown/dist/pdfplumber_wasm.wasm -o /tmp/named.wasm   # build-wasm.mjs's flags, -g
    python3 scripts/wasm-size.py /tmp/named.wasm
    cargo rustc --profile dist --target wasm32-unknown-unknown -p pdfplumber-wasm --lib \\
      -- -C link-arg=--Map=/tmp/link.map      # the same module, with its map
    python3 scripts/wasm-size.py --map /tmp/link.map target/wasm32-unknown-unknown/dist/pdfplumber_wasm.wasm

Rust v0 symbols are demangled by `c++filt` (binutils) when it is there.
`--grep REGEX` (repeatable) totals the functions whose demangled name
matches; `--dump-crates DIR` writes each crate's bodies to DIR/<crate>.bin,
to compress them separately (`node scripts/size.mjs` does not; brotli of a
part is an estimate: the parts share the module's dictionary).
"""

import argparse
import os
import re
import shutil
import subprocess
import sys
from collections import defaultdict

SECTIONS = {0: "custom", 1: "type", 2: "import", 3: "function", 4: "table", 5: "memory", 6: "global",
            7: "export", 8: "start", 9: "element", 10: "code", 11: "data", 12: "datacount", 13: "tag"}


def leb(buf, pos):
    result = shift = 0
    while True:
        b = buf[pos]
        pos += 1
        result |= (b & 0x7F) << shift
        shift += 7
        if not b & 0x80:
            return result, pos


def name_at(buf, pos):
    n, pos = leb(buf, pos)
    return buf[pos:pos + n].decode("utf-8", "replace"), pos + n


def sections(buf):
    if buf[:4] != b"\0asm":
        sys.exit("not a WebAssembly module")
    pos, out = 8, []
    while pos < len(buf):
        sid = buf[pos]
        size, start = leb(buf, pos + 1)
        name = SECTIONS.get(sid, str(sid))
        if sid == 0:
            name = "custom:" + name_at(buf, start)[0]
        out.append({"name": name, "start": start, "size": size, "total": start + size - pos})
        pos = start + size
    return out


def imported_functions(buf, sec):
    pos = sec["start"]
    count, pos = leb(buf, pos)
    funcs = 0
    for _ in range(count):
        _, pos = name_at(buf, pos)
        _, pos = name_at(buf, pos)
        kind = buf[pos]
        pos += 1
        if kind == 0:  # function: type index
            _, pos = leb(buf, pos)
            funcs += 1
        elif kind == 1:  # table: reftype, limits
            pos += 1
            flags, pos = leb(buf, pos)
            _, pos = leb(buf, pos)
            if flags & 1:
                _, pos = leb(buf, pos)
        elif kind == 2:  # memory: limits
            flags, pos = leb(buf, pos)
            _, pos = leb(buf, pos)
            if flags & 1:
                _, pos = leb(buf, pos)
        elif kind == 3:  # global: valtype, mutability
            pos += 2
        elif kind == 4:  # tag
            pos += 1
            _, pos = leb(buf, pos)
    return funcs


def bodies(buf, sec):
    """(offset, size) of each function body, size counted with its length."""
    pos = sec["start"]
    count, pos = leb(buf, pos)
    out = []
    for _ in range(count):
        size, start = leb(buf, pos)
        out.append((pos, start + size - pos))
        pos = start + size
    return out


def function_names(buf, sec):
    pos, end = sec["start"], sec["start"] + sec["size"]
    _, pos = name_at(buf, pos)
    names = {}
    while pos < end:
        sub = buf[pos]
        size, pos = leb(buf, pos + 1)
        if sub == 1:
            p = pos
            count, p = leb(buf, p)
            for _ in range(count):
                idx, p = leb(buf, p)
                names[idx], p = name_at(buf, p)
        pos += size
    return names


DISAMBIGUATOR = re.compile(r"\[[0-9a-f]{1,16}\]")
HASH = re.compile(r"::h[0-9a-f]{16}$")


def demangle(names):
    if not names or not shutil.which("c++filt"):
        return names
    keys = list(names)
    # wasm-opt makes duplicate names unique with a `_N` suffix
    raw = [re.sub(r"_\d+$", "", names[k]) if names[k].startswith("_R") else names[k] for k in keys]
    out = subprocess.run(["c++filt"], input="\n".join(r.replace("\n", " ") for r in raw),
                         capture_output=True, text=True, check=True).stdout.split("\n")
    return {k: DISAMBIGUATOR.sub("", out[i]) for i, k in enumerate(keys)}


def erase_generics(s):
    out, depth = [], 0
    for ch in s:
        if ch == "<":
            depth += 1
            if depth == 1:
                out.append("<")
        elif ch == ">":
            depth -= 1
            if depth == 0:
                out.append(">")
        elif depth == 0:
            out.append(ch)
    return "".join(out)


PRIMITIVE = re.compile(r"^(\(|\[|fn|bool|char|str\b|[iuf](8|16|32|64|128|size)\b)")
REFS = re.compile(r"^(&|\*|mut |const |dyn )+")


def split_impl(n):
    """`<A as B>::rest` or `<A>::rest` -> (A, B or None, ::rest)."""
    depth = 0
    for i, ch in enumerate(n):
        if ch == "<":
            depth += 1
        elif ch == ">":
            depth -= 1
            if depth == 0:
                inner, rest = n[1:i], n[i + 1:]
                break
    else:
        return n[1:], None, ""
    depth = 0
    for i, ch in enumerate(inner):
        if ch in "<([":
            depth += 1
        elif ch in ">)]":
            depth -= 1
        elif depth == 0 and inner.startswith(" as ", i):
            return inner[:i], inner[i + 4:], rest
    return inner, None, rest


def generic_key(name):
    """A function with its type parameters erased, its impl kept:
    `<a::S<T> as b::Tr>::f::<U>` -> `<a::S<> as b::Tr>::f::<>`."""
    n = HASH.sub("", name)
    if not n.startswith("<"):
        return erase_generics(n)
    ty, trait, rest = split_impl(n)
    return "<" + erase_generics(ty) + (" as " + erase_generics(trait) if trait else "") + ">" + erase_generics(rest)


def path_of(name):
    """The module path of a demangled symbol: `<A as B>::f` is A's (B's when
    A is a primitive, a slice, a tuple or a function), `<A>::f` A's."""
    n = HASH.sub("", name)
    if n.startswith("<"):
        ty, trait, _ = split_impl(n)
        ty = REFS.sub("", ty.strip())
        if PRIMITIVE.match(ty) and trait:
            ty = REFS.sub("", trait.strip())
        n = ty
    return [p for p in erase_generics(n).split("::") if p]


def table(title, totals, total, top):
    print(f"\n# {title}")
    for k, (size, count) in sorted(totals.items(), key=lambda kv: -kv[1][0])[:top]:
        print(f"  {size:>9} {100 * size / total:5.1f}% {count:>6} fn  {k[:150]}")


def profile(args, buf):
    secs = sections(buf)
    print(f"# {args.wasm}: {len(buf)} bytes")
    for s in secs:
        print(f"  section {s['name']:<28} {s['total']:>10}  {100 * s['total'] / len(buf):5.1f}%")
    by = {s["name"]: s for s in secs}
    if "code" not in by:
        return
    first = imported_functions(buf, by["import"]) if "import" in by else 0
    names = demangle(function_names(buf, by["custom:name"])) if "custom:name" in by else {}
    funcs = [(off, size, names.get(first + i, f"func[{first + i}]")) for i, (off, size) in enumerate(bodies(buf, by["code"]))]
    code = sum(f[1] for f in funcs)
    print(f"\n# code: {len(funcs)} functions, {code} bytes; {len(names)} named")
    if not names:
        print("  (no name section: profile a module built with wasm-opt -g, see the docstring)")
    if args.grep:
        for g in args.grep:
            rx = re.compile(g)
            sel = [f for f in funcs if rx.search(f[2])]
            print(f"# --grep {g}: {len(sel)} functions, {sum(f[1] for f in sel)} bytes")
            for _, size, n in sorted(sel, key=lambda f: -f[1])[: args.top]:
                print(f"  {size:>8}  {n[:200]}")
        return
    crate, module, generic = (defaultdict(lambda: [0, 0]) for _ in range(3))
    for _, size, n in funcs:
        p = path_of(n)
        for d, k in ((crate, p[0] if p else n), (module, "::".join(p[: args.depth]) if p else n),
                     (generic, generic_key(n))):
            d[k][0] += size
            d[k][1] += 1
    table("by crate (code bytes, % of code)", crate, code, args.top)
    table(f"by module (path depth {args.depth})", module, code, args.top)
    dup = {k: v for k, v in generic.items() if v[1] > 1}
    table("generic functions with several instances (type parameters erased)", dup, code, args.top)
    print(f"  instances beyond the first: {sum(v[1] - 1 for v in dup.values())} functions, "
          f"{sum(v[0] for v in dup.values())} bytes in all")
    print("\n# largest functions")
    for _, size, n in sorted(funcs, key=lambda f: -f[1])[: args.top]:
        print(f"  {size:>8}  {n[:180]}")
    if args.dump_crates:
        os.makedirs(args.dump_crates, exist_ok=True)
        parts = defaultdict(list)
        for off, size, n in funcs:
            p = path_of(n)
            parts[p[0] if p else "other"].append(buf[off:off + size])
        for c, bs in parts.items():
            with open(os.path.join(args.dump_crates, re.sub(r"[^\w.-]", "_", c) + ".bin"), "wb") as f:
                f.write(b"".join(bs))


def data_map(args, buf):
    """The DATA output section of a wasm-ld map, by input crate."""
    lines = open(args.map).read().split("\n")
    start = next(i for i, line in enumerate(lines) if line.split()[3:4] == ["DATA"])
    crate, outsec, items, cur = defaultdict(int), defaultdict(int), [], None
    for line in lines[start + 1:]:
        if len(line) < 28 or not line[26:].strip():
            continue
        col = len(line) - len(line[26:].lstrip())
        parts = line.split(None, 3)
        if len(parts) < 4:
            continue
        _, off, size, rest = parts
        if col == 27:  # an output section (.rodata, .data, .bss)
            cur = rest.strip()
            continue
        if col != 35:  # a symbol under an input section
            continue
        n = int(size, 16)
        m = re.search(r"lib([a-z_0-9]+)-[0-9a-f]+\.rlib", rest)
        c = m.group(1) if m else "<linker: merged strings>" if rest.startswith("<internal>") else re.sub(r"\..*", "", os.path.basename(rest.split("(")[0]))
        crate[c] += n
        outsec[cur] += n
        items.append((n, c, rest[rest.find("(") + 1:].rstrip(")"), int(off, 16) if off != "-" else 0, cur))
    total = sum(crate.values())
    print(f"\n# data ({args.map}): {total} bytes")
    for k, v in sorted(outsec.items(), key=lambda kv: -kv[1]):
        print(f"  {v:>9}  output section {k}")
    for k, v in sorted(crate.items(), key=lambda kv: -kv[1]):
        print(f"  {v:>9} {100 * v / total:5.1f}%  {k}")
    print("\n# largest input sections (first bytes, read from the module at the map's offset)")
    for n, c, sec, off, cur in sorted(items, key=lambda i: -i[0])[: args.top]:
        head = buf[off:off + 40] if cur != ".bss" else b"(bss)"
        print(f"  {n:>8}  {c:<16} {sec[-40:]:<40} {''.join(chr(b) if 32 <= b < 127 else '.' for b in head)}")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("wasm")
    ap.add_argument("--top", type=int, default=30)
    ap.add_argument("--depth", type=int, default=2, help="path depth of the module table")
    ap.add_argument("--grep", action="append", help="total the functions whose name matches (repeatable)")
    ap.add_argument("--dump-crates", metavar="DIR")
    ap.add_argument("--map", help="the wasm-ld map of this module (-C link-arg=--Map=FILE): data by crate")
    args = ap.parse_args()
    buf = open(args.wasm, "rb").read()
    if args.map:
        data_map(args, buf)
    else:
        profile(args, buf)


if __name__ == "__main__":
    main()
