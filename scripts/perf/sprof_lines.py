"""Self samples of the functions matching REGEX, by source line (addr2line;
the binary built with line tables), innermost inlined frame first.

usage: sprof_lines.py OUT BINARY REGEX [N]
"""
import bisect
import re
import struct
import subprocess
import sys
from collections import Counter

out, binary, pat = sys.argv[1], sys.argv[2], re.compile(sys.argv[3])
n = int(sys.argv[4]) if len(sys.argv) > 4 else 40
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
data = open(out, "rb").read()
words = struct.unpack(f"<{len(data) // 8}Q", data)
pos = 0
hits = Counter()
total = 0
while pos < len(words):
    d = words[pos]
    ip = words[pos + 1]
    pos += 1 + d
    total += 1
    v = ip - base
    k = bisect.bisect_right(addrs, v) - 1
    if k >= 0 and pat.search(syms[k][1]):
        hits[v] += 1
res = subprocess.run(["addr2line", "-i", "-e", binary] + [hex(a) for a in hits], capture_output=True, text=True).stdout
# with -i, each address may print several lines (inlining): the first is the innermost
lines = Counter()
outl = res.splitlines()
# re-run per address to keep the mapping simple
for a, c in hits.items():
    r = subprocess.run(["addr2line", "-i", "-e", binary, hex(a)], capture_output=True, text=True).stdout.splitlines()
    key = " <- ".join(x.split("/")[-1] for x in r[:3])
    lines[key] += c
print("samples", total, "in matching functions", sum(hits.values()))
for k, c in lines.most_common(n):
    print(f"{100 * c / total:6.2f}% {k}")
