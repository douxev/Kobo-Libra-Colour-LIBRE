// kobo-dbtool — manipulation de KoboReader.sqlite (Groupe B).
// Sous-commandes : collections | watch | sortfilter
// Backup de sécurité de la base AVANT toute écriture.
mod collections;
mod watched_folder;
mod sortfilter;

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use rusqlite::Connection;

pub fn parse_ini(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut section = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') { continue; }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_string();
            continue;
        }
        if let Some(eq) = line.find('=') {
            let key = line[..eq].trim().to_lowercase();
            let mut val = line[eq + 1..].trim().to_string();
            if val.len() >= 2 && ((val.starts_with('"') && val.ends_with('"')) || (val.starts_with('\'') && val.ends_with('\''))) {
                val = val[1..val.len() - 1].to_string();
            }
            map.insert(format!("{}.{}", section, key), val);
        }
    }
    map
}

fn backup_db(db: &str) -> Result<(), String> {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let bak = format!("{}.bak.{}", db, secs);
    fs::copy(db, &bak).map_err(|e| format!("backup {} : {}", bak, e))?;
    eprintln!("[dbtool] backup : {}", bak);
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: kobo-dbtool <collections|watch|sortfilter> [--config F] [--db F]");
        std::process::exit(2);
    }
    let cmd = args[1].clone();
    let mut config_path = String::new();
    let mut db_path = String::new();
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--config" => { i += 1; if i < args.len() { config_path = args[i].clone(); } }
            "--db" => { i += 1; if i < args.len() { db_path = args[i].clone(); } }
            _ => {}
        }
        i += 1;
    }
    let cfg = if config_path.is_empty() {
        HashMap::new()
    } else {
        match fs::read_to_string(&config_path) {
            Ok(t) => parse_ini(&t),
            Err(e) => { eprintln!("config {} : {}", config_path, e); HashMap::new() }
        }
    };
    if db_path.is_empty() {
        db_path = cfg.get("paths.kobo_db").cloned()
            .unwrap_or_else(|| "/mnt/onboard/.kobo/KoboReader.sqlite".to_string());
    }
    if !Path::new(&db_path).exists() {
        eprintln!("Base introuvable : {}", db_path);
        std::process::exit(1);
    }
    // backup avant les opérations qui écrivent
    if cmd == "collections" || cmd == "watch" || cmd == "sortfilter" {
        if let Err(e) = backup_db(&db_path) { eprintln!("{}", e); std::process::exit(1); }
    }
    let conn = match Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => { eprintln!("ouverture DB : {}", e); std::process::exit(1); }
    };
    let res = match cmd.as_str() {
        "collections" => collections::run(&conn, &cfg),
        "watch" => watched_folder::run(&conn, &cfg),
        "sortfilter" => sortfilter::run(&conn, &cfg),
        other => { eprintln!("sous-commande inconnue : {}", other); std::process::exit(2); }
    };
    match res {
        Ok(s) => println!("[dbtool] {} : {}", cmd, s),
        Err(e) => { eprintln!("[dbtool] {} ERREUR : {}", cmd, e); std::process::exit(1); }
    }
}
