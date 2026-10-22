"""Inclusive share of each direct callee of the functions matching REGEX in
a V8 .cpuprofile.

usage: prof_callees.py PROFILE REGEX [N]
"""
import json
import re
import sys
from collections import Counter

prof = json.load(open(sys.argv[1]))
pat = re.compile(sys.argv[2])
n = int(sys.argv[3]) if len(sys.argv) > 3 else 40
nodes = {x["id"]: x for x in prof["nodes"]}
hits = Counter(prof["samples"])
total = sum(hits.values())


def name(node):
    f = node["callFrame"]["functionName"] or "(anon)"
    return re.sub(r"Cs[0-9A-Za-z]{10,13}_", "", f)[:160]


memo = {}


def incl(nid):
    if nid in memo:
        return memo[nid]
    x = nodes[nid]
    t = hits.get(nid, 0) + sum(incl(c) for c in x.get("children", []))
    memo[nid] = t
    return t


sys.setrecursionlimit(100000)
callee = Counter()
own = 0
tot = 0
for nid, x in nodes.items():
    if not pat.search(x["callFrame"]["functionName"]):
        continue
    # skip if an ancestor matches too (count outermost only)
    tot += incl(nid)
    own += hits.get(nid, 0)
    for c in x.get("children", []):
        callee[name(nodes[c])] += incl(c)
print(f"total {100 * tot / total:.2f}%  self {100 * own / total:.2f}%")
for k, v in callee.most_common(n):
    print(f"{100 * v / total:6.2f}% {k}")
