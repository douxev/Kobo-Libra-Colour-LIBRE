// Ghidra headless post-script (Java) : décompilation ciblée en 3 phases.
//  A) corriger les frontières des fonctions cibles (clear+Thumb+disasm+createFunction)
//  B) re-analyse incrémentale (analyzeChanges) -> rétablit références/paramètres/chaînes
//  C) décompiler chaque cible et écrire le pseudo-C
// targets.tsv = addr<TAB>size<TAB>mangled<TAB>demangled
//@category Nickel
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.address.*;
import ghidra.program.model.lang.Register;
import ghidra.app.cmd.disassemble.ArmDisassembleCommand;
import ghidra.util.task.ConsoleTaskMonitor;
import java.io.*;
import java.math.BigInteger;
import java.util.*;

public class DecompileTargets extends GhidraScript {
    static class T { String addrS, mangled, dem; long va, size; boolean thumb; }

    public void run() throws Exception {
        String targetsF = "/home/maelle/fw/re/out/targets.tsv";
        String outdir   = "/home/maelle/fw/re/out/decomp";
        FunctionManager fm = currentProgram.getFunctionManager();
        Register tmode = currentProgram.getProgramContext().getRegister("TMode");
        ConsoleTaskMonitor mon = new ConsoleTaskMonitor();

        // charger cibles
        List<T> ts = new ArrayList<>();
        BufferedReader br = new BufferedReader(new FileReader(targetsF));
        String line;
        while ((line = br.readLine()) != null) {
            if (line.trim().isEmpty()) continue;
            String[] p = line.split("\t");
            if (p.length < 2) continue;
            T t = new T();
            t.addrS = p[0]; t.size = Long.parseLong(p[1]);
            t.mangled = p.length>2?p[2]:p[0]; t.dem = p.length>3?p[3]:"";
            long raw = Long.parseLong(p[0].replace("0x",""),16);
            t.thumb = (raw&1L)!=0; t.va = raw & ~1L;
            ts.add(t);
        }
        br.close();
        println("== Phase A : correction des frontières (" + ts.size() + " cibles) ==");
        AddressSet changed = new AddressSet();
        for (T t : ts) {
            Address start = toAddr(t.va), lastA = toAddr(t.va + Math.max(t.size,2) - 1);
            try {
                AddressSet range = new AddressSet(start, lastA);
                // suppression EN BOUCLE de toute fonction chevauchant la plage
                for (int pass = 0; pass < 12; pass++) {
                    List<Function> rm = new ArrayList<>();
                    Iterator<Function> it = fm.getFunctionsOverlapping(range);
                    while (it.hasNext()) rm.add(it.next());
                    Function cont = getFunctionContaining(start);
                    if (cont != null && !rm.contains(cont)) rm.add(cont);
                    if (rm.isEmpty()) break;
                    for (Function f : rm) { try { removeFunction(f); } catch (Exception e) {} }
                }
                clearListing(start, lastA);
                if (t.thumb && tmode != null)
                    currentProgram.getProgramContext().setValue(tmode, start, lastA, BigInteger.ONE);
                new ArmDisassembleCommand(start, range, t.thumb).applyTo(currentProgram, mon);
                Function func = getFunctionAt(start);
                if (func == null) func = createFunction(start, null);
                if (func == null) println("  A-MISS création " + t.addrS);
                changed.add(range);
            } catch (Exception e) { println("  A-EXC " + t.addrS + " : " + e); }
        }

        // Phase B (analyzeChanges) RETIRÉE : la re-analyse globale re-gonfle les
        // frontières ARM/Thumb (bug d'auto-analyse). On garde les frontières corrigées.
        println("== Phase C : décompilation (frontières corrigées, sans re-analyse) ==");
        DecompInterface ifc = new DecompInterface();
        DecompileOptions opt = new DecompileOptions();
        ifc.setOptions(opt);
        ifc.toggleCCode(true);
        ifc.toggleSyntaxTree(true);
        ifc.setSimplificationStyle("decompile");
        ifc.openProgram(currentProgram);
        int ok=0, fail=0;
        for (T t : ts) {
            Address start = toAddr(t.va);
            Function func = getFunctionAt(start);   // entrée EXACTE uniquement (pas de fallback contaminant)
            if (func == null) { println("MISS " + t.addrS + " (pas de fonction à l'entrée exacte)"); fail++; continue; }
            long bodySz = func.getBody().getNumAddresses();
            if (bodySz > t.size * 3 + 64) {          // garde-fou anti-gonflement (serré)
                println("BLOAT " + t.addrS + " corps=" + bodySz + " attendu~" + t.size + " -> ignoré (voir capstone)");
                fail++; continue;
            }
            DecompileResults res = ifc.decompileFunction(func, 180, mon);
            if (res == null || !res.decompileCompleted()) { println("FAIL " + t.addrS); fail++; continue; }
            String c = res.getDecompiledFunction().getC();
            String base = t.mangled.replaceAll("[^A-Za-z0-9_.-]", "_");
            if (base.length() > 90) base = base.substring(0,90);
            PrintWriter w = new PrintWriter(new FileWriter(outdir+"/"+t.addrS.replace("0x","")+"_"+base+".c"));
            w.println("// addr="+t.addrS+"  size="+t.size+"  entry=0x"+Long.toHexString(func.getEntryPoint().getOffset())+"  thumb="+t.thumb);
            w.println("// "+t.dem);
            w.println("// mangled="+t.mangled+"   ghidra-label="+func.getName());
            w.println();
            w.print(c);
            w.close();
            ok++;
            println("OK "+t.addrS+" ("+c.length()+"o) "+t.dem);
        }
        println("décompilées: "+ok+"  échecs: "+fail);
    }
}
