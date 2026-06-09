#!/usr/bin/env python3
"""Teste les modules base de données (stats via kobo-syncd, collections/sortfilter via kobo-dbtool)
contre une fausse KoboReader.sqlite."""
import os, sqlite3, subprocess, tempfile, textwrap, sys

W = tempfile.mkdtemp(prefix="kdb_")
DB = os.path.join(W, "KoboReader.sqlite")
MOCK = os.path.join(W, "mock")
for d in ("BD", "Romans"):
    os.makedirs(os.path.join(MOCK, d), exist_ok=True)

con = sqlite3.connect(DB)
con.executescript("""
CREATE TABLE content (ContentID TEXT PRIMARY KEY, ContentType TEXT, MimeType TEXT, Title TEXT,
  Attribution TEXT, Series TEXT, SeriesNumber TEXT, ___PercentRead INTEGER, ReadStatus INTEGER,
  DateLastRead TEXT, TimeSpentReading INTEGER);
CREATE TABLE Shelf (Id TEXT, Name TEXT, CreationDate TEXT, LastModified TEXT, Type TEXT,
  _IsDeleted TEXT, _IsVisible TEXT, _IsSynced TEXT, InternalName TEXT);
CREATE TABLE ShelfContent (ShelfName TEXT, ContentId TEXT, DateModified TEXT, _IsDeleted TEXT, _IsSynced TEXT);
CREATE TABLE Bookmark (BookmarkID TEXT PRIMARY KEY, VolumeID TEXT, ContentID TEXT, Text TEXT,
  Annotation TEXT, ChapterProgress REAL, StartContainerPath TEXT, DateCreated TEXT, Type TEXT);
""")
rows = [
 (f"file://{MOCK}/BD/naruto.epub","6","application/epub+zip","Naruto T03","Kishimoto",None,None,40,1,"2026-01-01",3600),
 (f"file://{MOCK}/Romans/book1.epub","6","application/epub+zip","Un Roman","Auteur A",None,None,100,2,"2026-01-02",7200),
 (f"file://{MOCK}/Romans/book2.epub","6","application/epub+zip","Autre Roman","Auteur B",None,None,0,0,None,0),
]
con.executemany("INSERT INTO content (ContentID,ContentType,MimeType,Title,Attribution,Series,SeriesNumber,___PercentRead,ReadStatus,DateLastRead,TimeSpentReading) VALUES (?,?,?,?,?,?,?,?,?,?,?)", rows)
con.commit(); con.close()

CONF = os.path.join(W, "k.conf")
open(CONF, "w").write(textwrap.dedent(f"""\
    [paths]
    kobo_db = {DB}
    [stats]
    out_file = {W}/stats.prom
    [collections]
    mode = folder
    root = {MOCK}
    [sortfilter]
    series_from_title = yes
"""))
FEAT = os.path.join(W, "features.conf"); open(FEAT,"w").write("FEAT_stats=yes\n")

SYNCD = os.path.join(os.path.dirname(__file__), "target", "debug", "kobo-syncd")
DBTOOL = os.path.join(os.path.dirname(__file__), "target", "debug", "kobo-dbtool")
def run(*a): return subprocess.run(a, capture_output=True, text=True)

ok = True
def check(label, cond, extra=""):
    global ok
    print(f"  [{label}] {'✅' if cond else '❌ '+extra}")
    if not cond: ok = False

# 1) stats via kobo-syncd
r = run(SYNCD, "--config", CONF, "--features", FEAT, "--once")
prom = open(f"{W}/stats.prom").read() if os.path.exists(f"{W}/stats.prom") else ""
check("stats total=3", "kobo_books_total 3" in prom, prom)
check("stats finished=1", "kobo_books_finished 1" in prom, prom)
check("stats reading=1", "kobo_books_reading 1" in prom, prom)
check("stats seconds=10800", "kobo_reading_seconds_total 10800" in prom, prom)

# 2) sortfilter via kobo-dbtool
r = run(DBTOOL, "sortfilter", "--config", CONF)
con = sqlite3.connect(DB)
ser = con.execute("SELECT Series, SeriesNumber FROM content WHERE Title='Naruto T03'").fetchone()
check("sortfilter série déduite", ser == ("Naruto", "3"), f"{ser} | {r.stdout}{r.stderr}")

# 3) collections via kobo-dbtool
r = run(DBTOOL, "collections", "--config", CONF)
shelves = {x[0] for x in con.execute("SELECT Name FROM Shelf").fetchall()}
attach = con.execute("SELECT COUNT(*) FROM ShelfContent").fetchone()[0]
con.close()
check("collections BD+Romans", {"BD","Romans"} <= shelves, f"{shelves} | {r.stdout}{r.stderr}")
check("collections 3 rattachements", attach == 3, f"{attach} | {r.stdout}")

import shutil; shutil.rmtree(W, ignore_errors=True)
print("\nRESULTAT:", "✅ OK" if ok else "❌ ECHEC")
sys.exit(0 if ok else 1)
