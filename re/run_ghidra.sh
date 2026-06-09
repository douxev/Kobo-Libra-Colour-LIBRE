#!/bin/bash
# Importe libnickel.so dans un projet Ghidra, analyse, puis décompile les cibles.
set -e
cd /home/maelle/fw
GH=ghidra_12.1.2_PUBLIC
JDK=/home/maelle/fw/jdk-21.0.11+10
PROJDIR=/home/maelle/fw/re/ghidra_proj
PROJ=nickel

# Forcer Ghidra à utiliser le JDK 21
if ! grep -q "JAVA_HOME_OVERRIDE=$JDK" "$GH/support/launch.properties"; then
  sed -i "s|^JAVA_HOME_OVERRIDE=.*|JAVA_HOME_OVERRIDE=$JDK|" "$GH/support/launch.properties"
fi
export JAVA_HOME=$JDK
mkdir -p "$PROJDIR" re/out/decomp

# Import + analyse (réutilise le projet si déjà importé)
if [ ! -f "$PROJDIR/$PROJ.gpr" ]; then
  echo "=== IMPORT + ANALYSE (long) ==="
  "$GH/support/analyzeHeadless" "$PROJDIR" "$PROJ" \
    -import /home/maelle/fw/libnickel.so \
    -processor ARM:LE:32:v8 \
    -scriptPath /home/maelle/fw/re/ghidra_scripts \
    -postScript DecompileTargets.py
else
  echo "=== Projet existant : décompilation seule ==="
  "$GH/support/analyzeHeadless" "$PROJDIR" "$PROJ" \
    -process libnickel.so -noanalysis \
    -scriptPath /home/maelle/fw/re/ghidra_scripts \
    -postScript DecompileTargets.py
fi
echo "=== Fichiers décompilés ==="
ls -la re/out/decomp/
