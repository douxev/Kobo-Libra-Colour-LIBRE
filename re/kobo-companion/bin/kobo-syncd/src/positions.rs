// Module positions de lecture (type KOSync) — push/pull de la progression par livre
// vers un endpoint self-hosted (mTLS partagé via kclient).
//
//  - DB : c.kobo_db(), table `content` (ContentID, Title, ___PercentRead, ChapterIDBookmarked,
//    ReadStatus, DateLastRead). Filtre ContentType='6' (livres) et ___PercentRead>0.
//  - PUSH : POST {document, percent, progress(0..1), device, timestamp}.
//  - PULL : GET endpoint ; si position distante plus récente, UPDATE ___PercentRead (prudemment).
use std::collections::HashMap;

use kclient::Client;
use crate::config::Config;
use crate::log;

/// Hash FNV-1a stable du ContentID -> identifiant de document (hex), comme convert/opds.
fn doc_id(content_id: &str) -> String {
    let mut h = 1469598103934665603u64;
    for b in content_id.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    format!("{:016x}", h)
}

/// Livre lu, tel que lu en base.
struct Local {
    content_id: String,
    title: String,
    percent: i64,
    timestamp: i64, // Unix secs dérivé de DateLastRead (0 si inconnu)
}

/// Convertit un DateLastRead Kobo (ISO-8601, ex "2024-01-02T03:04:05Z" ou
/// "2024-01-02T03:04:05.000") en timestamp Unix. Best-effort : 0 si non parsable.
fn iso_to_unix(s: &str) -> i64 {
    let s = s.trim();
    if s.is_empty() {
        return 0;
    }
    // Découpe AAAA-MM-JJ?HH:MM:SS, en ignorant fraction/zone.
    let bytes = s.as_bytes();
    let digits = |a: usize, b: usize| -> Option<i64> {
        if b > bytes.len() {
            return None;
        }
        s.get(a..b).and_then(|x| x.parse::<i64>().ok())
    };
    let year = match digits(0, 4) { Some(v) => v, None => return 0 };
    let month = match digits(5, 7) { Some(v) => v, None => return 0 };
    let day = match digits(8, 10) { Some(v) => v, None => return 0 };
    // Heure facultative.
    let (hour, min, sec) = if s.len() >= 19 {
        (digits(11, 13).unwrap_or(0), digits(14, 16).unwrap_or(0), digits(17, 19).unwrap_or(0))
    } else {
        (0, 0, 0)
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return 0;
    }
    days_from_civil(year, month, day) * 86400 + hour * 3600 + min * 60 + sec
}

/// Jours depuis l'époque Unix (1970-01-01) — algorithme de Howard Hinnant.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

/// Lit en base les livres lus (ContentType='6', ___PercentRead>0).
fn read_local(db_path: &str) -> Result<Vec<Local>, String> {
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| format!("ouverture DB {} : {}", db_path, e))?;

    let mut stmt = conn
        .prepare(
            "SELECT ContentID, Title, ___PercentRead, DateLastRead \
             FROM content \
             WHERE ContentType='6' AND ___PercentRead IS NOT NULL AND ___PercentRead > 0",
        )
        .map_err(|e| format!("préparation requête : {}", e))?;

    let rows = stmt
        .query_map([], |row| {
            let content_id: String = row.get(0)?;
            let title: String = row.get::<_, Option<String>>(1)?.unwrap_or_default();
            let percent: i64 = row.get::<_, Option<i64>>(2)?.unwrap_or(0);
            let date: String = row.get::<_, Option<String>>(3)?.unwrap_or_default();
            Ok(Local {
                content_id,
                title,
                percent,
                timestamp: iso_to_unix(&date),
            })
        })
        .map_err(|e| format!("lecture lignes : {}", e))?;

    let mut out = Vec::new();
    for r in rows {
        match r {
            Ok(l) => out.push(l),
            Err(e) => return Err(format!("ligne content : {}", e)),
        }
    }
    Ok(out)
}

/// Position distante normalisée.
struct Remote {
    percent: i64,
    timestamp: i64,
}

/// Récupère et indexe par `document` les positions distantes.
/// Accepte soit un tableau d'objets, soit un objet {document: {...}}.
fn fetch_remote(client: &Client, endpoint: &str) -> Result<HashMap<String, Remote>, String> {
    let resp = client.get(endpoint).send().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let body = resp.text().map_err(|e| e.to_string())?;
    if body.trim().is_empty() {
        return Ok(HashMap::new());
    }
    let val: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;

    let mut map = HashMap::new();
    let mut ingest = |doc: Option<&str>, obj: &serde_json::Value| {
        // document : depuis la clé ou un champ "document".
        let document = doc
            .map(|s| s.to_string())
            .or_else(|| obj.get("document").and_then(|v| v.as_str()).map(|s| s.to_string()));
        let document = match document {
            Some(d) if !d.is_empty() => d,
            _ => return,
        };
        // percent : champ "percent" (0..100) ou "progress" (0..1).
        let percent = obj
            .get("percent")
            .and_then(value_as_f64)
            .map(|p| p.round() as i64)
            .or_else(|| {
                obj.get("progress")
                    .and_then(value_as_f64)
                    .map(|p| (p * 100.0).round() as i64)
            })
            .unwrap_or(0);
        let percent = percent.clamp(0, 100);
        let timestamp = obj.get("timestamp").and_then(value_as_f64).map(|t| t as i64).unwrap_or(0);
        map.insert(document, Remote { percent, timestamp });
    };

    match &val {
        serde_json::Value::Array(arr) => {
            for obj in arr {
                ingest(None, obj);
            }
        }
        serde_json::Value::Object(o) => {
            // Cas 1 : objet unique décrivant une position.
            if o.contains_key("document") || o.contains_key("percent") || o.contains_key("progress") {
                ingest(None, &val);
            } else {
                // Cas 2 : dictionnaire {document: {...}}.
                for (k, obj) in o {
                    ingest(Some(k), obj);
                }
            }
        }
        _ => {}
    }
    Ok(map)
}

/// Lit un nombre JSON tolérant (number ou string numérique).
fn value_as_f64(v: &serde_json::Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
}

/// POST d'une position vers l'endpoint.
fn push_one(client: &Client, endpoint: &str, payload: &serde_json::Value) -> Result<(), String> {
    let body = serde_json::to_string(payload).map_err(|e| e.to_string())?;
    let resp = client
        .post(endpoint)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

pub fn run(client: &Client, c: &Config) -> Result<String, String> {
    let endpoint = c.gets("positions", "endpoint", "");
    if endpoint.trim().is_empty() {
        return Err("endpoint non configuré".to_string());
    }
    let device = c.gets("positions", "device", "kobo");
    let do_pull = c.getb("positions", "pull", true);
    let db_path = c.kobo_db();

    let locals = read_local(&db_path)?;

    // Indexe les locaux par doc_id (pour le pull) en gardant ContentID + timestamp.
    let mut by_doc: HashMap<String, (String, i64)> = HashMap::new();
    for l in &locals {
        by_doc.insert(doc_id(&l.content_id), (l.content_id.clone(), l.timestamp));
    }

    // --- PUSH ---
    let mut pushed = 0u32;
    for l in &locals {
        let document = doc_id(&l.content_id);
        let payload = serde_json::json!({
            "document": document,
            "percent": l.percent,
            "progress": l.percent as f64 / 100.0,
            "device": device,
            "timestamp": l.timestamp,
        });
        match push_one(client, &endpoint, &payload) {
            Ok(()) => pushed += 1,
            Err(e) => log(&c.dest, &format!("ERR push position {} : {}", l.title, e)),
        }
    }

    // --- PULL ---
    let mut applied = 0u32;
    if do_pull {
        match fetch_remote(client, &endpoint) {
            Ok(remote) => {
                // Ouvre la DB en écriture une seule fois (si au moins une MAJ candidate).
                let mut conn: Option<rusqlite::Connection> = None;
                for (document, r) in &remote {
                    let (content_id, local_ts) = match by_doc.get(document) {
                        Some(v) => v,
                        None => continue, // livre inconnu localement : on ne crée pas d'entrée.
                    };
                    // Prudence : n'appliquer que si la position distante est STRICTEMENT plus
                    // récente (timestamp). On ne régresse pas une progression locale plus avancée
                    // tant que le distant n'est pas plus récent.
                    if r.timestamp <= *local_ts {
                        continue;
                    }
                    let percent = r.percent.clamp(0, 100);
                    // Connexion paresseuse en lecture-écriture.
                    if conn.is_none() {
                        match rusqlite::Connection::open(&db_path) {
                            Ok(cn) => conn = Some(cn),
                            Err(e) => {
                                log(&c.dest, &format!("ERR ouverture DB (rw) : {}", e));
                                break;
                            }
                        }
                    }
                    if let Some(cn) = conn.as_ref() {
                        let res = cn.execute(
                            "UPDATE content SET ___PercentRead=?1 WHERE ContentID=?2",
                            rusqlite::params![percent, content_id],
                        );
                        match res {
                            Ok(n) if n > 0 => applied += 1,
                            Ok(_) => {}
                            Err(e) => log(
                                &c.dest,
                                &format!("ERR maj position {} : {}", content_id, e),
                            ),
                        }
                    }
                }
            }
            Err(e) => log(&c.dest, &format!("WARN pull positions : {}", e)),
        }
    }

    Ok(format!("{} poussée(s) / {} appliquée(s)", pushed, applied))
}
