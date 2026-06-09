// kobo-syncd — daemon de sync Kobo. Client mTLS partagé (kclient), modules activés via
// features.conf : opds, panelize, wallabag, annotations, positions, stats, wifi.
// Sert aussi le PANNEAU DE CONFIG WEB (localhost uniquement) — voir web.rs.
mod config;
mod opds;
mod wallabag;
mod annotations;
mod positions;
mod stats;
mod wifi;
mod panelize;
mod web;

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use config::{load_features, Config};
use web::Status;

pub fn log(dest: &str, msg: &str) {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let line = format!("[{}] {}", secs, msg);
    println!("{}", line);
    let p = Path::new(dest).join("kobo-syncd.log");
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(p) {
        use std::io::Write;
        let _ = writeln!(f, "{}", line);
    }
}

fn reachable(client: &kclient::Client, base: &str) -> bool {
    base.is_empty() || client.get(base).send().is_ok()
}

fn run_cycle(
    client: &kclient::Client,
    c: &Config,
    features: &std::collections::HashSet<String>,
    status: &Arc<Mutex<Status>>,
    list_only: bool,
) {
    // 1) wifi conditionnel
    let online = if features.contains("wifi") {
        let allowed = wifi::allowed_now(c);
        wifi::ensure(c, allowed);
        if !allowed { log(&c.dest, "wifi: fenêtre hors-ligne (airplane), modules réseau ignorés"); }
        allowed
    } else { true };

    // 2) modules réseau (si en ligne)
    if online {
        if features.contains("opds") && !c.base_url.is_empty() && !c.api_key.is_empty()
            && reachable(client, &c.base_url) {
            let (n, seen) = opds::sync(client, c, list_only);
            if !list_only { log(&c.dest, &format!("opds: {} nouveau(x) / {} vus", n, seen)); }
        }
        if !list_only {
            for (feat, name, f) in [
                ("wallabag", "wallabag", wallabag::run as fn(&kclient::Client, &Config) -> Result<String, String>),
                ("annotations", "annotations", annotations::run),
                ("positions", "positions", positions::run),
            ] {
                if features.contains(feat) {
                    match f(client, c) {
                        Ok(s) => log(&c.dest, &format!("{}: {}", name, s)),
                        Err(e) => log(&c.dest, &format!("{}: ERREUR {}", name, e)),
                    }
                }
            }
        }
    }

    // 3) stats (local)
    if features.contains("stats") && !list_only {
        match stats::run(client, c) {
            Ok(s) => log(&c.dest, &format!("stats: {}", s)),
            Err(e) => log(&c.dest, &format!("stats: ERREUR {}", e)),
        }
    }

    // 4) panelization auto (après le téléchargement des BD), gardée contre le chevauchement
    //    avec un déclenchement depuis le panneau web.
    if features.contains("panelize") && !list_only {
        let go = match status.lock() {
            Ok(mut s) if !s.panelizing => { s.panelizing = true; true }
            _ => false,
        };
        if go {
            let res = panelize::run(c).unwrap_or_else(|e| format!("erreur: {}", e));
            log(&c.dest, &format!("panelize: {}", res));
            if let Ok(mut s) = status.lock() { s.panelizing = false; s.last_panelize = res; }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut config_path = String::new();
    let mut features_path = String::new();
    let (mut daemon, mut list) = (false, false);
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--once" => {}
            "--daemon" => daemon = true,
            "--list" => list = true,
            "--config" => { i += 1; if i < args.len() { config_path = args[i].clone(); } }
            "--features" => { i += 1; if i < args.len() { features_path = args[i].clone(); } }
            "-h" | "--help" => { eprintln!("usage: kobo-syncd [--config F] [--features F] [--once|--daemon|--list]"); return; }
            _ => {}
        }
        i += 1;
    }
    if config_path.is_empty() {
        let exe = std::env::current_exe().ok();
        config_path = exe.and_then(|p| p.parent().map(|d| d.join("kobo-syncd.conf")))
            .map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| "kobo-syncd.conf".to_string());
    }
    if features_path.is_empty() {
        features_path = Path::new(&config_path).parent()
            .map(|d| d.join("features.conf")).map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "features.conf".to_string());
    }
    let c0 = match Config::load(&config_path) {
        Ok(c) => c,
        Err(e) => { eprintln!("Erreur config : {}", e); std::process::exit(1); }
    };
    let _ = fs::create_dir_all(&c0.dest);
    log(&c0.dest, &format!("kobo-syncd démarré (mTLS={})", kclient::has_identity(&c0.mtls)));

    let status = Arc::new(Mutex::new(Status::default()));

    if daemon {
        // Panneau de config web (localhost uniquement), si activé.
        if c0.getb("web", "enabled", true) {
            let (cp, fp, st) = (config_path.clone(), features_path.clone(), status.clone());
            std::thread::spawn(move || web::serve(cp, fp, st));
        }
        log(&c0.dest, &format!("daemon (interval={}s)", c0.interval));
        loop {
            // Recharge config + features à chaque cycle (les réglages du panneau prennent effet).
            let c = match Config::load(&config_path) {
                Ok(c) => c,
                Err(e) => { log(&c0.dest, &format!("config reload: {}", e)); std::thread::sleep(Duration::from_secs(60)); continue; }
            };
            let mut features = load_features(&features_path);
            if features.is_empty() { features.insert("opds".to_string()); }
            match kclient::build(&c.mtls) {
                Ok(client) => run_cycle(&client, &c, &features, &status, false),
                Err(e) => log(&c.dest, &format!("client HTTP/mTLS: {}", e)),
            }
            std::thread::sleep(Duration::from_secs(c.interval.max(10)));
        }
    } else {
        let mut features = load_features(&features_path);
        if features.is_empty() { features.insert("opds".to_string()); }
        match kclient::build(&c0.mtls) {
            Ok(client) => run_cycle(&client, &c0, &features, &status, list),
            Err(e) => { eprintln!("Erreur client HTTP/mTLS : {}", e); std::process::exit(1); }
        }
    }
}
