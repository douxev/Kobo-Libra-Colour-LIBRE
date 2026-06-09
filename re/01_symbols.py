#!/usr/bin/env python3
"""Extraction & classification de la table de symboles dynamiques de libnickel.so.
Sortie: re/out/ - symbols.tsv, classes.txt, vtables.tsv, stats.
Aucune dépendance réseau. Utilise pyelftools + c++filt (batch)."""
import subprocess, sys, os, json, re
from collections import defaultdict
from elftools.elf.elffile import ELFFile

BIN = sys.argv[1] if len(sys.argv) > 1 else "/home/maelle/fw/libnickel.so"
OUT = "/home/maelle/fw/re/out"
os.makedirs(OUT, exist_ok=True)

def norm_type(t):  # pyelftools donne 'STT_FUNC', 'STT_OBJECT', ...
    return str(t).replace('STT_','')
def norm_bind(b):
    return str(b).replace('STB_','')
def is_undef(sh):
    return sh == 'SHN_UNDEF' or sh == 0

def batch_demangle(names):
    """Demangle en masse via c++filt (un appel)."""
    p = subprocess.run(["c++filt"], input="\n".join(names), capture_output=True, text=True)
    return p.stdout.splitlines()

print(f"[*] Lecture {BIN}")
with open(BIN,'rb') as f:
    elf = ELFFile(f)
    dynsym = elf.get_section_by_name('.dynsym')
    syms = []
    for s in dynsym.iter_symbols():
        if not s.name: continue
        info = s['st_info']
        typ = norm_type(info['type'])
        val = s['st_value']
        syms.append({
            'name': s.name,
            'addr': val,
            'thumb': bool(val & 1) if typ=='FUNC' else False,
            'size': s['st_size'],
            'type': typ,
            'bind': norm_bind(info['bind']),
            'shndx': s['st_shndx'],
        })

print(f"[*] {len(syms)} symboles nommés")
mangled = [s['name'] for s in syms]
demangled = batch_demangle(mangled)
for s,d in zip(syms, demangled):
    s['demangled'] = d

# Classification
vtables = [s for s in syms if s['name'].startswith('_ZTV')]
typeinfo = [s for s in syms if s['name'].startswith('_ZTI')]
typename = [s for s in syms if s['name'].startswith('_ZTS')]
funcs = [s for s in syms if s['type']=='FUNC' and not is_undef(s['shndx'])]
undef = [s for s in syms if is_undef(s['shndx'])]
objects = [s for s in syms if s['type']=='OBJECT' and not is_undef(s['shndx'])]

# Extraction du nom de classe depuis un vtable mang_: _ZTVNxxxE -> classe
def cls_from_vtable(d):
    # d demangled = "vtable for X::Y"
    m = re.match(r'vtable for (.+)', d)
    return m.group(1) if m else None

classes = sorted({c for c in (cls_from_vtable(s['demangled']) for s in vtables) if c})

# Top-level namespaces / class roots (heuristique sur demangled funcs)
ns = defaultdict(int)
for s in funcs:
    d = s['demangled']
    m = re.match(r'(?:virtual\s+)?[\w:<>~ ,\*&\[\]]*?\b(\w+)::', d)
    # racine = premier segment ::-séparé du nom qualifié
    m2 = re.search(r'\b([A-Za-z_]\w*)::', d)
    if m2: ns[m2.group(1)] += 1

# Ecriture
with open(f"{OUT}/symbols.tsv","w") as f:
    f.write("addr\tthumb\tsize\ttype\tbind\tshndx\tmangled\tdemangled\n")
    for s in sorted(syms, key=lambda x:(x['addr'])):
        f.write(f"{s['addr']:#x}\t{int(s['thumb'])}\t{s['size']}\t{s['type']}\t{s['bind']}\t{s['shndx']}\t{s['name']}\t{s['demangled']}\n")

with open(f"{OUT}/classes.txt","w") as f:
    f.write("\n".join(classes))

with open(f"{OUT}/vtables.tsv","w") as f:
    f.write("addr\tsize\tmangled\tdemangled\n")
    for s in sorted(vtables, key=lambda x:x['addr']):
        f.write(f"{s['addr']:#x}\t{s['size']}\t{s['name']}\t{s['demangled']}\n")

top_ns = sorted(ns.items(), key=lambda x:-x[1])
with open(f"{OUT}/namespaces.tsv","w") as f:
    f.write("root\tfunc_count\n")
    for k,v in top_ns:
        f.write(f"{k}\t{v}\n")

print("=== STATS ===")
print(f"  symboles totaux : {len(syms)}")
print(f"  FUNC définis    : {len(funcs)}")
print(f"  OBJECT définis  : {len(objects)}")
print(f"  UNDEF (imports) : {len(undef)}")
print(f"  vtables (_ZTV)  : {len(vtables)}")
print(f"  typeinfo(_ZTI)  : {len(typeinfo)}")
print(f"  classes (via vtable) : {len(classes)}")
print(f"  fonctions Thumb : {sum(1 for s in funcs if s['thumb'])} / ARM : {sum(1 for s in funcs if not s['thumb'])}")
print("\n=== TOP 30 racines (classes/namespaces par nb de fonctions) ===")
for k,v in top_ns[:30]:
    print(f"  {v:6d}  {k}")
print(f"\n[OK] écrits dans {OUT}/")
