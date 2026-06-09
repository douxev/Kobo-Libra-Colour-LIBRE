// Module export de stats de lecture.
// Lit les stats depuis KoboReader.sqlite (read-only) et écrit un fichier au
// format texte/Prometheus (scrapeable). N'utilise pas le réseau (le `client`
// est ignoré).
use std::fs;
use std::io::Write;
use std::path::Path;

use kclient::Client;
use rusqlite::{Connection, OpenFlags};

use crate::config::Config;

/// Compteurs extraits de la table `content`.
struct Stats {
    total: i64,
    finished: i64,
    reading: i64,
    seconds: i64,
}

/// Vérifie si la colonne `col` existe dans la table `content` via PRAGMA.
fn has_column(conn: &Connection, col: &str) -> bool {
    let mut found = false;
    if let Ok(mut stmt) = conn.prepare("PRAGMA table_info(content)") {
        if let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(1)) {
            for name in rows.flatten() {
                if name.eq_ignore_ascii_case(col) {
                    found = true;
                    break;
                }
            }
        }
    }
    found
}

/// Lit un compteur entier depuis une requête sans paramètre ; 0 si échec.
fn query_count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |row| row.get::<_, i64>(0)).unwrap_or(0)
}

/// Récupère tous les compteurs depuis la base Kobo (ouverte en lecture seule).
fn gather(db: &str) -> Result<Stats, String> {
    let conn = Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| format!("ouverture {} : {}", db, e))?;

    // ContentType='6' = livres (les autres types sont chapitres/etc.).
    let total = query_count(&conn, "SELECT COUNT(*) FROM content WHERE ContentType='6'");
    let finished = query_count(
        &conn,
        "SELECT COUNT(*) FROM content WHERE ContentType='6' \
         AND (ReadStatus=2 OR ___PercentRead>=100)",
    );
    let reading = query_count(
        &conn,
        "SELECT COUNT(*) FROM content WHERE ContentType='6' AND ReadStatus=1",
    );

    // TimeSpentReading peut ne pas exister selon le firmware : on détecte la
    // colonne avant d'agréger pour éviter une erreur SQL.
    let seconds = if has_column(&conn, "TimeSpentReading") {
        query_count(
            &conn,
            "SELECT COALESCE(SUM(TimeSpentReading),0) FROM content WHERE ContentType='6'",
        )
    } else {
        0
    };

    Ok(Stats { total, finished, reading, seconds })
}

/// Construit le texte Prometheus avec en-têtes HELP/TYPE.
fn render(s: &Stats) -> String {
    let mut out = String::new();
    out.push_str("# HELP kobo_books_total Nombre total de livres dans la bibliothèque.\n");
    out.push_str("# TYPE kobo_books_total gauge\n");
    out.push_str(&format!("kobo_books_total {}\n", s.total));

    out.push_str("# HELP kobo_books_finished Nombre de livres terminés.\n");
    out.push_str("# TYPE kobo_books_finished gauge\n");
    out.push_str(&format!("kobo_books_finished {}\n", s.finished));

    out.push_str("# HELP kobo_books_reading Nombre de livres en cours de lecture.\n");
    out.push_str("# TYPE kobo_books_reading gauge\n");
    out.push_str(&format!("kobo_books_reading {}\n", s.reading));

    out.push_str("# HELP kobo_reading_seconds_total Temps de lecture cumulé en secondes.\n");
    out.push_str("# TYPE kobo_reading_seconds_total counter\n");
    out.push_str(&format!("kobo_reading_seconds_total {}\n", s.seconds));

    out
}

/// Écriture atomique : écrit dans un .tmp puis rename sur la cible.
fn write_atomic(out: &str, content: &str) -> Result<(), String> {
    let path = Path::new(out);
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            fs::create_dir_all(dir).map_err(|e| format!("création {} : {}", dir.display(), e))?;
        }
    }
    let tmp = path.with_extension("prom.tmp");
    {
        let mut f = fs::File::create(&tmp)
            .map_err(|e| format!("création {} : {}", tmp.display(), e))?;
        f.write_all(content.as_bytes())
            .map_err(|e| format!("écriture {} : {}", tmp.display(), e))?;
        f.flush().map_err(|e| format!("flush {} : {}", tmp.display(), e))?;
    }
    fs::rename(&tmp, path).map_err(|e| {
        // Nettoyage best-effort du tmp si le rename échoue.
        let _ = fs::remove_file(&tmp);
        format!("rename {} -> {} : {}", tmp.display(), path.display(), e)
    })
}

pub fn run(_client: &Client, c: &Config) -> Result<String, String> {
    let out = c.gets(
        "stats",
        "out_file",
        "/mnt/onboard/.adds/kobo-companion/stats.prom",
    );
    let stats = gather(&c.kobo_db())?;
    let text = render(&stats);
    write_atomic(&out, &text)?;
    Ok(format!("stats écrites: {}", out))
}
