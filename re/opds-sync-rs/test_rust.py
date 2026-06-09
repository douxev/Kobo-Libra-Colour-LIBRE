#!/usr/bin/env python3
"""Teste le binaire Rust (build hôte) contre le mock OPDS de test_local.py."""
import os, sys, tempfile, subprocess, threading, socketserver, textwrap
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "opds-sync"))
import test_local as M   # réutilise H, ROUTES, KEY

BIN = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "target", "debug", "opds-sync")

def run():
    srv = socketserver.ThreadingTCPServer(("127.0.0.1", 0), M.H)
    port = srv.server_address[1]
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    d = tempfile.mkdtemp(prefix="opds_rs_")
    conf = os.path.join(d, "c.conf")
    open(conf, "w").write(textwrap.dedent(f"""\
        [server]
        base_url = http://127.0.0.1:{port}
        api_key = {M.KEY}
        opds_path = /api/opds/{{api_key}}
        [sync]
        dest = {d}/books
        formats =
        overwrite = false
        max_books = 0
        [net]
        verify_tls = true
        timeout = 10
        """))
    def once():
        return subprocess.run([BIN, "--config", conf, "--once"], capture_output=True, text=True)
    r1 = once()
    print("=== PASSE 1 ==="); print(r1.stdout.strip() or r1.stderr.strip())
    books = sorted(f for f in os.listdir(os.path.join(d, "books")) if not f.startswith(".") and not f.endswith(".log"))
    print("Fichiers :", books)
    r2 = once()
    print("=== PASSE 2 (dédup) ==="); print((r2.stdout.strip() or "").splitlines()[-1] if r2.stdout.strip() else r2.stderr)
    ok = True
    if len(books) != 3: print("ECHEC: attendu 3, eu", len(books)); ok = False
    if not any(f.endswith(".pdf") for f in books): print("ECHEC: pdf manquant"); ok = False
    if any(ch in "".join(books) for ch in '/:?*'): print("ECHEC: caractère illégal"); ok = False
    if "0 téléchargé" not in r2.stdout: print("ECHEC: dédup != 0"); ok = False
    print("\nRESULTAT:", "✅ OK" if ok else "❌ ECHEC")
    srv.shutdown(); sys.exit(0 if ok else 1)

if __name__ == "__main__":
    run()
