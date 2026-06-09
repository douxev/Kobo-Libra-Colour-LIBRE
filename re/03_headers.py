#!/usr/bin/env python3
"""Reconstruction de headers C++ (interfaces) depuis symboles + RTTI + vtables.
- Index méthodes par classe (toutes les classes) -> out/methods_by_class.tsv
- Headers .hpp reconstruits pour une liste ciblée, avec virtuals ordonnés par vtable.
"""
import sys, os, struct, subprocess, re, json
from collections import defaultdict, OrderedDict
from elftools.elf.elffile import ELFFile
from elftools.elf.relocation import RelocationSection

BIN = "/home/maelle/fw/libnickel.so"
OUT = "/home/maelle/fw/re/out"
os.makedirs(f"{OUT}/headers", exist_ok=True)

f = open(BIN,'rb'); elf = ELFFile(f)
segs=[(s['p_vaddr'],s['p_offset'],s['p_filesz']) for s in elf.iter_segments() if s['p_type']=='PT_LOAD']
def v2o(va):
    for vaddr,off,sz in segs:
        if vaddr<=va<vaddr+sz: return off+(va-vaddr)
    return None
def read32(va):
    o=v2o(va)
    if o is None: return None
    f.seek(o); return struct.unpack('<I',f.read(4))[0]

dynsym=elf.get_section_by_name('.dynsym')
addr2name={}; name2addr={}
funcs_by_addr={}
for s in dynsym.iter_symbols():
    if not s.name: continue
    v=s['st_value']; t=str(s['st_info']['type'])
    name2addr[s.name]=v
    if t=='STT_FUNC':
        funcs_by_addr[v & ~1]=s.name
    addr2name.setdefault(v & ~1, s.name)

# reloc map offset->target_addr (pour vtables)
reloc_target={}
for sec in elf.iter_sections():
    if not isinstance(sec,RelocationSection): continue
    symtab=elf.get_section(sec['sh_link'])
    for r in sec.iter_relocations():
        off=r['r_offset']; symidx=r['r_info_sym']
        inplace=read32(off)
        if inplace is None: continue
        if symidx!=0:
            sym=symtab.get_symbol(symidx)
            reloc_target[off]=(sym['st_value']+inplace)&0xffffffff
        else:
            reloc_target[off]=inplace

# Charger symbols.tsv (déjà démanglé) pour grouper méthodes par classe
funcs=[]  # (addr, thumb, mangled, demangled)
with open(f"{OUT}/symbols.tsv") as fh:
    next(fh)
    for line in fh:
        p=line.rstrip("\n").split("\t")
        if len(p)<8: continue
        addr,thumb,size,typ,bind,shndx,mang,dem=p
        if typ!='FUNC' or shndx=='SHN_UNDEF': continue
        funcs.append((int(addr,16),thumb=='1',mang,dem))

# Extraire "classe::methode(...)" -> classe qualifiée. On parse le démanglé.
# Le nom de classe = tout avant le dernier "::" du nom de la fonction (hors args).
def split_class_method(dem):
    # retirer la signature d'arguments pour localiser le bon '::'
    # on coupe à la 1ère '(' de profondeur 0
    depth=0; cut=len(dem)
    for i,ch in enumerate(dem):
        if ch in '(<': depth+=1
        elif ch in ')>': depth-=1
        elif ch=='(' : pass
        if ch=='(' and depth==1:  # 1ère parenthèse d'argument top-level... approx
            cut=i; break
    head=dem[:cut]
    # head ex: "WebRequester::makeRequest" ; trouver dernier :: hors <...>
    depth=0; last=-1
    for i in range(len(head)-1):
        ch=head[i]
        if ch=='<': depth+=1
        elif ch=='>': depth-=1
        elif ch==':' and head[i+1]==':' and depth==0:
            last=i
    if last<0: return None,head
    return head[:last], head[last+2:]

methods_by_class=defaultdict(list)
for addr,thumb,mang,dem in funcs:
    if mang.startswith(('_ZTV','_ZTI','_ZTS','_ZTT','_ZGV','_ZTh','_ZTc','_ZTv')): continue
    cls,meth=split_class_method(dem)
    if cls is None: continue
    methods_by_class[cls].append((addr,mang,dem,meth))

# Ecrire index global
with open(f"{OUT}/methods_by_class.tsv","w") as o:
    o.write("class\tn_methods\n")
    for c in sorted(methods_by_class, key=lambda k:-len(methods_by_class[k])):
        o.write(f"{c}\t{len(methods_by_class[c])}\n")
print(f"[*] {len(methods_by_class)} classes avec méthodes ; top:")
for c in sorted(methods_by_class,key=lambda k:-len(methods_by_class[k]))[:12]:
    print(f"    {len(methods_by_class[c]):4d}  {c}")

# Reconstruire l'ordre vtable d'une classe -> liste ordonnée de fonctions virtuelles
def vtable_methods(cls):
    vt=name2addr.get('_ZTV'+mangle_guess(cls)) if False else None
    # On cherche le symbole vtable par son nom démanglé "vtable for <cls>"
    return None

# Charger vtables.tsv (addr, mangled) et mapper cls->vtable addr+size
vt_by_cls={}
with open(f"{OUT}/vtables.tsv") as fh:
    next(fh)
    for line in fh:
        p=line.rstrip("\n").split("\t")
        if len(p)<4: continue
        addr,size,mang,dem=p
        cls=dem.replace('vtable for ','')
        vt_by_cls[cls]=(int(addr,16),int(size))

def vtable_order(cls):
    if cls not in vt_by_cls: return []
    base,size=vt_by_cls[cls]
    out=[]
    # primary vtable: slots à partir de base+8 (offset-to-top, typeinfo)
    a=base+8
    end=base+size if size else base+8+4*200
    while a<end:
        tgt=reloc_target.get(a)
        if tgt is None:
            # peut être un séparateur (offset-to-top secondaire) -> on saute
            v=read32(a)
            if v==0:  # début possible d'une sous-vtable (offset-to-top=0) déjà géré
                a+=4; continue
            a+=4; continue
        name=funcs_by_addr.get(tgt & ~1)
        if name:
            out.append((a-base,tgt,name))
        a+=4
    return out

def demangle_batch(names):
    p=subprocess.run(["c++filt"],input="\n".join(names),capture_output=True,text=True)
    return dict(zip(names,p.stdout.splitlines()))

# Charger héritage
bases_of=defaultdict(list)
with open(f"{OUT}/inheritance.tsv") as fh:
    next(fh)
    for line in fh:
        p=line.rstrip("\n").split("\t")
        if len(p)<4: continue
        c,b,off,kind=p
        bases_of[c].append((b,off,kind))

TARGETS=["WebRequester","OneStoreServiceSettings","TolinoServiceSettings","Settings",
         "SyncStateMachineWorker","SyncStateBase","WebEngineRenderer","Device",
         "DeviceDiscoverer","ConfigurationRequest","WebResponseInflater","Request",
         "WirelessWorkflowManager","WirelessManager","NetworkAccessManager",
         "GoogleAnalyticsRequester","GoogleAnalyticsHandler","AnalyticsEventManager",
         "UpgradeCheckCommand","SyncClient","N3SyncManager"]

def gen_header(cls):
    lines=[]
    bs=bases_of.get(cls,[])
    base_str=" : "+", ".join(f"public {b}" for b,_,_ in bs) if bs else ""
    lines.append(f"// === Reconstruit depuis symboles + RTTI + vtable — interface, PAS layout mémoire ===")
    lines.append(f"class {cls}{base_str} {{")
    # virtuals ordonnés
    vorder=vtable_order(cls)
    if vorder:
        names=[n for _,_,n in vorder]
        dm=demangle_batch(names)
        lines.append("  // ---- table virtuelle (ordre exact) ----")
        seen=set()
        for slot,tgt,n in vorder:
            d=dm.get(n,n)
            if n in seen: continue
            seen.add(n)
            lines.append(f"  virtual {d.replace(cls+'::','')};   // vt+{slot:#x}")
    # méthodes non-virtuelles (toutes les méthodes connues moins celles déjà listées)
    meths=methods_by_class.get(cls,[])
    vnames={n for _,_,n in vorder}
    nonv=[(dem,meth) for addr,mang,dem,meth in sorted(meths) if mang not in vnames]
    if nonv:
        lines.append("  // ---- autres membres (statiques/non-virtuels/ctors) ----")
        seen=set()
        for dem,meth in nonv:
            sig=dem.replace(cls+'::','')
            if sig in seen: continue
            seen.add(sig)
            lines.append(f"  {sig};")
    lines.append("};")
    return "\n".join(lines)

for t in TARGETS:
    if t not in methods_by_class and t not in vt_by_cls:
        print(f"  [skip] {t} (aucune méthode/vtable)"); continue
    h=gen_header(t)
    with open(f"{OUT}/headers/{t}.hpp","w") as o:
        o.write(h+"\n")
print(f"\n[OK] headers reconstruits dans {OUT}/headers/ pour: {', '.join(TARGETS)}")
