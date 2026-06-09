// Module annotations/surlignages.
// Lit les surlignages/notes de KoboReader.sqlite (table Bookmark) et les synchronise (push +
// pull optionnel) avec un endpoint self-hosted via le client mTLS partagé.
//
// PUSH : sérialise tous les surlignages en JSON et POST vers l'endpoint.
// PULL : GET l'endpoint puis insère (INSERT OR IGNORE) les surlignages absents localement.
use rusqlite::{params, Connection, OpenFlags};
use serde_json::{Map, Value};

use kclient::Client;
use crate::config::Config;
use crate::log;

/// Un surlignage tel que stocké dans la table Bookmark.
struct Bookmark {
    bookmark_id: String,
    volume_id: String,
    content_id: String,
    text: String,
    annotation: String,
    chapter_progress: f64,
    start_container_path: String,
    date_created: String,
    typ: String,
}

/// Ouvre la base Kobo en lecture seule.
fn open_ro(path: &str) -> Result<Connection, String> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("ouverture DB (ro) {} : {}", path, e))
}

/// Lit tous les surlignages depuis la table Bookmark (tolérant aux valeurs NULL).
fn read_bookmarks(conn: &Connection) -> Result<Vec<Bookmark>, String> {
    let sql = "SELECT BookmarkID, VolumeID, ContentID, Text, Annotation, \
               ChapterProgress, StartContainerPath, DateCreated, Type FROM Bookmark";
    let mut stmt = conn.prepare(sql).map_err(|e| format!("préparation requête Bookmark : {}", e))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Bookmark {
                bookmark_id: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                volume_id: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                content_id: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                text: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                annotation: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                chapter_progress: r.get::<_, Option<f64>>(5)?.unwrap_or(0.0),
                start_container_path: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                date_created: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
                typ: r.get::<_, Option<String>>(8)?.unwrap_or_default(),
            })
        })
        .map_err(|e| format!("lecture Bookmark : {}", e))?;
    let mut out = Vec::new();
    for row in rows {
        match row {
            Ok(b) => {
                if !b.bookmark_id.is_empty() {
                    out.push(b);
                }
            }
            Err(e) => return Err(format!("ligne Bookmark : {}", e)),
        }
    }
    Ok(out)
}

/// Convertit un surlignage en objet JSON.
fn bookmark_to_json(b: &Bookmark) -> Value {
    let mut m = Map::new();
    m.insert("bookmark_id".to_string(), Value::String(b.bookmark_id.clone()));
    m.insert("volume_id".to_string(), Value::String(b.volume_id.clone()));
    m.insert("content_id".to_string(), Value::String(b.content_id.clone()));
    m.insert("text".to_string(), Value::String(b.text.clone()));
    m.insert("annotation".to_string(), Value::String(b.annotation.clone()));
    m.insert(
        "chapter_progress".to_string(),
        serde_json::Number::from_f64(b.chapter_progress)
            .map(Value::Number)
            .unwrap_or(Value::Null),
    );
    m.insert("start_container_path".to_string(), Value::String(b.start_container_path.clone()));
    m.insert("date_created".to_string(), Value::String(b.date_created.clone()));
    m.insert("type".to_string(), Value::String(b.typ.clone()));
    Value::Object(m)
}

/// Extrait un String depuis une Value (objet -> clé), tolérant aux types/absences.
fn jstr(obj: &Map<String, Value>, key: &str) -> String {
    match obj.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

/// Lit un f64 depuis une Value (objet -> clé), 0.0 par défaut.
fn jf64(obj: &Map<String, Value>, key: &str) -> f64 {
    match obj.get(key) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// PULL : insère les surlignages distants absents localement (INSERT OR IGNORE, read-write).
fn pull_merge(db_path: &str, remote: &[Value], known: &std::collections::HashSet<String>) -> Result<u32, String> {
    // N'ouvre la base en écriture que s'il y a quelque chose à insérer.
    let to_insert: Vec<&Map<String, Value>> = remote
        .iter()
        .filter_map(|v| v.as_object())
        .filter(|o| {
            let id = jstr(o, "bookmark_id");
            !id.is_empty() && !known.contains(&id)
        })
        .collect();
    if to_insert.is_empty() {
        return Ok(0);
    }

    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE,
    )
    .map_err(|e| format!("ouverture DB (rw) {} : {}", db_path, e))?;

    let sql = "INSERT OR IGNORE INTO Bookmark \
               (BookmarkID, VolumeID, ContentID, Text, Annotation, ChapterProgress, \
                StartContainerPath, DateCreated, Type) \
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)";
    let mut stmt = conn.prepare(sql).map_err(|e| format!("préparation INSERT : {}", e))?;
    let mut inserted = 0u32;
    for o in to_insert {
        let res = stmt.execute(params![
            jstr(o, "bookmark_id"),
            jstr(o, "volume_id"),
            jstr(o, "content_id"),
            jstr(o, "text"),
            jstr(o, "annotation"),
            jf64(o, "chapter_progress"),
            jstr(o, "start_container_path"),
            jstr(o, "date_created"),
            jstr(o, "type"),
        ]);
        match res {
            Ok(n) if n > 0 => inserted += 1,
            Ok(_) => {} // déjà présent (IGNORE)
            Err(e) => return Err(format!("INSERT surlignage : {}", e)),
        }
    }
    Ok(inserted)
}

pub fn run(client: &Client, c: &Config) -> Result<String, String> {
    let endpoint = c.gets("annotations", "endpoint", "");
    if endpoint.is_empty() {
        return Ok("endpoint non configuré".to_string());
    }

    // 1) Lecture locale (read-only). DB absente -> dégrade proprement.
    let db_path = c.kobo_db();
    let conn = match open_ro(&db_path) {
        Ok(conn) => conn,
        Err(e) => {
            log(&c.dest, &format!("annotations: DB indisponible : {}", e));
            return Ok("0 poussé(s) / 0 reçu(s)".to_string());
        }
    };
    let locals = match read_bookmarks(&conn) {
        Ok(v) => v,
        Err(e) => {
            log(&c.dest, &format!("annotations: lecture impossible : {}", e));
            return Ok("0 poussé(s) / 0 reçu(s)".to_string());
        }
    };
    let known: std::collections::HashSet<String> =
        locals.iter().map(|b| b.bookmark_id.clone()).collect();
    drop(conn); // libère le handle read-only avant un éventuel pull read-write

    // 2) PUSH : sérialise tous les surlignages et POST.
    let arr: Vec<Value> = locals.iter().map(bookmark_to_json).collect();
    let mut root = Map::new();
    root.insert("device_id".to_string(), Value::String(c.gets("annotations", "device_id", "kobo")));
    root.insert("annotations".to_string(), Value::Array(arr));
    let body = Value::Object(root).to_string();

    let mut pushed = 0u32;
    match client
        .post(&endpoint)
        .header("content-type", "application/json")
        .body(body)
        .send()
    {
        Ok(resp) => {
            if resp.status().is_success() {
                pushed = locals.len() as u32;
            } else {
                log(&c.dest, &format!("annotations: push HTTP {}", resp.status()));
            }
        }
        Err(e) => log(&c.dest, &format!("annotations: push réseau : {}", e)),
    }

    // 3) PULL optionnel.
    let mut received = 0u32;
    if c.getb("annotations", "pull", false) {
        match client.get(&endpoint).send() {
            Ok(resp) => {
                if resp.status().is_success() {
                    match resp.text() {
                        Ok(text) => match serde_json::from_str::<Value>(&text) {
                            Ok(v) => {
                                // Accepte soit un tableau racine, soit {"annotations": [...]}.
                                let remote: Vec<Value> = match v {
                                    Value::Array(a) => a,
                                    Value::Object(ref o) => o
                                        .get("annotations")
                                        .and_then(|x| x.as_array())
                                        .cloned()
                                        .unwrap_or_default(),
                                    _ => Vec::new(),
                                };
                                match pull_merge(&db_path, &remote, &known) {
                                    Ok(n) => received = n,
                                    Err(e) => log(&c.dest, &format!("annotations: pull/merge : {}", e)),
                                }
                            }
                            Err(e) => log(&c.dest, &format!("annotations: JSON pull invalide : {}", e)),
                        },
                        Err(e) => log(&c.dest, &format!("annotations: lecture corps pull : {}", e)),
                    }
                } else {
                    log(&c.dest, &format!("annotations: pull HTTP {}", resp.status()));
                }
            }
            Err(e) => log(&c.dest, &format!("annotations: pull réseau : {}", e)),
        }
    }

    Ok(format!("{} poussé(s) / {} reçu(s)", pushed, received))
}
