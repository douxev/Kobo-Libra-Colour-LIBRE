#!/usr/bin/env python3
"""Récupère les types de retour (absents du mangling Itanium) depuis les
chaînes Q_FUNC_INFO/__PRETTY_FUNCTION__ embarquées dans .rodata.
Sortie: re/out/funcinfo.tsv  (qualname \t argcount \t rettype \t signature complète)
Puis enrichit les headers existants avec les types de retour récupérés.
"""
import re, subprocess, os, glob
from elftools.elf.elffile import ELFFile

BIN="/home/maelle/fw/libnickel.so"; OUT="/home/maelle/fw/re/out"

# 1) Extraire les chaînes de .rodata
out=subprocess.run(["strings","-n","8",BIN],capture_output=True,text=True).stdout.splitlines()
SIG=re.compile(r'^(?:(static|virtual)\s+)?(.+?)\s+([A-Za-z_]\w*(?:<[^()]*>)?(?:::[~A-Za-z_]\w*(?:<[^()]*>)?)+)\((.*)\)(\s+const)?$')

def depth_split_last_space(s):
    """Sépare 'rettype qualname' au dernier espace de profondeur 0 (hors <>)."""
    depth=0; idx=-1
    for i,ch in enumerate(s):
        if ch=='<': depth+=1
        elif ch=='>': depth-=1
        elif ch==' ' and depth==0: idx=i
    return (s[:idx], s[idx+1:]) if idx>=0 else (None,s)

def argcount(args):
    args=args.strip()
    if args=='' or args=='void': return 0
    depth=0; n=1
    for ch in args:
        if ch in '<([{': depth+=1
        elif ch in '>)]}': depth-=1
        elif ch==',' and depth==0: n+=1
    return n

records={}   # (qualname, argc) -> (rettype, fullsig)
allsig=[]
for line in out:
    line=line.strip()
    if '::' not in line or '(' not in line: continue
    m=SIG.match(line)
    if not m: continue
    storage, pre, qual, args, isconst = m.groups()
    # pre = "rettype" potentiellement avec qualname collé si regex a sur-capturé : re-split
    # ici 'qual' est déjà le nom qualifié ; 'pre' = type de retour
    rettype=pre.strip()
    if not rettype or rettype in ('return','else','const'): continue
    if any(c in rettype for c in ';{}='): continue
    ac=argcount(args)
    full=f"{(storage+' ') if storage else ''}{rettype} {qual}({args}){isconst or ''}"
    records[(qual,ac)]=(rettype, storage or '', isconst.strip() if isconst else '', full)
    allsig.append(full)

os.makedirs(OUT,exist_ok=True)
with open(f"{OUT}/funcinfo.tsv","w") as f:
    f.write("qualname\targc\tstorage\trettype\tconst\tfull\n")
    for (qual,ac),(rt,st,cst,full) in sorted(records.items()):
        f.write(f"{qual}\t{ac}\t{st}\t{rt}\t{cst}\t{full}\n")
print(f"[*] {len(records)} signatures uniques récupérées (avec type de retour)")

# map nom_methode_qualifié -> rettype (si non ambigu sur argc on prend quand même)
ret_by_qual={}
for (qual,ac),(rt,st,cst,full) in records.items():
    ret_by_qual.setdefault(qual, rt)

# 2) Enrichir les headers .hpp : pour chaque ligne "  sig;" tenter d'ajouter le rettype
def enrich(cls, path):
    lines=open(path).read().splitlines()
    out=[]
    for L in lines:
        s=L.strip()
        mm=re.match(r'^((?:virtual\s+)?)([~A-Za-z_]\w*(?:<.*>)?)\((.*)\)(\s*(?:const)?);$', s)
        if mm and not s.startswith('//'):
            virt, meth, args, cst = mm.groups()
            qual=f"{cls}::{meth}"
            rt=ret_by_qual.get(qual)
            if rt:
                indent=L[:len(L)-len(L.lstrip())]
                out.append(f"{indent}{virt}{rt} {meth}({args}){cst};   // [rettype récupéré]")
                continue
        out.append(L)
    open(path,"w").write("\n".join(out)+"\n")

cnt=0
for path in glob.glob(f"{OUT}/headers/*.hpp"):
    cls=os.path.basename(path)[:-4]
    before=open(path).read()
    enrich(cls, path)
    after=open(path).read()
    cnt+= after.count("[rettype récupéré]")
print(f"[*] types de retour injectés dans les headers: {cnt} méthodes")
# top classes couvertes
from collections import Counter
c=Counter(q.split('::')[0] for (q,a) in records)
print("=== Top classes avec signatures embarquées ===")
for k,v in c.most_common(15): print(f"  {v:4d}  {k}")
print(f"[OK] {OUT}/funcinfo.tsv")
