#!/usr/bin/env python3
"""Pour chaque fonction cible (targets.tsv), désassemble la plage exacte avec capstone
et collecte TOUTES les chaînes .rodata résolues (idiome PIC ldr+add pc, et literal direct)
+ les cibles d'appel (fonctions internes / imports PLT). Sortie: re/out/endpoints.md
"""
import struct, re, subprocess
from elftools.elf.elffile import ELFFile
from elftools.elf.relocation import RelocationSection
from capstone import *
from capstone.arm import ARM_OP_IMM, ARM_OP_REG

BIN="/home/maelle/fw/libnickel.so"; OUT="/home/maelle/fw/re/out"
f=open(BIN,'rb'); elf=ELFFile(f)
segs=[(s['p_vaddr'],s['p_offset'],s['p_filesz']) for s in elf.iter_segments() if s['p_type']=='PT_LOAD']
def v2o(va):
    for vaddr,off,sz in segs:
        if vaddr<=va<vaddr+sz: return off+(va-vaddr)
    return None
def read(va,n):
    o=v2o(va);
    if o is None: return None
    f.seek(o); return f.read(n)
def read32(va):
    d=read(va,4); return struct.unpack('<I',d)[0] if d else None
def get_string(va,maxlen=200):
    o=v2o(va)
    if o is None: return None
    f.seek(o); raw=f.read(maxlen); e=raw.find(b'\0')
    if e<=0: return None
    try: t=raw[:e].decode('utf-8')
    except: return None
    return t if (len(t)>=3 and all(32<=ord(c)<127 or c=='\t' for c in t)) else None

dynsym=elf.get_section_by_name('.dynsym')
addr2func={}; addr2any={}
for s in dynsym.iter_symbols():
    if not s.name: continue
    v=s['st_value']; t=str(s['st_info']['type'])
    if t=='STT_FUNC': addr2func.setdefault(v&~1,s.name)
    addr2any.setdefault(v&~1,s.name)
got2sym={}
for sec in elf.iter_sections():
    if isinstance(sec,RelocationSection):
        st=elf.get_section(sec['sh_link'])
        for r in sec.iter_relocations():
            if r['r_info_sym']:
                nm=st.get_symbol(r['r_info_sym']).name
                if nm: got2sym[r['r_offset']]=nm
plt_lo=plt_hi=None
for s in elf.iter_sections():
    if s.name=='.plt': plt_lo,plt_hi=s['sh_addr'],s['sh_addr']+s['sh_size']
_arm=Cs(CS_ARCH_ARM,CS_MODE_ARM|CS_MODE_LITTLE_ENDIAN); _arm.detail=True
def resolve_plt(target):
    if plt_lo is None or not(plt_lo<=target<plt_hi): return None
    code=read(target&~1,16)
    if not code: return None
    ip=None
    for ins in _arm.disasm(code,target&~1):
        ops=ins.operands
        if ins.mnemonic.startswith('add'):
            src=(ins.address+8) if (len(ops)>=2 and ops[1].type==ARM_OP_REG and ins.reg_name(ops[1].reg)=='pc') else (ip or 0)
            imm=ops[-1].imm if ops and ops[-1].type==ARM_OP_IMM else 0
            ip=src+imm
        elif ins.mnemonic.startswith('ldr'):
            mm=re.search(r'#(-?(?:0x)?[0-9a-fA-F]+)',ins.op_str); imm=int(mm.group(1),0) if mm else 0
            return got2sym.get((ip or 0)+imm)
    return None

def demangle(names):
    p=subprocess.run(["c++filt"],input="\n".join(names),capture_output=True,text=True)
    return dict(zip(names,p.stdout.splitlines()))

def scan(va, size, thumb):
    md=Cs(CS_ARCH_ARM,(CS_MODE_THUMB if thumb else CS_MODE_ARM)|CS_MODE_LITTLE_ENDIAN); md.detail=True
    code=read(va,size); pc=4 if thumb else 8
    strings=[]; calls=[]; litregs={}
    for ins in md.disasm(code,va):
        ops=ins.operands
        if ins.mnemonic.startswith(('bl','blx')) and ops and ops[0].type==ARM_OP_IMM:
            tgt=ops[0].imm; nm=addr2func.get(tgt&~1) or resolve_plt(tgt)
            if nm: calls.append(nm)
        if ins.mnemonic.startswith('ldr') and 'pc' in ins.op_str and ops and ops[0].type==ARM_OP_REG:
            mm=re.search(r'#(-?(?:0x)?[0-9a-fA-F]+)',ins.op_str.split('pc',1)[1]); imm=int(mm.group(1),0) if mm else 0
            lit=read32(((ins.address+pc)&~3)+imm)
            if lit is not None:
                rn=ins.reg_name(ops[0].reg); litregs[rn]=lit
                st=get_string(lit)
                if st: strings.append(st)
        if ins.mnemonic.startswith('add') and re.search(r'\bpc\b',ins.op_str) and ops and ops[0].type==ARM_OP_REG:
            rn=ins.reg_name(ops[0].reg)
            if rn in litregs:
                ab=((ins.address+pc)+litregs[rn])&0xffffffff
                st=get_string(ab)
                if st: strings.append(st)
    return strings, calls

# charger cibles
rows=[]
for line in open(f"{OUT}/targets.tsv"):
    p=line.rstrip("\n").split("\t")
    if len(p)<4: continue
    raw=int(p[0],16); rows.append((p[0],int(p[1]),raw&1==1,raw&~1,p[3]))

allcalls=set()
out=["# Endpoints & appels par fonction (capstone, vérité terrain)\n"]
for addrS,size,thumb,va,dem in rows:
    strs,calls=scan(va,size,thumb)
    allcalls.update(calls)
    out.append(f"\n## {dem}\n`{addrS}` size={size} thumb={thumb}\n")
    if strs:
        out.append("**Chaînes .rodata résolues :**\n")
        for s in dict.fromkeys(strs): out.append(f"- `{s}`")
    icalls=[c for c in dict.fromkeys(calls)]
    if icalls:
        dm=demangle(icalls)
        out.append("\n**Appels (résolus) :**")
        for c in icalls[:40]:
            out.append(f"- {dm.get(c,c)}")
open(f"{OUT}/endpoints.md","w").write("\n".join(out))
print(f"[OK] {OUT}/endpoints.md")
# Aperçu: chaînes parlantes globales
print("\n=== Chaînes parlantes (réseau) trouvées ===")
for addrS,size,thumb,va,dem in rows:
    strs,_=scan(va,size,thumb)
    juicy=[s for s in strs if re.search(r'kobo|google|analyt|rakuten|http|\.com|\.jp|/v1|connectivity|config|client|UA-|collect|reading|oauth|hostname|connman|gateway|generate_204|captive',s,re.I)]
    if juicy:
        print(f"\n{dem}  [{addrS}]")
        for s in dict.fromkeys(juicy): print(f"   {s!r}")
