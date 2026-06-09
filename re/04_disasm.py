#!/usr/bin/env python3
"""Désassembleur ARM/Thumb annoté pour libnickel.so (capstone).
Usage: 04_disasm.py <addr_hex> [size]   (addr avec bit Thumb optionnel)
       04_disasm.py name "WebRequester::sendRequest"  (recherche par démanglé)
Annote: cibles BL/B -> symbole, LDR pc-relatif -> valeur/symbole/string .rodata.
"""
import sys, struct, re
from elftools.elf.elffile import ELFFile
from capstone import Cs, CS_ARCH_ARM, CS_MODE_ARM, CS_MODE_THUMB, CS_MODE_LITTLE_ENDIAN

BIN="/home/maelle/fw/libnickel.so"
f=open(BIN,'rb'); elf=ELFFile(f)
segs=[(s['p_vaddr'],s['p_offset'],s['p_filesz']) for s in elf.iter_segments() if s['p_type']=='PT_LOAD']
def v2o(va):
    for vaddr,off,sz in segs:
        if vaddr<=va<vaddr+sz: return off+(va-vaddr)
    return None
def read(va,n):
    o=v2o(va)
    if o is None: return None
    f.seek(o); return f.read(n)
def read32(va):
    d=read(va,4); return struct.unpack('<I',d)[0] if d else None

# sections rodata range
rodata=None
for s in elf.iter_sections():
    if s.name=='.rodata': rodata=(s['sh_addr'],s['sh_addr']+s['sh_size'])
def get_string(va,maxlen=120):
    o=v2o(va)
    if o is None: return None
    f.seek(o); raw=f.read(maxlen)
    end=raw.find(b'\0')
    if end<=0: return None
    s=raw[:end]
    try: txt=s.decode('utf-8')
    except: return None
    if all(32<=ord(c)<127 or c in '\t' for c in txt) and len(txt)>=2: return txt
    return None

# symboles: addr->name (FUNC), et tous pour data
dynsym=elf.get_section_by_name('.dynsym')
addr2func={}; addr2any={}
name2sym=[]
for s in dynsym.iter_symbols():
    if not s.name: continue
    v=s['st_value']; t=str(s['st_info']['type'])
    if t=='STT_FUNC': addr2func.setdefault(v&~1,s.name)
    addr2any.setdefault(v&~1,s.name)
    name2sym.append((s.name,v,s['st_size'],t))

def find_by_demangled(query):
    import subprocess
    names=[n for n,_,_,_ in name2sym]
    dm=subprocess.run(["c++filt"],input="\n".join(names),capture_output=True,text=True).stdout.splitlines()
    for (n,v,sz,t),d in zip(name2sym,dm):
        if t=='STT_FUNC' and query in d:
            return v,sz,n,d
    return None

# entrée
if sys.argv[1]=='name':
    res=find_by_demangled(sys.argv[2])
    if not res: print("introuvable"); sys.exit(1)
    val,size,mang,dem=res
    print(f"// {dem}\n// {mang} @ {val:#x} size={size}")
    addr=val; size=size or 64
else:
    addr=int(sys.argv[1],16)
    size=int(sys.argv[2]) if len(sys.argv)>2 else 64

thumb = bool(addr & 1)
addr &= ~1
mode = (CS_MODE_THUMB if thumb else CS_MODE_ARM) | CS_MODE_LITTLE_ENDIAN
md=Cs(CS_ARCH_ARM, mode); md.detail=True
code=read(addr,size)
print(f"// mode={'THUMB' if thumb else 'ARM'} @ {addr:#x} ({size} octets)\n")

for ins in md.disasm(code, addr):
    line=f"  {ins.address:#08x}:  {ins.mnemonic:<8} {ins.op_str}"
    ann=[]
    # cibles de branche
    if ins.mnemonic.startswith(('bl','b','bx','blx','cbz','cbnz')) and ins.operands:
        for op in ins.operands:
            if op.type==1: # IMM
                tgt=op.imm
                nm=addr2func.get(tgt&~1) or addr2any.get(tgt&~1)
                if nm: ann.append(f"-> {nm}")
    # LDR pc-relatif (literal pool)
    m=re.search(r'\[pc, #(-?\d+)\]', ins.op_str) or re.search(r'\[pc, #0x([0-9a-f]+)\]', ins.op_str)
    if 'ldr' in ins.mnemonic and ('pc' in ins.op_str):
        # capstone fournit souvent l'adresse résolue dans op.mem ; calc manuel:
        mm=re.search(r'#(-?(?:0x)?[0-9a-fA-F]+)', ins.op_str.split('pc')[1]) if 'pc' in ins.op_str else None
        try:
            imm=int(mm.group(1),0) if mm else 0
            base=(ins.address+ (4 if thumb else 8)) & ~3
            lit=base+imm
            val=read32(lit)
            if val is not None:
                nm=addr2any.get(val&~1)
                st=get_string(val)
                if nm: ann.append(f"lit={val:#x} ({nm})")
                elif st is not None: ann.append(f'lit={val:#x} "{st}"')
                else: ann.append(f"lit={val:#x}")
        except Exception: pass
    if ann: line+= "    ; "+" ".join(ann)
    print(line)
