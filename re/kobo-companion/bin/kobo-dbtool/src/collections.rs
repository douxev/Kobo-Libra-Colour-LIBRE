// Séries / collections pour le contenu sideloadé.
// Crée des collections Nickel (tables Shelf / ShelfContent) à partir d'un mapping.
//
// Spec :
//  - config : cfg["collections.mode"] = "folder" (défaut) | "explicit"
//      mode "folder"  : une collection par sous-dossier de cfg["collections.root"]
//                       (défaut "/mnt/onboard") ; rattache chaque livre de `content` dont le
//                       chemin (ContentID/file://) est sous ce sous-dossier.
//      mode "explicit": cfg["collections.map"] = "NomCollection:/chemin1;Autre:/chemin2"
//  - Tables Kobo :
//      Shelf(Id TEXT, Name TEXT, CreationDate, LastModified, Type='UserTag', _IsDeleted=0,
//            _IsVisible=1, _IsSynced=0, InternalName, ...)  -> créer si absent.
//      ShelfContent(ShelfName TEXT, ContentId TEXT, DateModified, _IsDeleted=0, _IsSynced=0)
//  - Idempotent (INSERT OR IGNORE / vérifier l'existant). Renvoyer "N collections / M rattachements".
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn run(conn: &Connection, cfg: &HashMap<String, String>) -> Result<String, String> {
    // Accès config avec défaut.
    let get = |k: &str, def: &str| -> String {
        cfg.get(k).cloned().unwrap_or_else(|| def.to_string())
    };

    let mode = get("collections.mode", "folder");
    // root sans slash final pour des chemins propres.
    let mut root = get("collections.root", "/mnt/onboard");
    while root.len() > 1 && root.ends_with('/') {
        root.pop();
    }

    // Liste (nom_collection, préfixe_chemin) à traiter.
    let mut targets: Vec<(String, String)> = Vec::new();

    match mode.as_str() {
        "explicit" => {
            // "Nom:/chemin;Nom2:/chemin2"
            let map = get("collections.map", "");
            for entry in map.split(';') {
                let entry = entry.trim();
                if entry.is_empty() {
                    continue;
                }
                // Couper au premier ':' uniquement (le chemin peut en contenir).
                if let Some(pos) = entry.find(':') {
                    let name = entry[..pos].trim().to_string();
                    let mut path = entry[pos + 1..].trim().to_string();
                    while path.len() > 1 && path.ends_with('/') {
                        path.pop();
                    }
                    if !name.is_empty() && !path.is_empty() {
                        targets.push((name, path));
                    }
                }
            }
        }
        _ => {
            // mode "folder" (défaut) : un sous-dossier direct de root = une collection.
            let entries = std::fs::read_dir(&root)
                .map_err(|e| format!("lecture du dossier {} : {}", root, e))?;
            for ent in entries {
                let ent = match ent {
                    Ok(e) => e,
                    Err(_) => continue, // entrée illisible : on ignore proprement
                };
                let is_dir = ent.file_type().map(|t| t.is_dir()).unwrap_or(false);
                if !is_dir {
                    continue;
                }
                let name = ent.file_name().to_string_lossy().to_string();
                // Ignorer les dossiers cachés (.kobo, .adobe-digital-editions, etc.).
                if name.starts_with('.') {
                    continue;
                }
                let path = format!("{}/{}", root, name);
                targets.push((name, path));
            }
        }
    }

    // Colonnes réellement présentes dans chaque table (robustesse au schéma).
    let shelf_cols = table_columns(conn, "Shelf")
        .map_err(|e| format!("PRAGMA Shelf : {}", e))?;
    if shelf_cols.is_empty() {
        return Err("table Shelf introuvable dans la base".to_string());
    }
    let sc_cols = table_columns(conn, "ShelfContent")
        .map_err(|e| format!("PRAGMA ShelfContent : {}", e))?;
    if sc_cols.is_empty() {
        return Err("table ShelfContent introuvable dans la base".to_string());
    }

    let now = iso_now();

    let mut created: usize = 0;
    let mut attached: usize = 0;

    for (name, path_prefix) in &targets {
        // 1) Créer la Shelf si elle n'existe pas (clé logique = Name).
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM Shelf WHERE Name = ?1 LIMIT 1",
                params![name],
                |_| Ok(()),
            )
            .map(|_| true)
            .unwrap_or(false);

        if !exists {
            let id = pseudo_uuid(name);
            if insert_shelf(conn, &shelf_cols, &id, name, &now)? {
                created += 1;
            }
        }

        // 2) Rattacher les content sous ce préfixe.
        //    ContentID des sideloads = "file://<chemin absolu>".
        //    On accepte aussi un ContentID stocké en chemin nu (sans file://).
        let like_file = format!("file://{}/%", path_prefix);
        let like_raw = format!("{}/%", path_prefix);

        let content_ids = select_content_ids(conn, &like_file, &like_raw)?;
        for cid in &content_ids {
            if insert_shelf_content(conn, &sc_cols, name, cid, &now)? {
                attached += 1;
            }
        }
    }

    Ok(format!("{} collections / {} rattachements", created, attached))
}

/// Récupère le nom des colonnes d'une table via PRAGMA table_info.
fn table_columns(conn: &Connection, table: &str) -> Result<Vec<String>, String> {
    let sql = format!("PRAGMA table_info({})", table);
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1)) // col 1 = name
        .map_err(|e| e.to_string())?;
    let mut cols = Vec::new();
    for r in rows {
        match r {
            Ok(c) => cols.push(c),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(cols)
}

/// Insère une ligne Shelf en ne renseignant que les colonnes présentes.
/// Renvoie Ok(true) si une ligne a été insérée.
fn insert_shelf(
    conn: &Connection,
    cols: &[String],
    id: &str,
    name: &str,
    now: &str,
) -> Result<bool, String> {
    // Valeurs candidates par défaut (uniquement si la colonne existe).
    let mut names: Vec<&str> = Vec::new();
    let mut vals: Vec<String> = Vec::new();

    let mut push = |col: &'static str, val: &str| {
        if cols.iter().any(|c| c == col) {
            names.push(col);
            vals.push(val.to_string());
        }
    };

    push("Id", id);
    push("Name", name);
    push("InternalName", name);
    push("CreationDate", now);
    push("LastModified", now);
    push("Type", "UserTag");
    push("_IsDeleted", "false");
    push("_IsVisible", "true");
    push("_IsSynced", "false");

    if names.is_empty() {
        return Err("aucune colonne connue dans Shelf".to_string());
    }

    let placeholders: Vec<String> = (1..=names.len()).map(|i| format!("?{}", i)).collect();
    let sql = format!(
        "INSERT OR IGNORE INTO Shelf ({}) VALUES ({})",
        names.join(", "),
        placeholders.join(", ")
    );

    let params_dyn: Vec<&dyn rusqlite::ToSql> =
        vals.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
    let n = conn
        .execute(&sql, params_dyn.as_slice())
        .map_err(|e| format!("INSERT Shelf {} : {}", name, e))?;
    Ok(n > 0)
}

/// Sélectionne les ContentID des livres dont le chemin est sous un préfixe.
fn select_content_ids(
    conn: &Connection,
    like_file: &str,
    like_raw: &str,
) -> Result<Vec<String>, String> {
    // On vise les livres (ContentType 6) si la colonne existe, sinon tout.
    let has_ct = table_columns(conn, "content")
        .map(|c| c.iter().any(|x| x == "ContentType"))
        .unwrap_or(false);

    let sql = if has_ct {
        "SELECT ContentID FROM content \
         WHERE ContentType = '6' AND (ContentID LIKE ?1 OR ContentID LIKE ?2)"
    } else {
        "SELECT ContentID FROM content WHERE ContentID LIKE ?1 OR ContentID LIKE ?2"
    };

    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![like_file, like_raw], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    let mut ids = Vec::new();
    for r in rows {
        match r {
            Ok(id) => ids.push(id),
            Err(_) => continue, // ligne illisible : on saute
        }
    }
    Ok(ids)
}

/// Insère une ligne ShelfContent. Renvoie Ok(true) si insérée (sinon déjà présente).
fn insert_shelf_content(
    conn: &Connection,
    cols: &[String],
    shelf_name: &str,
    content_id: &str,
    now: &str,
) -> Result<bool, String> {
    let mut names: Vec<&str> = Vec::new();
    let mut vals: Vec<String> = Vec::new();

    let mut push = |col: &'static str, val: &str| {
        if cols.iter().any(|c| c == col) {
            names.push(col);
            vals.push(val.to_string());
        }
    };

    push("ShelfName", shelf_name);
    push("ContentId", content_id);
    push("DateModified", now);
    push("_IsDeleted", "false");
    push("_IsSynced", "false");

    if !names.iter().any(|n| *n == "ShelfName") || !names.iter().any(|n| *n == "ContentId") {
        return Err("colonnes clés manquantes dans ShelfContent".to_string());
    }

    let placeholders: Vec<String> = (1..=names.len()).map(|i| format!("?{}", i)).collect();
    let sql = format!(
        "INSERT OR IGNORE INTO ShelfContent ({}) VALUES ({})",
        names.join(", "),
        placeholders.join(", ")
    );

    let params_dyn: Vec<&dyn rusqlite::ToSql> =
        vals.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
    let n = conn
        .execute(&sql, params_dyn.as_slice())
        .map_err(|e| format!("INSERT ShelfContent {} : {}", shelf_name, e))?;
    Ok(n > 0)
}

/// Horodatage ISO 8601 approximatif (UTC), suffisant pour Nickel.
fn iso_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Conversion civile depuis l'epoch (UTC), algorithme de Howard Hinnant.
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y, m, d, hh, mm, ss
    )
}

/// Identifiant déterministe « uuid-like » (hex) dérivé d'une chaîne.
/// Format 8-4-4-4-12 pour ressembler à un UUID, stable et idempotent.
fn pseudo_uuid(seed: &str) -> String {
    // Quatre hash FNV-1a 64 bits sur des variantes du seed -> 256 bits d'entropie.
    let h0 = fnv1a(seed.as_bytes());
    let h1 = fnv1a(format!("{}|1", seed).as_bytes());
    let h2 = fnv1a(format!("{}|2", seed).as_bytes());
    let h3 = fnv1a(format!("{}|3", seed).as_bytes());

    let a = (h0 >> 32) as u32;
    let b = (h0 & 0xFFFF) as u16;
    let c = (h1 >> 48) as u16;
    let d = (h1 & 0xFFFF) as u16;
    let e = ((h2 ^ h3) & 0xFFFF_FFFF_FFFF) as u64;

    format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}", a, b, c, d, e)
}

/// FNV-1a 64 bits.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
