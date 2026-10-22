"""Summarize a V8 .cpuprofile (`node --cpu-prof`, the Wasm module with its
name section: `wasm-opt -O3 -g`): self and inclusive sample share per
function.

usage: prof_sum.py PROFILE [N] [FILTER]
Rust symbols are demangled roughly (hash suffix dropped).
"""
import json
import re
import sys
from collections import Counter, defaultdict

prof = json.load(open(sys.argv[1]))
n = int(sys.argv[2]) if len(sys.argv) > 2 else 60
filt = sys.argv[3] if len(sys.argv) > 3 else None
nodes = {x["id"]: x for x in prof["nodes"]}
parent = {}
for x in prof["nodes"]:
    for c in x.get("children", []):
        parent[c] = x["id"]


def name(node):
    f = node["callFrame"]["functionName"] or "(anon)"
    f = re.sub(r"::h[0-9a-f]{16}$", "", f)
    f = f.replace("$LT$", "<").replace("$GT$", ">").replace("$u20$", " ").replace("$RF$", "&").replace("..", "::")
    f = re.sub(r"^_ZN|17h[0-9a-f]{16}E$", "", f)
    return f[:150]


hits = Counter()
for s in prof["samples"]:
    hits[s] += 1
total = sum(hits.values())
self_t = Counter()
incl = Counter()
for nid, h in hits.items():
    self_t[name(nodes[nid])] += h
    seen = set()
    cur = nid
    while cur is not None:
        nm = name(nodes[cur])
        if nm not in seen:
            seen.add(nm)
            incl[nm] += h
        cur = parent.get(cur)
print("samples", total)
print("--- self")
for k, v in self_t.most_common(n):
    if filt and not re.search(filt, k):
        continue
    print(f"{100 * v / total:6.2f}% {k}")
print("--- inclusive")
for k, v in incl.most_common(n):
    if filt and not re.search(filt, k):
        continue
    print(f"{100 * v / total:6.2f}% {k}")
