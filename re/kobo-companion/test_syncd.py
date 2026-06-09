#!/usr/bin/env python3
"""Test de kobo-syncd : (1) OPDS plain HTTP, (2) OPDS sur mTLS (serveur exige cert client)."""
import os, sys, ssl, tempfile, subprocess, threading, socketserver, http.server, textwrap, shutil
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "opds-sync"))
import test_local as M   # mock OPDS (H, ROUTES, KEY)

BIN = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "target", "debug", "kobo-syncd")
WORK = tempfile.mkdtemp(prefix="syncd_")

class MH(http.server.BaseHTTPRequestHandler):
    """Mock OPDS avec Content-Length (cadrage correct du corps, y compris en TLS)."""
    protocol_version = "HTTP/1.1"
    def log_message(self, *a): pass
    def _send(self, body, ctype):
        self.send_response(200); self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body))); self.end_headers()
        self.wfile.write(body)
    def do_HEAD(self):
        self.send_response(200); self.send_header("Content-Length", "0"); self.end_headers()
    def do_GET(self):
        if self.path in M.ROUTES:
            self._send(M.ROUTES[self.path], "application/atom+xml"); return
        if self.path.startswith("/download/"):
            n = self.path.rsplit("/", 1)[-1]
            self._send(("FAKEBOOK-%s" % n).encode() * 10, "application/octet-stream"); return
        self.send_response(404); self.send_header("Content-Length","0"); self.end_headers()

def write_conf(path, base, dest, mtls=None):
    s = textwrap.dedent(f"""\
        [server]
        base_url = {base}
        api_key = {M.KEY}
        opds_path = /api/opds/{{api_key}}
        [sync]
        dest = {dest}
        formats =
        max_books = 0
        [net]
        timeout = 10
    """)
    if mtls:
        s += f"[mtls]\nclient_cert = {mtls['cert']}\nclient_key = {mtls['key']}\nca_cert = {mtls['ca']}\n"
    open(path, "w").write(s)

def run(conf):
    return subprocess.run([BIN, "--config", conf, "--once"], capture_output=True, text=True)

def serve_plain():
    srv = socketserver.ThreadingTCPServer(("127.0.0.1", 0), MH)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv, srv.server_address[1]

def gen_certs(d):
    def sh(cmd): subprocess.run(cmd, shell=True, check=True, capture_output=True)
    sh(f'openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -keyout {d}/ca.key -out {d}/ca.crt -subj "/CN=Test CA" -days 2')
    san = f'{d}/san.cnf'; open(san,"w").write("subjectAltName=DNS:localhost,IP:127.0.0.1")
    for who, cn in (("srv","localhost"),("cli","kobo-device")):
        sh(f'openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -keyout {d}/{who}.key -out {d}/{who}.csr -subj "/CN={cn}"')
        ext = f'-extfile {san}' if who=="srv" else ''
        sh(f'openssl x509 -req -in {d}/{who}.csr -CA {d}/ca.crt -CAkey {d}/ca.key -CAcreateserial -out {d}/{who}.crt -days 2 {ext}')
    return {"ca": f"{d}/ca.crt", "cert": f"{d}/cli.crt", "key": f"{d}/cli.key",
            "srvcrt": f"{d}/srv.crt", "srvkey": f"{d}/srv.key"}

def serve_mtls(certs):
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain(certs["srvcrt"], certs["srvkey"])
    ctx.verify_mode = ssl.CERT_REQUIRED            # <-- exige le cert client (mTLS)
    ctx.load_verify_locations(certs["ca"])
    srv = socketserver.ThreadingTCPServer(("127.0.0.1", 0), MH)
    srv.socket = ctx.wrap_socket(srv.socket, server_side=True)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv, srv.server_address[1]

ok_all = True
def check(label, books_dir, stdout):
    global ok_all
    books = sorted(f for f in os.listdir(books_dir) if not f.startswith(".") and not f.endswith(".log")) if os.path.isdir(books_dir) else []
    ok = len(books) == 3 and any(f.endswith(".pdf") for f in books)
    print(f"  [{label}] livres={books}  -> {'✅' if ok else '❌'}")
    if not ok: print("    stdout:", stdout.strip()); ok_all = False

# 1) PLAIN HTTP
srv, port = serve_plain()
dest1 = f"{WORK}/plain"; conf1 = f"{WORK}/plain.conf"
write_conf(conf1, f"http://127.0.0.1:{port}", dest1)
r = run(conf1); check("plain-http", dest1, r.stdout); srv.shutdown()

# 2) mTLS
certs = gen_certs(WORK)
srv2, port2 = serve_mtls(certs)
dest2 = f"{WORK}/mtls"; conf2 = f"{WORK}/mtls.conf"
write_conf(conf2, f"https://localhost:{port2}", dest2, mtls=certs)
r2 = run(conf2); check("mtls", dest2, r2.stdout)

# 2b) contrôle négatif : sans cert client, le serveur doit REFUSER
dest3 = f"{WORK}/nomtls"; conf3 = f"{WORK}/nomtls.conf"
write_conf(conf3, f"https://localhost:{port2}", dest3)  # pas de [mtls]
r3 = run(conf3)
books3 = os.listdir(dest3) if os.path.isdir(dest3) else []
neg_ok = not any(f.endswith((".epub",".pdf")) for f in books3)
print(f"  [mtls-negatif] sans cert client -> {'refusé ✅' if neg_ok else 'ACCEPTÉ ❌'}")
ok_all = ok_all and neg_ok
srv2.shutdown()

shutil.rmtree(WORK, ignore_errors=True)
print("\nRESULTAT:", "✅ OK" if ok_all else "❌ ECHEC")
sys.exit(0 if ok_all else 1)
