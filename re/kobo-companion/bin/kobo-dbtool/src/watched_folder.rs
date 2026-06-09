// Watched folder -> métadonnées/couvertures (sans Calibre).
//
// Approche ROBUSTE retenue (documentée) :
//   Insérer une ligne `content` complète et valide pour Nickel est fragile : le schéma exact
//   varie selon les versions du firmware et de nombreuses colonnes sont attendues. On privilégie
//   donc la mise à jour de l'existant :
//     - si le fichier est DÉJÀ présent dans `content` (Nickel l'a importé), on fait un simple
//       UPDATE des métadonnées (Title/Attribution) issues de convert::epub_meta ;
//     - sinon, on tente un INSERT OR IGNORE MINIMAL en ne renseignant que des colonnes dont on
//       a vérifié l'existence via PRAGMA table_info (jamais de colonne inconnue).
//   La couverture extraite est déposée à côté du livre (<chemin>.cover.<ext>) faute de connaître
//   l'emplacement exact du cache Nickel. Tout est idempotent et défensif (aucun panic).
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

pub fn run(conn: &Connection, cfg: &HashMap<String, String>) -> Result<String, String> {
    let folder = cfg
        .get("watched.folder")
        .cloned()
        .unwrap_or_else(|| "/mnt/onboard".to_string());

    // Colonnes réellement présentes dans `content` (défensif vis-à-vis du schéma).
    let cols = content_columns(conn)?;
    let has = |name: &str| cols.contains(name);

    // Parcours récursif : on collecte les ebooks.
    let mut books = Vec::new();
    collect_ebooks(Path::new(&folder), &mut books, 0);

    let mut traites = 0usize;
    let mut maj = 0usize;
    let mut inseres = 0usize;

    for path in &books {
        traites += 1;

        // ContentID candidat : "file://" + chemin absolu.
        let abs = match fs::canonicalize(path) {
            Ok(p) => p,
            Err(_) => path.clone(),
        };
        let content_id = format!("file://{}", abs.to_string_lossy());

        // Métadonnées de l'ebook (dégradation propre si lecture impossible).
        let meta = match convert::epub_meta(path) {
            Ok(m) => m,
            Err(_) => continue, // fichier illisible : on ignore proprement
        };
        let title = if meta.title.trim().is_empty() {
            file_stem(path)
        } else {
            meta.title.clone()
        };
        let author = meta.author.clone();

        // Couverture : déposée à côté du livre (best effort, jamais bloquant).
        if let Some(ref data) = meta.cover {
            write_cover(path, data, &meta.cover_mime);
        }

        // L'entrée existe-t-elle déjà dans `content` ?
        let exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM content WHERE ContentID = ?1 LIMIT 1",
                params![content_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("SELECT content : {}", e))?;

        if exists.is_some() {
            // UPDATE des métadonnées sur l'entrée importée par Nickel.
            // Placeholders positionnels construits dynamiquement : on ne lie QUE les
            // colonnes réellement présentes (sinon rusqlite refuserait un param inutilisé).
            let mut sets: Vec<String> = Vec::new();
            let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            if has("Title") {
                binds.push(Box::new(title.clone()));
                sets.push(format!("Title = ?{}", binds.len()));
            }
            if has("Attribution") {
                binds.push(Box::new(author.clone()));
                sets.push(format!("Attribution = ?{}", binds.len()));
            }
            if sets.is_empty() {
                continue; // aucune colonne de métadonnée connue : rien à faire
            }
            // Dernier placeholder pour le WHERE ContentID.
            binds.push(Box::new(content_id.clone()));
            let sql = format!(
                "UPDATE content SET {} WHERE ContentID = ?{}",
                sets.join(", "),
                binds.len()
            );
            let refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
            match conn.execute(&sql, refs.as_slice()) {
                Ok(n) if n > 0 => maj += 1,
                _ => {}
            }
        } else {
            // INSERT OR IGNORE minimal : uniquement des colonnes existantes.
            let mut names: Vec<&str> = Vec::new();
            let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

            // ContentID est obligatoire (clé).
            names.push("ContentID");
            binds.push(Box::new(content_id.clone()));

            let mut push_str = |col: &'static str, val: String| {
                if has(col) {
                    names.push(col);
                    binds.push(Box::new(val));
                }
            };
            push_str("ContentType", "6".to_string());
            push_str("MimeType", mime_for(path).to_string());
            push_str("Title", title.clone());
            push_str("Attribution", author.clone());

            // Champs numériques courants pour un sideload propre.
            if has("___PercentRead") {
                names.push("___PercentRead");
                binds.push(Box::new(0i64));
            }
            if has("ReadStatus") {
                names.push("ReadStatus");
                binds.push(Box::new(0i64));
            }

            let placeholders: Vec<String> =
                (1..=names.len()).map(|i| format!("?{}", i)).collect();
            let sql = format!(
                "INSERT OR IGNORE INTO content ({}) VALUES ({})",
                names.join(", "),
                placeholders.join(", ")
            );
            let refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
            match conn.execute(&sql, refs.as_slice()) {
                Ok(n) if n > 0 => inseres += 1,
                _ => {} // déjà présent / refusé : on ne plante pas
            }
        }
    }

    Ok(format!(
        "{} livres traités ({} mis à jour / {} insérés)",
        traites, maj, inseres
    ))
}

/// Récupère l'ensemble des noms de colonnes de la table `content`.
fn content_columns(conn: &Connection) -> Result<HashSet<String>, String> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(content)")
        .map_err(|e| format!("PRAGMA content : {}", e))?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(1)) // colonne 1 = name
        .map_err(|e| format!("table_info : {}", e))?;
    let mut set = HashSet::new();
    for r in rows {
        if let Ok(name) = r {
            set.insert(name);
        }
    }
    Ok(set)
}

/// Parcours récursif borné en profondeur, collecte les ebooks .epub / .kepub.epub.
fn collect_ebooks(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 32 {
        return; // garde-fou anti-récursion pathologique
    }
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return, // dossier illisible : on ignore
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let ftype = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if ftype.is_dir() {
            collect_ebooks(&path, out, depth + 1);
        } else if ftype.is_file() && is_ebook(&path) {
            out.push(path);
        }
    }
}

/// Vrai pour les fichiers .epub et .kepub.epub (insensible à la casse).
fn is_ebook(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let lower = name.to_lowercase();
    lower.ends_with(".epub") // couvre aussi ".kepub.epub"
}

/// MimeType associé au fichier.
fn mime_for(path: &Path) -> &'static str {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();
    if name.ends_with(".kepub.epub") {
        "application/x-kobo-epub+zip"
    } else {
        "application/epub+zip"
    }
}

/// Nom de fichier sans extension, fallback pour un titre vide.
fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Sans titre")
        .to_string()
}

/// Écrit la couverture à côté du livre : <chemin>.cover.<ext>. Best effort.
fn write_cover(path: &Path, data: &[u8], mime: &str) {
    let ext = match mime {
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "jpg", // jpeg par défaut
    };
    let mut target = path.as_os_str().to_os_string();
    target.push(format!(".cover.{}", ext));
    let _ = fs::write(PathBuf::from(target), data); // erreurs ignorées volontairement
}
