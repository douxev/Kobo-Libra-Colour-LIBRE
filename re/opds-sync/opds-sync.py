#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
opds-sync — synchronise une bibliothèque OPDS (Kavita, calibre-web, COPS…) vers
la mémoire interne d'une liseuse Kobo, pour lecture dans Nickel.

- Pur stdlib Python 3.10 (présent sur le firmware Kobo) : aucune dépendance.
- Marche en HTTP et HTTPS (y compris certificat auto-signé via verify_tls=false).
- Parcourt récursivement les flux OPDS de navigation jusqu'aux flux d'acquisition,
  télécharge chaque livre absent dans le dossier cible (Nickel l'importe ensuite).
- Dé-duplication via un manifeste JSON (ne re-télécharge pas ce qui existe déjà).
- Modes : --once (une passe) | --daemon (boucle, sync quand le serveur est joignable)
          --list (n'affiche que, ne télécharge rien).

Config : voir opds-sync.conf (INI). Tout est paramétrable (IP, clé API, dossier…).
"""
import argparse, configparser, json, os, re, ssl, sys, time, hashlib
import urllib.request, urllib.error, urllib.parse
import xml.etree.ElementTree as ET

ATOM = "{http://www.w3.org/2005/Atom}"
OPDS_ACQ_PREFIX = "http://opds-spec.org/acquisition"
NAV_TYPE_HINT = "kind=navigation"
ACQ_TYPE_HINT = "kind=acquisition"

# extensions de fichiers livres reconnues par Nickel
TYPE_EXT = {
    "application/epub+zip": "epub",
    "application/x-kobo-epub+zip": "kepub.epub",
    "application/pdf": "pdf",
    "application/x-cbz": "cbz",
    "application/vnd.comicbook+zip": "cbz",
    "application/x-cbr": "cbr",
    "application/vnd.comicbook-rar": "cbr",
    "application/x-mobipocket-ebook": "mobi",
    "text/plain": "txt",
}

def log(msg, logfile=None):
    line = time.strftime("%Y-%m-%d %H:%M:%S ") + msg
    print(line, flush=True)
    if logfile:
        try:
            with open(logfile, "a") as f:
                f.write(line + "\n")
        except OSError:
            pass

def sanitize(name, maxlen=120):
    name = re.sub(r'[\x00-\x1f/\\:*?"<>|]+', "_", name).strip().strip(".")
    name = re.sub(r"\s+", " ", name)
    return (name or "untitled")[:maxlen]

class Config:
    def __init__(self, path):
        cp = configparser.ConfigParser()
        if not cp.read(path):
            raise SystemExit("Config introuvable : %s" % path)
        s, sy, n = cp["server"], cp["sync"], cp["net"]
        self.base_url = s.get("base_url", "").rstrip("/")
        self.api_key = s.get("api_key", "").strip()
        self.opds_path = s.get("opds_path", "/api/opds/{api_key}")
        self.username = s.get("username", "").strip()
        self.password = s.get("password", "").strip()
        self.dest = sy.get("dest", "/mnt/onboard/OPDS")
        self.formats = [x.strip().lower() for x in sy.get("formats", "").split(",") if x.strip()]
        self.overwrite = sy.getboolean("overwrite", False)
        self.layout = sy.get("layout", "series")  # series | author | flat
        self.max_books = sy.getint("max_books", 0)
        self.verify_tls = n.getboolean("verify_tls", True)
        self.timeout = n.getint("timeout", 30)
        self.interval = n.getint("interval", 900)

    def root_url(self):
        path = self.opds_path.format(api_key=urllib.parse.quote(self.api_key))
        return self.base_url + path

class OpdsClient:
    def __init__(self, cfg, logfile=None):
        self.cfg = cfg
        self.logfile = logfile
        ctx = ssl.create_default_context()
        if not cfg.verify_tls:
            ctx.check_hostname = False
            ctx.verify_mode = ssl.CERT_NONE
        handlers = [urllib.request.HTTPSHandler(context=ctx)]
        if cfg.username:
            mgr = urllib.request.HTTPPasswordMgrWithDefaultRealm()
            mgr.add_password(None, cfg.base_url, cfg.username, cfg.password)
            handlers.append(urllib.request.HTTPBasicAuthHandler(mgr))
        self.opener = urllib.request.build_opener(*handlers)
        self.opener.addheaders = [("User-Agent", "opds-sync/1.0 (Kobo)")]

    def open(self, url):
        return self.opener.open(url, timeout=self.cfg.timeout)

    def get_bytes(self, url):
        with self.open(url) as r:
            return r.read()

def parse_feed(xml_bytes, feed_url):
    """Renvoie (nav_urls, acquisitions, next_url).
    acquisitions = list de dict {id,title,author,href,type}."""
    root = ET.fromstring(xml_bytes)
    nav_urls, acqs = [], []
    next_url = None
    # liens de pagination au niveau du flux
    for link in root.findall(ATOM + "link"):
        rel = link.get("rel", ""); href = link.get("href", "")
        if rel == "next" and href:
            next_url = urllib.parse.urljoin(feed_url, href)
    for entry in root.findall(ATOM + "entry"):
        eid = (entry.findtext(ATOM + "id") or "").strip()
        title = (entry.findtext(ATOM + "title") or "").strip()
        author = ""
        a = entry.find(ATOM + "author")
        if a is not None:
            author = (a.findtext(ATOM + "name") or "").strip()
        for link in entry.findall(ATOM + "link"):
            rel = link.get("rel", ""); typ = link.get("type", ""); href = link.get("href", "")
            if not href:
                continue
            absurl = urllib.parse.urljoin(feed_url, href)
            if rel.startswith(OPDS_ACQ_PREFIX):
                # lien d'acquisition direct => fichier à télécharger
                acqs.append({"id": eid or absurl, "title": title, "author": author,
                             "href": absurl, "type": typ})
            elif rel == "subsection" or "kind=navigation" in typ or "kind=acquisition" in typ:
                # sous-flux (navigation OU acquisition) => à parcourir récursivement
                nav_urls.append(absurl)
    return nav_urls, acqs, next_url

def ext_for(acq, resp_headers=None):
    # priorité : Content-Disposition, puis type MIME, puis extension de l'URL
    if resp_headers:
        cd = resp_headers.get("Content-Disposition", "")
        m = re.search(r'filename\*?=(?:UTF-8\'\')?"?([^";]+)', cd)
        if m:
            e = os.path.splitext(urllib.parse.unquote(m.group(1)))[1].lstrip(".").lower()
            if e:
                return e
    t = (acq.get("type") or "").split(";")[0].strip().lower()
    if t in TYPE_EXT:
        return TYPE_EXT[t]
    path = urllib.parse.urlparse(acq["href"]).path
    e = os.path.splitext(path)[1].lstrip(".").lower()
    return e or "epub"

class Manifest:
    def __init__(self, path):
        self.path = path
        self.data = {}
        if os.path.exists(path):
            try:
                self.data = json.load(open(path))
            except (OSError, ValueError):
                self.data = {}
    def has(self, key):
        return key in self.data
    def add(self, key, fname):
        self.data[key] = {"file": fname, "ts": int(time.time())}
    def save(self):
        tmp = self.path + ".tmp"
        try:
            json.dump(self.data, open(tmp, "w"), ensure_ascii=False, indent=0)
            os.replace(tmp, self.path)
        except OSError as e:
            log("WARN manifeste non sauvé: %s" % e, None)

def target_path(cfg, acq, ext):
    base = sanitize("%s - %s" % (acq["author"], acq["title"]) if acq["author"] else acq["title"])
    sub = ""
    if cfg.layout == "author" and acq["author"]:
        sub = sanitize(acq["author"])
    elif cfg.layout == "series":
        sub = ""  # Kavita expose déjà la série dans le titre la plupart du temps
    d = os.path.join(cfg.dest, sub) if sub else cfg.dest
    return os.path.join(d, base + "." + ext)

def crawl(client, cfg, only_list=False):
    """Parcourt l'arbre OPDS, télécharge les acquisitions absentes."""
    logfile = os.path.join(cfg.dest, "opds-sync.log")
    os.makedirs(cfg.dest, exist_ok=True)
    manifest = Manifest(os.path.join(cfg.dest, ".opds-sync-state.json"))
    root = cfg.root_url()
    to_visit = [root]
    visited = set()
    downloaded = 0
    seen_acq = 0
    while to_visit:
        url = to_visit.pop(0)
        if url in visited:
            continue
        visited.add(url)
        try:
            data = client.get_bytes(url)
        except (urllib.error.URLError, urllib.error.HTTPError, ssl.SSLError, OSError) as e:
            log("WARN flux injoignable %s : %s" % (url, e), logfile)
            continue
        try:
            nav_urls, acqs, next_url = parse_feed(data, url)
        except ET.ParseError as e:
            log("WARN XML invalide %s : %s" % (url, e), logfile)
            continue
        for n in nav_urls:
            if n not in visited:
                to_visit.append(n)
        if next_url and next_url not in visited:
            to_visit.append(next_url)
        for acq in acqs:
            seen_acq += 1
            if cfg.formats:
                guessed = ext_for(acq)
                if guessed.split(".")[-1] not in cfg.formats:
                    continue
            key = acq["id"] or acq["href"]
            if manifest.has(key) and not cfg.overwrite:
                continue
            if only_list:
                log("• %s — %s" % (acq["author"] or "?", acq["title"]), logfile)
                continue
            try:
                ok = download(client, cfg, acq, manifest, logfile)
                if ok:
                    downloaded += 1
                    manifest.save()
            except (urllib.error.URLError, urllib.error.HTTPError, ssl.SSLError, OSError) as e:
                log("ERR  téléchargement %s : %s" % (acq["title"], e), logfile)
            if cfg.max_books and downloaded >= cfg.max_books:
                log("Limite max_books=%d atteinte." % cfg.max_books, logfile)
                manifest.save()
                return downloaded, seen_acq
    manifest.save()
    return downloaded, seen_acq

def download(client, cfg, acq, manifest, logfile):
    with client.open(acq["href"]) as r:
        ext = ext_for(acq, r.headers)
        path = target_path(cfg, acq, ext)
        if os.path.exists(path) and not cfg.overwrite:
            manifest.add(acq["id"] or acq["href"], os.path.basename(path))
            return False
        os.makedirs(os.path.dirname(path), exist_ok=True)
        tmp = path + ".part"
        with open(tmp, "wb") as f:
            while True:
                chunk = r.read(65536)
                if not chunk:
                    break
                f.write(chunk)
        os.replace(tmp, path)
    manifest.add(acq["id"] or acq["href"], os.path.basename(path))
    log("⤓ %s" % os.path.basename(path), logfile)
    return True

def reachable(cfg):
    try:
        ctx = ssl.create_default_context()
        if not cfg.verify_tls:
            ctx.check_hostname = False; ctx.verify_mode = ssl.CERT_NONE
        req = urllib.request.Request(cfg.base_url, method="HEAD")
        urllib.request.urlopen(req, timeout=5, context=ctx)
        return True
    except Exception:
        # HEAD peut être refusé ; un échec de connexion lèvera URLError(connexion)
        try:
            urllib.request.urlopen(cfg.base_url, timeout=5, context=ctx)
            return True
        except urllib.error.HTTPError:
            return True   # serveur répond (auth/4xx) => joignable
        except Exception:
            return False

def main():
    ap = argparse.ArgumentParser(description="Sync OPDS -> Kobo (Nickel)")
    here = os.path.dirname(os.path.abspath(__file__))
    ap.add_argument("--config", default=os.path.join(here, "opds-sync.conf"))
    ap.add_argument("--once", action="store_true", help="une seule passe de sync")
    ap.add_argument("--daemon", action="store_true", help="boucle : sync quand le serveur est joignable")
    ap.add_argument("--list", action="store_true", help="liste seulement, ne télécharge rien")
    args = ap.parse_args()
    cfg = Config(args.config)
    if not cfg.base_url or not cfg.api_key:
        raise SystemExit("Configure base_url et api_key dans %s" % args.config)
    client = OpdsClient(cfg)
    logfile = os.path.join(cfg.dest, "opds-sync.log")
    os.makedirs(cfg.dest, exist_ok=True)

    if args.daemon:
        log("daemon démarré (interval=%ds, dest=%s)" % (cfg.interval, cfg.dest), logfile)
        while True:
            if reachable(cfg):
                try:
                    n, seen = crawl(client, cfg)
                    log("sync OK : %d nouveau(x) / %d vus" % (n, seen), logfile)
                except Exception as e:
                    log("ERR sync : %r" % e, logfile)
            else:
                log("serveur injoignable, on réessaie plus tard", logfile)
            time.sleep(cfg.interval)
    else:
        n, seen = crawl(client, cfg, only_list=args.list)
        if not args.list:
            log("Terminé : %d téléchargé(s) / %d vus." % (n, seen), logfile)

if __name__ == "__main__":
    main()
