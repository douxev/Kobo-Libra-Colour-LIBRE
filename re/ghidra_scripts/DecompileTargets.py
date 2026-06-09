# Ghidra headless post-script : décompile une liste de fonctions cibles.
# Lit /home/maelle/fw/re/out/targets.tsv (addr<TAB>mangled[<TAB>demangled])
# Ecrit le pseudo-C dans /home/maelle/fw/re/out/decomp/<addr>_<nom>.c
# @category Nickel
from ghidra.app.decompiler import DecompInterface, DecompileOptions
from ghidra.util.task import ConsoleTaskMonitor
import re as _re

TARGETS = "/home/maelle/fw/re/out/targets.tsv"
OUTDIR  = "/home/maelle/fw/re/out/decomp"

def safe(name):
    return _re.sub(r'[^A-Za-z0-9_.-]', '_', name)[:120]

prog = currentProgram
fm = prog.getFunctionManager()
ifc = DecompInterface()
opts = DecompileOptions()
ifc.setOptions(opts)
ifc.openProgram(prog)
mon = ConsoleTaskMonitor()

lines = open(TARGETS).read().splitlines()
print("[*] %d cibles" % len(lines))
ok=0; fail=0
for ln in lines:
    if not ln.strip(): continue
    parts = ln.split("\t")
    addr_s = parts[0]
    mangled = parts[1] if len(parts)>1 else addr_s
    va = int(addr_s, 16) & ~1
    addr = toAddr(va)
    func = fm.getFunctionContaining(addr)
    if func is None:
        func = getFunctionAt(addr)
    if func is None:
        # tente de créer la fonction (au cas où l'analyse ne l'a pas faite)
        try:
            func = createFunction(addr, None)
        except:
            func = None
    if func is None:
        print("  [MISS] %s %s (pas de fonction)" % (addr_s, mangled))
        fail+=1
        continue
    res = ifc.decompileFunction(func, 120, mon)
    if res is None or not res.decompileCompleted():
        print("  [FAIL] %s %s : %s" % (addr_s, mangled, res.getErrorMessage() if res else "null"))
        fail+=1
        continue
    c = res.getDecompiledFunction().getC()
    fn = "%s/%s_%s.c" % (OUTDIR, addr_s.replace("0x",""), safe(func.getName()))
    f = open(fn, "w")
    f.write("// addr=%s  mangled=%s\n" % (addr_s, mangled))
    f.write("// ghidra-name=%s  sig=%s\n\n" % (func.getName(), func.getPrototypeString(False, False)))
    f.write(c)
    f.close()
    ok+=1
    print("  [OK]   %s -> %s" % (func.getName(), fn))
print("[*] décompilées: %d  échecs: %d" % (ok, fail))
