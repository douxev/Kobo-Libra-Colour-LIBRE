// opds-sync — synchronise une bibliothèque OPDS (Kavita, calibre-web, COPS…) vers
// la mémoire interne d'une liseuse Kobo, pour lecture dans Nickel.
// Binaire statique (musl) ARMv7 hard-float : zéro dépendance sur le firmware.
//
// Modes : --once (une passe) | --daemon (boucle) | --list (n'affiche que).
// Config : opds-sync.conf (INI), même format que la version Python.

use std::collections::{HashSet, VecDeque};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use url::Url;

const ACQ_REL_PREFIX: &str = "http://opds-spec.org/acquisition";

struct Config {
    base_url: String,
    api_key: String,
    opds_path: String,
    username: String,
    password: String,
    dest: String,
    formats: Vec<String>,
    overwrite: bool,
    max_books: u32,
    verify_tls: bool,
    timeout: u64,
    interval: u64,
}

fn now_str() -> String {
    // horodatage minimal sans dépendance (secondes depuis epoch)
    use std::time::{SystemTime, UNIX_EPOCH};
    let s = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("[{}]", s)
}

fn log(dest: &str, msg: &str) {
    let line = format!("{} {}", now_str(), msg);
    println!("{}", line);
    let p = Path::new(dest).join("opds-sync.log");
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "{}", line);
    }
}

// --- INI minimal -----------------------------------------------------------
fn parse_ini(text: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let mut section = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
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

fn load_config(path: &str) -> Result<Config, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("config {} : {}", path, e))?;
    let m = parse_ini(&text);
    let get = |k: &str, d: &str| m.get(k).cloned().unwrap_or_else(|| d.to_string());
    let getb = |k: &str, d: bool| match m.get(k).map(|s| s.to_lowercase()) {
        Some(v) => v == "true" || v == "yes" || v == "1",
        None => d,
    };
    let getu = |k: &str, d: u64| m.get(k).and_then(|s| s.parse().ok()).unwrap_or(d);
    Ok(Config {
        base_url: get("server.base_url", "").trim_end_matches('/').to_string(),
        api_key: get("server.api_key", "").trim().to_string(),
        opds_path: get("server.opds_path", "/api/opds/{api_key}"),
        username: get("server.username", ""),
        password: get("server.password", ""),
        dest: get("sync.dest", "/mnt/onboard/OPDS"),
        formats: get("sync.formats", "")
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect(),
        overwrite: getb("sync.overwrite", false),
        max_books: getu("sync.max_books", 0) as u32,
        verify_tls: getb("net.verify_tls", true),
        timeout: getu("net.timeout", 30),
        interval: getu("net.interval", 900),
    })
}

fn root_url(c: &Config) -> String {
    format!("{}{}", c.base_url, c.opds_path.replace("{api_key}", &c.api_key))
}

// --- base64 (pour Basic Auth) ---------------------------------------------
fn base64(input: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        out.push(T[(b[0] >> 2) as usize] as char);
        out.push(T[(((b[0] & 0x03) << 4) | (b[1] >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 { T[(((b[1] & 0x0f) << 2) | (b[2] >> 6)) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[(b[2] & 0x3f) as usize] as char } else { '=' });
    }
    out
}

fn agent(c: &Config) -> ureq::Agent {
    if !c.verify_tls {
        log(&c.dest, "NOTE: verify_tls=false n'est pas géré dans le binaire Rust (v1). \
                       HTTPS auto-signé non supporté -> utilise HTTP sur le LAN (réseau coupé, c'est sain), \
                       ou un certificat valide.");
    }
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(c.timeout))
        .user_agent("opds-sync/1.0 (Kobo)")
        .build()
}

struct Acq {
    id: String,
    title: String,
    author: String,
    href: String,
    mime: String,
}

fn req<'a>(ag: &'a ureq::Agent, c: &Config, url: &str) -> ureq::Request {
    let mut r = ag.get(url);
    if !c.username.is_empty() {
        let token = base64(format!("{}:{}", c.username, c.password).as_bytes());
        r = r.set("Authorization", &format!("Basic {}", token));
    }
    r
}

fn parse_feed(xml: &str, base: &str) -> (Vec<String>, Vec<Acq>, Option<String>) {
    let mut navs = Vec::new();
    let mut acqs = Vec::new();
    let mut next = None;
    let doc = match roxmltree::Document::parse(xml) {
        Ok(d) => d,
        Err(_) => return (navs, acqs, next),
    };
    let join = |href: &str| -> Option<String> {
        Url::parse(base).ok().and_then(|b| b.join(href).ok()).map(|u| u.to_string())
    };
    // liens de pagination au niveau du flux
    for link in doc.root_element().children().filter(|n| n.has_tag_name("link")) {
        if link.attribute("rel") == Some("next") {
            if let Some(h) = link.attribute("href") {
                next = join(h);
            }
        }
    }
    for entry in doc.descendants().filter(|n| n.has_tag_name("entry")) {
        let title = entry.children().find(|n| n.has_tag_name("title")).and_then(|n| n.text()).unwrap_or("").trim().to_string();
        let id = entry.children().find(|n| n.has_tag_name("id")).and_then(|n| n.text()).unwrap_or("").trim().to_string();
        let author = entry.children().find(|n| n.has_tag_name("author"))
            .and_then(|a| a.children().find(|n| n.has_tag_name("name")))
            .and_then(|n| n.text()).unwrap_or("").trim().to_string();
        for link in entry.children().filter(|n| n.has_tag_name("link")) {
            let rel = link.attribute("rel").unwrap_or("");
            let typ = link.attribute("type").unwrap_or("");
            let href = match link.attribute("href") {
                Some(h) => h,
                None => continue,
            };
            let abs = match join(href) {
                Some(a) => a,
                None => continue,
            };
            if rel.starts_with(ACQ_REL_PREFIX) {
                acqs.push(Acq { id: if id.is_empty() { abs.clone() } else { id.clone() }, title: title.clone(), author: author.clone(), href: abs, mime: typ.to_string() });
            } else if rel == "subsection" || typ.contains("kind=navigation") || typ.contains("kind=acquisition") {
                navs.push(abs);
            }
        }
    }
    (navs, acqs, next)
}

fn ext_for(acq: &Acq, content_disposition: Option<&str>) -> String {
    if let Some(cd) = content_disposition {
        if let Some(i) = cd.find("filename") {
            let tail = &cd[i..];
            if let Some(eq) = tail.find('=') {
                let name = tail[eq + 1..].trim_matches(|c| c == '"' || c == ' ' || c == '\'' || c == ';');
                if let Some(dot) = name.rfind('.') {
                    let e = name[dot + 1..].to_lowercase();
                    if !e.is_empty() && e.len() <= 5 {
                        return e;
                    }
                }
            }
        }
    }
    let mime = acq.mime.split(';').next().unwrap_or("").trim();
    let e = match mime {
        "application/epub+zip" => "epub",
        "application/x-kobo-epub+zip" => "kepub.epub",
        "application/pdf" => "pdf",
        "application/x-cbz" | "application/vnd.comicbook+zip" => "cbz",
        "application/x-cbr" | "application/vnd.comicbook-rar" => "cbr",
        "application/x-mobipocket-ebook" => "mobi",
        "text/plain" => "txt",
        _ => "",
    };
    if !e.is_empty() {
        return e.to_string();
    }
    // sinon extension de l'URL
    Url::parse(&acq.href).ok()
        .and_then(|u| u.path_segments().and_then(|s| s.last().map(|x| x.to_string())))
        .and_then(|seg| seg.rsplit('.').next().map(|x| x.to_lowercase()))
        .filter(|e| !e.is_empty() && e.len() <= 5)
        .unwrap_or_else(|| "epub".to_string())
}

fn sanitize(name: &str) -> String {
    let mut s: String = name.chars().map(|c| if "/\\:*?\"<>|".contains(c) || (c as u32) < 0x20 { '_' } else { c }).collect();
    s = s.trim().trim_matches('.').to_string();
    if s.len() > 120 {
        s.truncate(120);
    }
    if s.is_empty() { "untitled".to_string() } else { s }
}

fn load_manifest(dest: &str) -> HashSet<String> {
    let p = Path::new(dest).join(".opds-sync-state");
    fs::read_to_string(&p).map(|t| t.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()).unwrap_or_default()
}

fn append_manifest(dest: &str, key: &str) {
    let p = Path::new(dest).join(".opds-sync-state");
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "{}", key);
    }
}

fn download(ag: &ureq::Agent, c: &Config, acq: &Acq) -> Result<bool, String> {
    let resp = req(ag, c, &acq.href).call().map_err(|e| format!("{}", e))?;
    let cd = resp.header("Content-Disposition").map(|s| s.to_string());
    let ext = ext_for(acq, cd.as_deref());
    let base = if acq.author.is_empty() { acq.title.clone() } else { format!("{} - {}", acq.author, acq.title) };
    let fname = format!("{}.{}", sanitize(&base), ext);
    let path = PathBuf::from(&c.dest).join(&fname);
    if path.exists() && !c.overwrite {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}", e))?;
    }
    let tmp = path.with_extension(format!("{}.part", ext));
    let mut reader = resp.into_reader();
    let mut f = fs::File::create(&tmp).map_err(|e| format!("{}", e))?;
    let mut buf = [0u8; 65536];
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("{}", e))?;
        if n == 0 { break; }
        f.write_all(&buf[..n]).map_err(|e| format!("{}", e))?;
    }
    drop(f);
    fs::rename(&tmp, &path).map_err(|e| format!("{}", e))?;
    log(&c.dest, &format!("\u{2913} {}", fname));
    Ok(true)
}

fn crawl(ag: &ureq::Agent, c: &Config, list_only: bool) -> (u32, u32) {
    let _ = fs::create_dir_all(&c.dest);
    let mut manifest = load_manifest(&c.dest);
    let mut queue: VecDeque<String> = VecDeque::new();
    let mut visited: HashSet<String> = HashSet::new();
    queue.push_back(root_url(c));
    let (mut downloaded, mut seen) = (0u32, 0u32);
    while let Some(u) = queue.pop_front() {
        if !visited.insert(u.clone()) { continue; }
        let resp = match req(ag, c, &u).call() {
            Ok(r) => r,
            Err(e) => { log(&c.dest, &format!("WARN flux injoignable {} : {}", u, e)); continue; }
        };
        let body = match resp.into_string() {
            Ok(s) => s,
            Err(e) => { log(&c.dest, &format!("WARN lecture {} : {}", u, e)); continue; }
        };
        let (navs, acqs, next) = parse_feed(&body, &u);
        for n in navs { if !visited.contains(&n) { queue.push_back(n); } }
        if let Some(n) = next { if !visited.contains(&n) { queue.push_back(n); } }
        for acq in acqs {
            seen += 1;
            if !c.formats.is_empty() {
                let e = ext_for(&acq, None);
                let last = e.rsplit('.').next().unwrap_or(&e).to_string();
                if !c.formats.contains(&last) { continue; }
            }
            let key = if acq.id.is_empty() { acq.href.clone() } else { acq.id.clone() };
            if manifest.contains(&key) && !c.overwrite { continue; }
            if list_only {
                log(&c.dest, &format!("\u{2022} {} \u{2014} {}", if acq.author.is_empty() { "?" } else { &acq.author }, acq.title));
                continue;
            }
            match download(ag, c, &acq) {
                Ok(true) => { downloaded += 1; manifest.insert(key.clone()); append_manifest(&c.dest, &key); }
                Ok(false) => { manifest.insert(key.clone()); append_manifest(&c.dest, &key); }
                Err(e) => log(&c.dest, &format!("ERR téléchargement {} : {}", acq.title, e)),
            }
            if c.max_books > 0 && downloaded >= c.max_books {
                log(&c.dest, &format!("Limite max_books={} atteinte.", c.max_books));
                return (downloaded, seen);
            }
        }
    }
    (downloaded, seen)
}

fn reachable(ag: &ureq::Agent, c: &Config) -> bool {
    match ag.get(&c.base_url).call() {
        Ok(_) => true,
        Err(ureq::Error::Status(_, _)) => true, // serveur répond (4xx/5xx) => joignable
        Err(_) => false,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut config = String::new();
    let (mut once, mut daemon, mut list) = (false, false, false);
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--once" => once = true,
            "--daemon" => daemon = true,
            "--list" => list = true,
            "--config" => { i += 1; if i < args.len() { config = args[i].clone(); } }
            "-h" | "--help" => { eprintln!("usage: opds-sync [--config FICHIER] [--once|--daemon|--list]"); return; }
            _ => {}
        }
        i += 1;
    }
    if config.is_empty() {
        // par défaut : à côté de l'exécutable
        let exe = std::env::current_exe().ok();
        config = exe.and_then(|p| p.parent().map(|d| d.join("opds-sync.conf"))).map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| "opds-sync.conf".to_string());
    }
    let c = match load_config(&config) {
        Ok(c) => c,
        Err(e) => { eprintln!("Erreur config : {}", e); std::process::exit(1); }
    };
    if c.base_url.is_empty() || c.api_key.is_empty() {
        eprintln!("Configure base_url et api_key dans {}", config);
        std::process::exit(1);
    }
    let _ = fs::create_dir_all(&c.dest);
    let ag = agent(&c);
    if daemon {
        log(&c.dest, &format!("daemon démarré (interval={}s, dest={})", c.interval, c.dest));
        loop {
            if reachable(&ag, &c) {
                let (n, seen) = crawl(&ag, &c, false);
                log(&c.dest, &format!("sync OK : {} nouveau(x) / {} vus", n, seen));
            } else {
                log(&c.dest, "serveur injoignable, on réessaie plus tard");
            }
            std::thread::sleep(Duration::from_secs(c.interval));
        }
    } else {
        let (n, seen) = crawl(&ag, &c, list);
        if !list {
            log(&c.dest, &format!("Terminé : {} téléchargé(s) / {} vus.", n, seen));
        }
    }
}
