#!/usr/bin/env python3
"""Test local de opds-sync contre un mock OPDS façon Kavita.
Vérifie : navigation récursive, pagination, acquisition, dédup, noms de fichiers."""
import http.server, socketserver, threading, tempfile, os, subprocess, sys, textwrap

HERE = os.path.dirname(os.path.abspath(__file__))
KEY = "TESTKEY"

def feed(entries_xml, extra_links=""):
    return ('<?xml version="1.0" encoding="UTF-8"?>'
            '<feed xmlns="http://www.w3.org/2005/Atom">'
            '<id>urn:test</id><title>Test</title>' + extra_links + entries_xml + '</feed>').encode()

NAV = 'application/atom+xml;profile=opds-catalog;kind=navigation'
ACQ_T = 'application/atom+xml;profile=opds-catalog;kind=acquisition'

ROOT = feed(
    f'<entry><id>lib-nav</id><title>Toutes les bibliothèques</title>'
    f'<link rel="subsection" type="{NAV}" href="/api/opds/{KEY}/libraries"/></entry>')

LIBRARIES = feed(
    f'<entry><id>lib-1</id><title>BD</title>'
    f'<link rel="subsection" type="{ACQ_T}" href="/api/opds/{KEY}/libraries/1"/></entry>')

# page 1 : 2 livres + lien next vers page 2
LIB1_P1 = feed(
    f'<entry><id>book-1</id><title>Tome 1</title><author><name>Hergé</name></author>'
    f'<link rel="http://opds-spec.org/acquisition" type="application/epub+zip" href="/download/1"/></entry>'
    f'<entry><id>book-2</id><title>Tome 2 / illégal:?*</title><author><name>Hergé</name></author>'
    f'<link rel="http://opds-spec.org/acquisition/open-access" type="application/epub+zip" href="/download/2"/></entry>',
    extra_links=f'<link rel="next" type="{ACQ_T}" href="/api/opds/{KEY}/libraries/1?page=2"/>')

# page 2 : 1 livre, pas de next
LIB1_P2 = feed(
    f'<entry><id>book-3</id><title>Tome 3</title><author><name>Hergé</name></author>'
    f'<link rel="http://opds-spec.org/acquisition" type="application/pdf" href="/download/3"/></entry>')

ROUTES = {
    f"/api/opds/{KEY}": ROOT,
    f"/api/opds/{KEY}/libraries": LIBRARIES,
    f"/api/opds/{KEY}/libraries/1": LIB1_P1,
    f"/api/opds/{KEY}/libraries/1?page=2": LIB1_P2,
}

class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_HEAD(self):
        self.send_response(200); self.end_headers()
    def do_GET(self):
        if self.path in ROUTES:
            body = ROUTES[self.path]
            self.send_response(200)
            self.send_header("Content-Type", "application/atom+xml")
            self.end_headers(); self.wfile.write(body); return
        if self.path.startswith("/download/"):
            n = self.path.rsplit("/", 1)[-1]
            data = ("FAKEBOOK-%s" % n).encode() * 10
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.end_headers(); self.wfile.write(data); return
        self.send_response(404); self.end_headers()

def run():
    srv = socketserver.ThreadingTCPServer(("127.0.0.1", 0), H)
    port = srv.server_address[1]
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    dest = tempfile.mkdtemp(prefix="opds_test_")
    conf = os.path.join(dest, "test.conf")
    open(conf, "w").write(textwrap.dedent(f"""\
        [server]
        base_url = http://127.0.0.1:{port}
        api_key = {KEY}
        opds_path = /api/opds/{{api_key}}
        username =
        password =
        [sync]
        dest = {dest}/books
        formats =
        overwrite = false
        layout = flat
        max_books = 0
        [net]
        verify_tls = false
        timeout = 10
        interval = 900
        """))
    py = sys.executable
    def run_once():
        return subprocess.run([py, os.path.join(HERE, "opds-sync.py"), "--config", conf, "--once"],
                              capture_output=True, text=True)
    r1 = run_once()
    print("=== PASSE 1 ==="); print(r1.stdout.strip());
    if r1.returncode != 0: print("STDERR:", r1.stderr)
    files = sorted(os.listdir(os.path.join(dest, "books")))
    books = [f for f in files if not f.startswith(".") and not f.endswith(".log")]
    print("Fichiers téléchargés :", books)
    r2 = run_once()
    print("=== PASSE 2 (dédup attendue : 0 nouveau) ==="); print(r2.stdout.strip().splitlines()[-1])

    # Assertions
    ok = True
    if len(books) != 3: print("ECHEC: attendu 3 livres, eu", len(books)); ok = False
    if not any(f.endswith(".pdf") for f in books): print("ECHEC: le .pdf (book-3) manque"); ok = False
    if any(c in "".join(books) for c in '/:?*'): print("ECHEC: caractères illégaux dans un nom"); ok = False
    if "0 téléchargé" not in r2.stdout and "0 nouveau" not in r2.stdout and "0 téléchargé(s)" not in r2.stdout:
        # vérifier via le compteur final
        if "Terminé : 0" not in r2.stdout: print("ECHEC: dédup passe 2 n'a pas donné 0"); ok = False
    print("\nRESULTAT:", "✅ OK" if ok else "❌ ECHEC")
    srv.shutdown()
    sys.exit(0 if ok else 1)

if __name__ == "__main__":
    run()
