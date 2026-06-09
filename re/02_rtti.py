#!/usr/bin/env python3
"""Reconstruction du graphe d'héritage C++ depuis le RTTI (Itanium ABI).
Parse les typeinfo (_ZTI*) dans .data.rel.ro + relocations .rel.dyn pour
récupérer kind (no-base / single / multi) et classes de base + offsets.
Sortie: re/out/inheritance.tsv
"""
import sys, os, struct, subprocess
from elftools.elf.elffile import ELFFile
from elftools.elf.relocation import RelocationSection

BIN = "/home/maelle/fw/libnickel.so"
OUT = "/home/maelle/fw/re/out"

f = open(BIN,'rb'); elf = ELFFile(f)

# vaddr -> file offset (via segments PT_LOAD)
segs = []
for seg in elf.iter_segments():
    if seg['p_type']=='PT_LOAD':
        segs.append((seg['p_vaddr'], seg['p_offset'], seg['p_filesz']))
def v2o(va):
    for vaddr,off,sz in segs:
        if vaddr <= va < vaddr+sz:
            return off + (va-vaddr)
    return None
def read32(va):
    o = v2o(va)
    if o is None: return None
    f.seek(o); return struct.unpack('<I', f.read(4))[0]

# Index des symboles dynamiques: addr -> name, name -> addr
dynsym = elf.get_section_by_name('.dynsym')
addr2name = {}
name2addr = {}
ti_addrs = {}   # addr -> name pour _ZTI*
for s in dynsym.iter_symbols():
    if not s.name: continue
    v = s['st_value']
    addr2name.setdefault(v & ~1, s.name)
    name2addr[s.name] = v
    if s.name.startswith('_ZTI'):
        ti_addrs[v] = s.name

# Les 3 vtables de cxxabi qui déterminent le "kind"
KIND_VT = {
 '_ZTVN10__cxxabiv117__class_type_infoE':'no_base',
 '_ZTVN10__cxxabiv120__si_class_type_infoE':'single',
 '_ZTVN10__cxxabiv121__vmi_class_type_infoE':'multi',
}
kind_vt_addr = {}
for n in KIND_VT:
    if n in name2addr:
        # le pointeur dans typeinfo pointe vt+8 (après les 2 premiers mots ABI)
        kind_vt_addr[name2addr[n]+8] = KIND_VT[n]
        kind_vt_addr[name2addr[n]] = KIND_VT[n]  # tolérance

# Construire une map: offset_reloc -> valeur cible résolue
# ARM REL: addend in-place. Pour ABS32 avec symbole -> sym+inplace ; RELATIVE -> inplace.
reloc_target = {}   # offset -> (target_vaddr, sym_name|None)
for sec in elf.iter_sections():
    if not isinstance(sec, RelocationSection): continue
    symtab = elf.get_section(sec['sh_link'])
    for r in sec.iter_relocations():
        off = r['r_offset']
        rtype = r['r_info_type']
        symidx = r['r_info_sym']
        inplace = read32(off)
        if inplace is None: continue
        sname = None; target = None
        if symidx != 0:
            sym = symtab.get_symbol(symidx)
            sname = sym.name
            target = (sym['st_value'] + inplace) & 0xffffffff
        else:
            target = inplace  # R_ARM_RELATIVE etc.
        reloc_target[off] = (target, sname)

print(f"[*] {len(ti_addrs)} typeinfo, {len(reloc_target)} relocations indexées")

# Pour chaque typeinfo, lire sa structure
# layout: [+0]=ptr vers vtable de la classe-typeinfo (kind), [+4]=ptr name (_ZTS),
#  single: [+8]=base typeinfo
#  multi:  [+8]=flags(u32),[+12]=base_count(u32),[+16..]= (base_ti_ptr, offset_flags) *count
def demangle(names):
    p = subprocess.run(["c++filt"], input="\n".join(names), capture_output=True, text=True)
    return dict(zip(names, p.stdout.splitlines()))

edges = []   # (class_ti, base_ti, offset, kind)
kinds = {}
for ti_addr, ti_name in ti_addrs.items():
    if v2o(ti_addr) is None: continue
    # kind via reloc à ti_addr+0
    rt = reloc_target.get(ti_addr)
    kind = None
    if rt:
        tgt, sn = rt
        if sn in KIND_VT:
            kind = KIND_VT[sn]
        elif tgt in kind_vt_addr:
            kind = kind_vt_addr[tgt]
    kinds[ti_name] = kind or 'unknown'
    if kind == 'single':
        rt2 = reloc_target.get(ti_addr+8)
        if rt2:
            btgt, bsn = rt2
            bname = bsn if (bsn and bsn.startswith('_ZTI')) else ti_addrs.get(btgt)
            if bname: edges.append((ti_name, bname, 0, 'single'))
    elif kind == 'multi':
        base_count = read32(ti_addr+12) or 0
        if base_count > 64: base_count = 0  # garde-fou
        for i in range(base_count):
            bptr_off = ti_addr+16+i*8
            rt2 = reloc_target.get(bptr_off)
            offflags = read32(ti_addr+16+i*8+4) or 0
            voff = offflags >> 8   # offset du sous-objet de base
            if rt2:
                btgt, bsn = rt2
                bname = bsn if (bsn and bsn.startswith('_ZTI')) else ti_addrs.get(btgt)
                if bname: edges.append((ti_name, bname, voff, 'multi'))

# Demangle pour lisibilité (_ZTIxxx -> "typeinfo for X" -> X)
allnames = set()
for c,b,o,k in edges: allnames.add(c); allnames.add(b)
for n in kinds: allnames.add(n)
dm = demangle(sorted(allnames))
def cls(ti):  # _ZTI... -> nom de classe
    d = dm.get(ti, ti)
    return d.replace('typeinfo for ','') if d.startswith('typeinfo for ') else d

os.makedirs(OUT, exist_ok=True)
with open(f"{OUT}/inheritance.tsv","w") as out:
    out.write("class\tbase\toffset\tkind\n")
    for c,b,o,k in sorted(edges, key=lambda e: cls(e[0])):
        out.write(f"{cls(c)}\t{cls(b)}\t{o}\t{k}\n")

from collections import Counter
kc = Counter(kinds.values())
nbases = Counter()
for c,b,o,k in edges: nbases[c]+=1
print("=== RTTI kinds ===")
for k,v in kc.most_common(): print(f"  {k:10s} {v}")
print(f"=== {len(edges)} arêtes d'héritage ; {sum(1 for v in nbases.values() if v>1)} classes à héritage multiple ===")
# exemples réseau/store
print("\n=== Exemples (classes contenant Web/Store/Service/Sync) ===")
seen=set()
for c,b,o,k in sorted(edges, key=lambda e:cls(e[0])):
    cc=cls(c)
    if any(t in cc for t in ('WebRequester','OneStore','TolinoService','SyncState','Device','WebEngineRenderer')) and cc not in seen:
        bases=[cls(bb) for (xc,bb,oo,kk) in edges if xc==c]
        print(f"  {cc}  :  {', '.join(bases)}  [{kinds.get(c)}]")
        seen.add(cc)
print(f"\n[OK] {OUT}/inheritance.tsv")
