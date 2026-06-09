#!/usr/bin/env python3
"""Chaînes d'ascendance (héritage jusqu'aux racines) + arbre du sous-système réseau."""
import os
from collections import defaultdict
OUT="/home/maelle/fw/re/out"
bases=defaultdict(list); children=defaultdict(list)
with open(f"{OUT}/inheritance.tsv") as f:
    next(f)
    for line in f:
        p=line.rstrip("\n").split("\t")
        if len(p)<4: continue
        c,b,off,kind=p
        bases[c].append(b); children[b].append(c)

def ancestry(cls, seen=None, depth=0):
    seen=seen or set()
    if cls in seen or depth>12: return []
    seen.add(cls)
    out=[(depth,cls)]
    for b in bases.get(cls,[]):
        out+=ancestry(b,seen,depth+1)
    return out

KEY=["WirelessWorkflowManager","WirelessManager","NetworkAccessManager","WebRequester",
     "GoogleAnalyticsRequester","GoogleAnalyticsHandler","SyncStateMachineWorker",
     "OneStoreServiceSettings","TolinoServiceSettings","WebEngineRenderer","SyncClient"]
with open(f"{OUT}/ancestry.txt","w") as f:
    for k in KEY:
        f.write(f"\n### {k}\n")
        for d,c in ancestry(k):
            f.write("    "*d + ("└─ " if d else "") + c + "\n")
print(open(f"{OUT}/ancestry.txt").read())
