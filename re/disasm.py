#!/usr/bin/env python3
"""Désassembleur ARM/Thumb annoté v2 pour libnickel.so.
Résout: BL/BLX -> fonction interne OU import PLT (via GOT/.rel.plt),
        idiome 'ldr rX,[pc,#i] ; add rX,pc' -> adresse absolue -> string/symbole.
Usage: disasm.py <addr_hex[+thumb]> [size]
       disasm.py name "<sous-chaîne démanglée>"
"""
import sys, struct, re, subprocess
from elftools.elf.elffile import ELFFile
from elftools.elf.relocation import RelocationSection
from capstone import *
from capstone.arm import ARM_OP_IMM, ARM_OP_REG, ARM_OP_MEM, ARM_REG_PC

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
def get_string(va,maxlen=160):
    o=v2o(va)
    if o is None: return None
    f.seek(o); raw=f.read(maxlen)
    end=raw.find(b'\0')
    if end<=0: return None
    try: txt=raw[:end].decode('utf-8')
    except: return None
    if len(txt)>=2 and all(32<=ord(c)<127 or c=='\t' for c in txt): return txt
    return None

dynsym=elf.get_section_by_name('.dynsym')
addr2func={}; addr2any={}; name2sym=[]
for s in dynsym.iter_symbols():
    if not s.name: continue
    v=s['st_value']; t=str(s['st_info']['type'])
    if t=='STT_FUNC': addr2func.setdefault(v&~1,s.name)
    addr2any.setdefault(v&~1,s.name)
    name2sym.append((s.name,v,s['st_size'],t))

# GOT slot vaddr -> symbole importé (via .rel.plt et .rel.dyn GLOB_DAT/JUMP_SLOT)
got2sym={}
for sec in elf.iter_sections():
    if not isinstance(sec,RelocationSection): continue
    symtab=elf.get_section(sec['sh_link'])
    for r in sec.iter_relocations():
        si=r['r_info_sym']
        if si==0: continue
        nm=symtab.get_symbol(si).name
        if nm: got2sym[r['r_offset']]=nm

plt_lo, plt_hi = None, None
for s in elf.iter_sections():
    if s.name=='.plt': plt_lo,plt_hi=s['sh_addr'],s['sh_addr']+s['sh_size']

_md_arm=Cs(CS_ARCH_ARM,CS_MODE_ARM|CS_MODE_LITTLE_ENDIAN); _md_arm.detail=True
def resolve_plt(target):
    """Emule un stub PLT ARM pour retrouver le slot GOT puis le symbole importé."""
    if plt_lo is None or not (plt_lo<=target<plt_hi): return None
    code=read(target&~1,16)
    if not code: return None
    ip=None; base=(target&~1)
    for ins in _md_arm.disasm(code, base):
        # add ip, pc, #imm  /  add ip, ip, #imm  /  ldr pc,[ip,#imm]
        if ins.id==0: break
        ops=ins.operands
        if ins.mnemonic.startswith('add') and len(ops)>=2:
            # valeur de pc = addr+8
            acc=ip if ip is not None else 0
            srcpc = (ins.address+8) if (len(ops)>=2 and ops[1].type==ARM_OP_REG and ops[1].reg==ARM_REG_PC) else acc
            imm=ops[-1].imm if ops[-1].type==ARM_OP_IMM else 0
            ip = srcpc + imm
        elif ins.mnemonic.startswith('ldr'):
            # ldr pc, [ip, #imm]!
            mm=re.search(r'#(-?(?:0x)?[0-9a-fA-F]+)', ins.op_str)
            imm=int(mm.group(1),0) if mm else 0
            got=(ip or 0)+imm
            return got2sym.get(got) or got2sym.get(got&0xffffffff)
    return None

# entrée
if sys.argv[1]=='name':
    names=[n for n,_,_,_ in name2sym]
    dm=subprocess.run(["c++filt"],input="\n".join(names),capture_output=True,text=True).stdout.splitlines()
    hit=None
    for (n,v,sz,t),d in zip(name2sym,dm):
        if t=='STT_FUNC' and sys.argv[2] in d: hit=(v,sz,n,d); break
    if not hit: print("introuvable"); sys.exit(1)
    val,size,mang,dem=hit
    print(f"// {dem}\n// {mang} @ {val:#x} size={size}")
    addr=val; size=size or 96
else:
    a=sys.argv[1]; addr=int(a,16); size=int(sys.argv[2]) if len(sys.argv)>2 else 96

thumb=bool(addr&1); addr&=~1
md=Cs(CS_ARCH_ARM,(CS_MODE_THUMB if thumb else CS_MODE_ARM)|CS_MODE_LITTLE_ENDIAN); md.detail=True
code=read(addr,size)
print(f"// mode={'THUMB' if thumb else 'ARM'} @ {addr:#x} ({size}o)\n")
pcoff = 4 if thumb else 8
litregs={}   # reg -> valeur chargée par ldr [pc]
for ins in md.disasm(code,addr):
    line=f"  {ins.address:#08x}:  {ins.mnemonic:<7} {ins.op_str}"
    ann=[]
    ops=ins.operands
    # branches
    if ins.mnemonic.startswith(('bl','blx','b','bx')) and ops and ops[0].type==ARM_OP_IMM:
        tgt=ops[0].imm
        nm=addr2func.get(tgt&~1) or addr2any.get(tgt&~1) or resolve_plt(tgt)
        if nm: ann.append(f"-> {nm}")
    # ldr rX,[pc,#imm]
    mldr=re.match(r'ldr', ins.mnemonic)
    if mldr and 'pc' in ins.op_str and ops and ops[0].type==ARM_OP_REG:
        mm=re.search(r'#(-?(?:0x)?[0-9a-fA-F]+)', ins.op_str.split('pc',1)[1])
        imm=int(mm.group(1),0) if mm else 0
        base=(ins.address+pcoff)&~3
        lit=read32(base+imm)
        rn=ins.reg_name(ops[0].reg)
        if lit is not None:
            litregs[rn]=(lit, ins.address)
            st=get_string(lit); nm=addr2any.get(lit&~1)
            if st is not None: ann.append(f'lit={lit:#x} "{st}"')
            elif nm: ann.append(f"lit={lit:#x} ({nm})")
            else: ann.append(f"lit={lit:#x}")
    # add rX, pc  -> résout l'idiome PIC: adresse = (addr_add+pcoff)+lit
    madd=re.match(r'add', ins.mnemonic)
    if madd and re.search(r'\bpc\b', ins.op_str) and ops and ops[0].type==ARM_OP_REG:
        rn=ins.reg_name(ops[0].reg)
        if rn in litregs:
            lit,_=litregs[rn]
            absaddr=((ins.address+pcoff)+lit)&0xffffffff
            st=get_string(absaddr); nm=addr2any.get(absaddr&~1)
            if st is not None: ann.append(f'=> {absaddr:#x} "{st}"')
            elif nm: ann.append(f"=> {absaddr:#x} ({nm})")
            else: ann.append(f"=> {absaddr:#x}")
    if ann: line+="    ; "+"  ".join(ann)
    print(line)
