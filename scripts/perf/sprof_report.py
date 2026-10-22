"""Report of sprof.c (or mcount.c) samples: self and inclusive share per
function, or the callees / callers of the functions matching REGEX. BINARY
must be the very file that ran (symbols by `nm -C`; other objects by `nm -D`,
exported symbols only: libc's internal functions show under the nearest
exported name).

usage: sprof_report.py OUT BINARY [N] [--callees REGEX] [--callers REGEX]
"""
import bisect
import re
import struct
import subprocess
import sys
from collections import Counter

out, binary = sys.argv[1], sys.argv[2]
args = sys.argv[3:]
n = 50
callees = callers = None
i = 0
while i < len(args):
    if args[i] == "--callees":
        callees = re.compile(args[i + 1]); i += 2
    elif args[i] == "--callers":
        callers = re.compile(args[i + 1]); i += 2
    else:
        n = int(args[i]); i += 1

syms = []
for line in subprocess.run(["nm", "-C", "--defined-only", binary], capture_output=True, text=True).stdout.splitlines():
    p = line.split(" ", 2)
    if len(p) == 3 and p[1] in "tTwW":
        syms.append((int(p[0], 16), p[2]))
syms.sort()
addrs = [a for a, _ in syms]
base = None
for line in open(out + ".maps"):
    f = line.split()
    if len(f) >= 6 and f[5].endswith(binary.split("/")[-1]) and int(f[2], 16) == 0:
        base = int(f[0].split("-")[0], 16)
        break
lo_hi = []
for line in open(out + ".maps"):
    f = line.split()
    if len(f) >= 6 and f[5].endswith(binary.split("/")[-1]):
        a, b = f[0].split("-")
        lo_hi.append((int(a, 16), int(b, 16)))

cache = {}
ext_maps = []
for line in open(out + ".maps"):
    f = line.split()
    if len(f) >= 6 and "x" in f[1] and f[5].startswith("/") and not f[5].endswith(binary.split("/")[-1]):
        a, b = f[0].split("-")
        ext_maps.append((int(a, 16), int(b, 16), int(f[2], 16), f[5]))
ext_syms = {}


def ext(a):
    for lo, hi, off, path in ext_maps:
        if lo <= a < hi:
            if path not in ext_syms:
                ss = []
                for line in subprocess.run(["nm", "-D", "--defined-only", path], capture_output=True, text=True).stdout.splitlines():
                    p = line.split()
                    if len(p) == 3 and p[1] in "tTiWw":
                        ss.append((int(p[0], 16), p[2]))
                ss.sort()
                ext_syms[path] = ss
            ss = ext_syms[path]
            v = a - lo + off
            k = bisect.bisect_right([x for x, _ in ss], v) - 1
            return "[" + path.split("/")[-1] + "] " + (ss[k][1] if k >= 0 else "?")
    return "?ext"


def sym(a, ret):
    key = (a, ret)
    if key in cache:
        return cache[key]
    if not any(lo <= a < hi for lo, hi in lo_hi):
        r = ext(a)
    else:
        v = a - base - (1 if ret else 0)
        k = bisect.bisect_right(addrs, v) - 1
        r = syms[k][1] if k >= 0 else "?"
        r = re.sub(r"::h[0-9a-f]{16}$", "", r)
        r = re.sub(r"<impl [^>]*>", "", r)
    cache[key] = r[:170]
    return cache[key]


data = open(out, "rb").read()
words = struct.unpack(f"<{len(data) // 8}Q", data)
pos = 0
selfc = Counter()
incl = Counter()
edges = Counter()
up = Counter()
total = 0
while pos < len(words):
    d = words[pos]
    stack = [sym(a, k > 0) for k, a in enumerate(words[pos + 1:pos + 1 + d])]
    pos += 1 + d
    total += 1
    selfc[stack[0]] += 1
    seen = set()
    for k, s in enumerate(stack):
        if s not in seen:
            seen.add(s)
            incl[s] += 1
        if callees and callees.search(s) and k > 0:
            edges[stack[k - 1]] += 1
        if callers and callers.search(s) and k + 1 < len(stack):
            up[stack[k + 1]] += 1
print("samples", total)
if callees:
    print("--- callees of", callees.pattern)
    for k, v in edges.most_common(n):
        print(f"{100 * v / total:6.2f}% {k}")
    sys.exit()
if callers:
    print("--- callers of", callers.pattern)
    for k, v in up.most_common(n):
        print(f"{100 * v / total:6.2f}% {k}")
    sys.exit()
print("--- self")
for k, v in selfc.most_common(n):
    print(f"{100 * v / total:6.2f}% {k}")
print("--- inclusive")
for k, v in incl.most_common(n):
    print(f"{100 * v / total:6.2f}% {k}")
