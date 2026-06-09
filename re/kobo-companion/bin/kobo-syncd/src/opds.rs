// Module OPDS : parcourt le catalogue (Kavita…) via le client mTLS partagé et
// télécharge les livres absents dans le dossier cible. Porté depuis opds-sync (ureq -> reqwest).
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use url::Url;

use kclient::Client;
use crate::config::Config;
use crate::log;

const ACQ_REL_PREFIX: &str = "http://opds-spec.org/acquisition";

pub struct Acq {
    pub id: String,
    pub title: String,
    pub author: String,
    pub href: String,
    pub mime: String,
}

fn get_text(client: &Client, url: &str) -> Result<String, String> {
    let resp = client.get(url).send().map_err(|e| format!("{}", e))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    resp.text().map_err(|e| format!("{}", e))
}

pub fn parse_feed(xml: &str, base: &str) -> (Vec<String>, Vec<Acq>, Option<String>) {
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
            let href = match link.attribute("href") { Some(h) => h, None => continue };
            let abs = match join(href) { Some(a) => a, None => continue };
            if rel.starts_with(ACQ_REL_PREFIX) {
                acqs.push(Acq { id: if id.is_empty() { abs.clone() } else { id.clone() }, title: title.clone(), author: author.clone(), href: abs, mime: typ.to_string() });
            } else if rel == "subsection" || typ.contains("kind=navigation") || typ.contains("kind=acquisition") {
                navs.push(abs);
            }
        }
    }
    (navs, acqs, next)
}

pub fn ext_for(acq: &Acq, content_disposition: Option<&str>) -> String {
    if let Some(cd) = content_disposition {
        if let Some(i) = cd.find("filename") {
            let tail = &cd[i..];
            if let Some(eq) = tail.find('=') {
                let name = tail[eq + 1..].trim_matches(|c| c == '"' || c == ' ' || c == '\'' || c == ';');
                if let Some(dot) = name.rfind('.') {
                    let e = name[dot + 1..].to_lowercase();
                    if !e.is_empty() && e.len() <= 5 { return e; }
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
    if !e.is_empty() { return e.to_string(); }
    Url::parse(&acq.href).ok()
        .and_then(|u| u.path_segments().and_then(|s| s.last().map(|x| x.to_string())))
        .and_then(|seg| seg.rsplit('.').next().map(|x| x.to_lowercase()))
        .filter(|e| !e.is_empty() && e.len() <= 5)
        .unwrap_or_else(|| "epub".to_string())
}

pub fn sanitize(name: &str) -> String {
    let mut s: String = name.chars().map(|c| if "/\\:*?\"<>|".contains(c) || (c as u32) < 0x20 { '_' } else { c }).collect();
    s = s.trim().trim_matches('.').to_string();
    if s.len() > 120 { s.truncate(120); }
    if s.is_empty() { "untitled".to_string() } else { s }
}

fn load_manifest(dest: &str) -> HashSet<String> {
    let p = Path::new(dest).join(".opds-sync-state");
    fs::read_to_string(p).map(|t| t.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()).unwrap_or_default()
}
fn append_manifest(dest: &str, key: &str) {
    let p = Path::new(dest).join(".opds-sync-state");
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(p) {
        let _ = writeln!(f, "{}", key);
    }
}

fn download(client: &Client, c: &Config, acq: &Acq) -> Result<bool, String> {
    let resp = client.get(&acq.href).send().map_err(|e| format!("{}", e))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let cd = resp.headers().get("content-disposition")
        .and_then(|v| v.to_str().ok()).map(|s| s.to_string());
    let ext = ext_for(acq, cd.as_deref());
    let base = if acq.author.is_empty() { acq.title.clone() } else { format!("{} - {}", acq.author, acq.title) };
    let fname = format!("{}.{}", sanitize(&base), ext);
    let path = PathBuf::from(&c.dest).join(&fname);
    if path.exists() && !c.overwrite { return Ok(false); }
    if let Some(dir) = path.parent() { fs::create_dir_all(dir).map_err(|e| format!("{}", e))?; }
    let tmp = path.with_extension(format!("{}.part", ext));
    let mut resp = resp;
    let mut f = fs::File::create(&tmp).map_err(|e| format!("{}", e))?;
    resp.copy_to(&mut f).map_err(|e| format!("{}", e))?;
    drop(f);
    fs::rename(&tmp, &path).map_err(|e| format!("{}", e))?;
    log(&c.dest, &format!("\u{2913} {}", fname));
    Ok(true)
}

pub fn sync(client: &Client, c: &Config, list_only: bool) -> (u32, u32) {
    let _ = fs::create_dir_all(&c.dest);
    let mut manifest = load_manifest(&c.dest);
    let mut queue: VecDeque<String> = VecDeque::new();
    let mut visited: HashSet<String> = HashSet::new();
    queue.push_back(c.root_url());
    let (mut downloaded, mut seen) = (0u32, 0u32);
    while let Some(u) = queue.pop_front() {
        if !visited.insert(u.clone()) { continue; }
        let body = match get_text(client, &u) {
            Ok(b) => b,
            Err(e) => { log(&c.dest, &format!("WARN flux {} : {}", u, e)); continue; }
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
            match download(client, c, &acq) {
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
