// Module Wallabag (read-it-later) — récupère les articles non lus via l'API Wallabag,
// les convertit en EPUB (convert::html_to_epub) et les dépose dans le dossier cible pour Nickel.
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use kclient::Client;
use crate::config::Config;
use crate::log;

// --- État de déduplication (liste d'ids déjà importés), à la manière d'opds.rs ---
fn state_path(dest: &str) -> PathBuf {
    Path::new(dest).join(".wallabag-state")
}
fn load_state(dest: &str) -> HashSet<String> {
    fs::read_to_string(state_path(dest))
        .map(|t| t.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default()
}
fn append_state(dest: &str, id: &str) {
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(state_path(dest)) {
        let _ = writeln!(f, "{}", id);
    }
}

// Nettoie un nom de fichier (mêmes règles que opds::sanitize).
fn sanitize(name: &str) -> String {
    let mut s: String = name.chars().map(|c| if "/\\:*?\"<>|".contains(c) || (c as u32) < 0x20 { '_' } else { c }).collect();
    s = s.trim().trim_matches('.').to_string();
    if s.len() > 120 { s.truncate(120); }
    if s.is_empty() { "untitled".to_string() } else { s }
}

// Extrait le domaine d'une URL pour l'utiliser comme "auteur" si rien d'autre.
fn domain_of(article_url: &str) -> String {
    url::Url::parse(article_url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.trim_start_matches("www.").to_string()))
        .unwrap_or_default()
}

// Lit un champ texte d'un objet JSON (chaîne uniquement).
fn jstr(v: &serde_json::Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

// Récupère le token : token direct configuré, sinon OAuth2 password grant.
fn obtain_token(client: &Client, c: &Config, base: &str) -> Result<String, String> {
    // Token fourni directement.
    let direct = c.gets("wallabag", "token", "").trim().to_string();
    if !direct.is_empty() {
        return Ok(direct);
    }
    let client_id = c.gets("wallabag", "client_id", "");
    let client_secret = c.gets("wallabag", "client_secret", "");
    let username = c.gets("wallabag", "username", "");
    let password = c.gets("wallabag", "password", "");
    if client_id.is_empty() || client_secret.is_empty() || username.is_empty() || password.is_empty() {
        return Err("identifiants OAuth2 incomplets ([wallabag] client_id/client_secret/username/password ou token)".to_string());
    }
    // Corps form-urlencoded (pas de feature urlencoded sur reqwest : on encode via url).
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "password")
        .append_pair("client_id", &client_id)
        .append_pair("client_secret", &client_secret)
        .append_pair("username", &username)
        .append_pair("password", &password)
        .finish();
    let token_url = format!("{}/oauth/v2/token", base);
    let resp = client
        .post(&token_url)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("OAuth2 HTTP {}", resp.status()));
    }
    let text = resp.text().map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let token = v.get("access_token").and_then(|x| x.as_str()).unwrap_or("").to_string();
    if token.is_empty() {
        return Err("réponse OAuth2 sans access_token".to_string());
    }
    Ok(token)
}

// Récupère les articles non archivés (toutes les pages, jusqu'à une limite raisonnable).
fn fetch_entries(client: &Client, base: &str, token: &str) -> Result<Vec<serde_json::Value>, String> {
    let mut items = Vec::new();
    let mut page = 1u32;
    loop {
        let url = format!("{}/api/entries.json?archive=0&perPage=50&page={}", base, page);
        let resp = client
            .get(&url)
            .header("authorization", format!("Bearer {}", token))
            .send()
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            // Page 1 en échec = vraie erreur ; au-delà on s'arrête proprement.
            if page == 1 {
                return Err(format!("entries HTTP {}", resp.status()));
            }
            break;
        }
        let text = resp.text().map_err(|e| e.to_string())?;
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let batch = v
            .get("_embedded")
            .and_then(|e| e.get("items"))
            .and_then(|i| i.as_array())
            .cloned()
            .unwrap_or_default();
        let got = batch.len();
        items.extend(batch);
        // Nombre total de pages indiqué par l'API si présent.
        let pages = v.get("pages").and_then(|p| p.as_u64()).unwrap_or(1) as u32;
        if got == 0 || page >= pages || page >= 50 {
            break;
        }
        page += 1;
    }
    Ok(items)
}

// Marque un article comme archivé (best-effort).
fn archive_entry(client: &Client, base: &str, token: &str, id: i64) {
    let url = format!("{}/api/entries/{}.json", base, id);
    let _ = client
        .patch(&url)
        .header("authorization", format!("Bearer {}", token))
        .header("content-type", "application/json")
        .body("{\"archive\":1}".to_string())
        .send();
}

pub fn run(client: &Client, c: &Config) -> Result<String, String> {
    let base = c.gets("wallabag", "url", "").trim_end_matches('/').to_string();
    if base.is_empty() {
        return Err("[wallabag] url manquante".to_string());
    }

    let token = obtain_token(client, c, &base)?;
    let entries = fetch_entries(client, &base, &token)?;

    // Sous-dossier de dépôt (créé si besoin).
    let subdir = c.gets("wallabag", "subdir", "Wallabag");
    let out_dir = if subdir.trim().is_empty() {
        PathBuf::from(&c.dest)
    } else {
        PathBuf::from(&c.dest).join(sanitize(&subdir))
    };
    if let Err(e) = fs::create_dir_all(&out_dir) {
        return Err(format!("création {} : {}", out_dir.display(), e));
    }

    let archive_after = c.getb("wallabag", "archive_after", false);
    let mut state = load_state(&c.dest);
    let mut imported = 0u32;

    for entry in &entries {
        // id peut être un nombre.
        let id = match entry.get("id").and_then(|x| x.as_i64()) {
            Some(n) => n,
            None => continue,
        };
        let id_key = id.to_string();
        if state.contains(&id_key) {
            continue;
        }

        let content = jstr(entry, "content");
        if content.trim().is_empty() {
            // Rien à convertir : on note l'id pour ne pas réessayer indéfiniment.
            state.insert(id_key.clone());
            append_state(&c.dest, &id_key);
            log(&c.dest, &format!("WARN wallabag #{} sans contenu", id));
            continue;
        }

        let raw_title = jstr(entry, "title");
        let title = if raw_title.trim().is_empty() { format!("article-{}", id) } else { raw_title };
        let article_url = jstr(entry, "url");
        // Auteur : domaine de la source (à défaut, vide).
        let author = domain_of(&article_url);

        let epub = match convert::html_to_epub(&content, &title, &author) {
            Ok(bytes) => bytes,
            Err(e) => {
                log(&c.dest, &format!("ERR conversion wallabag #{} ({}) : {}", id, title, e));
                continue;
            }
        };

        let fname = format!("{}-{}.epub", sanitize(&title), id);
        let path = out_dir.join(&fname);
        if let Err(e) = fs::write(&path, &epub) {
            log(&c.dest, &format!("ERR écriture {} : {}", path.display(), e));
            continue;
        }

        imported += 1;
        state.insert(id_key.clone());
        append_state(&c.dest, &id_key);
        log(&c.dest, &format!("\u{2913} {}", fname));

        if archive_after {
            archive_entry(client, &base, &token, id);
        }
    }

    Ok(format!("{} article(s) importé(s)", imported))
}
