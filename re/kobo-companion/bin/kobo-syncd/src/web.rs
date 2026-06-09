// Panneau de configuration web — servi par kobo-syncd, BIND 127.0.0.1 UNIQUEMENT
// (accessible seulement depuis le navigateur de la liseuse). Optimisé e-ink :
// noir/blanc, gros caractères, gros boutons, ZÉRO JavaScript, rechargement pleine page
// (Post/Redirect/Get). Configure les modules, le serveur OPDS, et déclenche sync/panelize.
use crate::config::{load_features, Config};
use crate::{opds, panelize};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tiny_http::{Header, Method, Request, Response, Server};

type Resp = Response<Cursor<Vec<u8>>>;

#[derive(Default)]
pub struct Status {
    pub syncing: bool,
    pub panelizing: bool,
    pub last_sync: String,
    pub last_panelize: String,
}

// Modules présentés dans le panneau : (id, libellé).
const MODULES: &[(&str, &str)] = &[
    ("opds", "Téléchargement OPDS"),
    ("panelize", "Panelization auto des BD"),
    ("wallabag", "Wallabag (read-it-later)"),
    ("annotations", "Sync surlignages"),
    ("positions", "Positions de lecture"),
    ("stats", "Export stats"),
    ("wifi", "Wi-Fi conditionnel"),
    ("collections", "Collections/séries (au boot)"),
    ("watch", "Watched folder (au boot)"),
    ("sortfilter", "Séries depuis titres (au boot)"),
    ("display", "Moteur d'affichage (display)"),
];

pub fn serve(config_path: String, features_path: String, status: Arc<Mutex<Status>>) {
    // Port lu dans la config (défaut 8080) ; bind STRICTEMENT en local.
    let port = Config::load(&config_path).map(|c| c.getu("web", "port", 8080)).unwrap_or(8080);
    let bind = format!("127.0.0.1:{}", port);
    let server = match Server::http(&bind) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("web: impossible d'écouter sur {} : {}", bind, e);
            return;
        }
    };
    eprintln!("web: panneau de config sur http://{}/ (local uniquement)", bind);
    for mut req in server.incoming_requests() {
        let resp = handle(&mut req, &config_path, &features_path, &status);
        let _ = req.respond(resp);
    }
}

fn handle(req: &mut Request, config_path: &str, features_path: &str, status: &Arc<Mutex<Status>>) -> Resp {
    let method = req.method().clone();
    let url = req.url().to_string();
    let path = url.splitn(2, '?').next().unwrap_or("/").to_string();
    let query = url.splitn(2, '?').nth(1).unwrap_or("").to_string();
    let mut body = String::new();
    if method == Method::Post {
        let _ = req.as_reader().read_to_string(&mut body);
    }
    match (&method, path.as_str()) {
        (Method::Get, "/") => page(config_path, features_path, status, &query),
        (Method::Post, "/save") => {
            let msg = do_save(config_path, features_path, &body);
            redirect(&format!("/?msg={}", urlencode(&msg)))
        }
        (Method::Post, "/sync") => {
            start_sync(config_path.to_string(), features_path.to_string(), status.clone());
            redirect("/?msg=Synchronisation+lancee")
        }
        (Method::Post, "/panelize") => {
            start_panelize(config_path.to_string(), status.clone());
            redirect("/?msg=Panelization+lancee")
        }
        _ => redirect("/"),
    }
}

// ─────────────────────────── Rendu de la page ───────────────────────────

fn page(config_path: &str, features_path: &str, status: &Arc<Mutex<Status>>, query: &str) -> Resp {
    let cfg = Config::load(config_path).ok();
    let feats = load_features(features_path);
    let base_url = cfg.as_ref().map(|c| c.base_url.clone()).unwrap_or_default();
    let api_key = cfg.as_ref().map(|c| c.api_key.clone()).unwrap_or_default();
    let dest = cfg.as_ref().map(|c| c.dest.clone()).unwrap_or_default();
    let fw = read_firewall(config_path);
    let (syncing, panelizing, last_sync, last_panelize) = {
        match status.lock() {
            Ok(s) => (s.syncing, s.panelizing, s.last_sync.clone(), s.last_panelize.clone()),
            Err(_) => (false, false, String::new(), String::new()),
        }
    };
    let flash = query_get(query, "msg");
    let logs = tail_log(&dest, 12);

    let mut h = String::new();
    h.push_str(HEAD);
    h.push_str("<h1>kobo-companion</h1>");
    if !flash.is_empty() {
        h.push_str(&format!("<p class=flash>{}</p>", esc(&flash)));
    }

    // État.
    h.push_str("<div class=card><h2>État</h2><p>");
    h.push_str(if syncing { "● Sync en cours…<br>" } else { "○ Sync au repos<br>" });
    h.push_str(if panelizing { "● Panelization en cours…<br>" } else { "○ Panelization au repos<br>" });
    if !last_sync.is_empty() { h.push_str(&format!("Dernière sync : {}<br>", esc(&last_sync))); }
    if !last_panelize.is_empty() { h.push_str(&format!("Dernière panelization : {}", esc(&last_panelize))); }
    h.push_str("</p></div>");

    // Actions.
    h.push_str("<div class=card><h2>Actions</h2>");
    h.push_str("<form method=post action=/sync><button>↻ Synchroniser maintenant</button></form>");
    h.push_str("<form method=post action=/panelize><button>▦ Paneliser les BD maintenant</button></form>");
    h.push_str("</div>");

    // Configuration (serveur + modules + pare-feu) dans un seul formulaire.
    h.push_str("<form method=post action=/save>");
    h.push_str("<div class=card><h2>Serveur OPDS</h2>");
    h.push_str(&field("URL de base (http://ip:port)", "base_url", &base_url, "text"));
    h.push_str(&field("Clé API", "api_key", &api_key, "text"));
    h.push_str("</div>");

    h.push_str("<div class=card><h2>Modules</h2>");
    for (id, label) in MODULES {
        let on = feats.contains(*id);
        h.push_str(&checkbox(&format!("feat_{}", id), label, on));
    }
    h.push_str("</div>");

    h.push_str("<div class=card><h2>Réseau</h2>");
    h.push_str(&checkbox("firewall", "Couper Internet (n'autoriser que le LAN)", fw));
    h.push_str("</div>");

    h.push_str("<button class=save>Enregistrer la configuration</button></form>");

    // Journaux.
    h.push_str("<div class=card><h2>Journal</h2><pre>");
    h.push_str(&esc(&logs));
    h.push_str("</pre></div>");

    h.push_str("<p><a href=/>↻ Rafraîchir</a></p>");
    h.push_str("</body></html>");

    html(h)
}

// CSS e-ink : contraste max, gros texte, gros boutons, pas d'images ni de JS.
const HEAD: &str = "<!doctype html><html lang=fr><head><meta charset=utf-8>\
<meta name=viewport content='width=device-width, initial-scale=1'>\
<title>kobo-companion</title><style>\
*{box-sizing:border-box}\
body{font-family:sans-serif;font-size:20px;line-height:1.5;color:#000;background:#fff;margin:0;padding:16px;max-width:760px}\
h1{font-size:30px;margin:.2em 0 .4em}h2{font-size:22px;margin:0 0 .4em;border-bottom:3px solid #000;padding-bottom:.2em}\
.card{border:3px solid #000;border-radius:8px;padding:14px;margin:0 0 16px}\
label{display:flex;align-items:center;gap:12px;min-height:52px;border-bottom:1px solid #000;padding:6px 0}\
label:last-child{border-bottom:0}\
input[type=text]{width:100%;font-size:20px;padding:12px;border:2px solid #000;border-radius:6px}\
input[type=checkbox]{width:34px;height:34px;flex:0 0 auto}\
.fieldlabel{display:block;font-weight:bold;margin:.4em 0 .2em}\
button{display:block;width:100%;font-size:21px;font-weight:bold;padding:16px;margin:10px 0;\
background:#fff;color:#000;border:3px solid #000;border-radius:8px}\
button.save{background:#000;color:#fff}\
.flash{border:3px solid #000;background:#000;color:#fff;padding:10px;border-radius:6px;font-weight:bold}\
a{color:#000;font-weight:bold;font-size:21px}\
pre{white-space:pre-wrap;word-break:break-word;font-size:15px;border:1px solid #000;padding:8px;max-height:300px;overflow:auto}\
</style></head><body>";

fn field(label: &str, name: &str, value: &str, ty: &str) -> String {
    format!(
        "<span class=fieldlabel>{}</span><input type={} name={} value=\"{}\">",
        esc(label), ty, name, esc(value)
    )
}
fn checkbox(name: &str, label: &str, on: bool) -> String {
    format!(
        "<label><input type=checkbox name={} value=1{}> {}</label>",
        name, if on { " checked" } else { "" }, esc(label)
    )
}

// ─────────────────────────── Enregistrement ───────────────────────────

fn do_save(config_path: &str, features_path: &str, body: &str) -> String {
    let form = parse_form(body);
    // 1) features.conf : réécriture complète des FEAT_* connus (coché = yes).
    let mut fc = String::from("# Modules activés (panneau web)\n");
    for (id, _) in MODULES {
        let on = form.get(&format!("feat_{}", id)).map(|v| v == "1").unwrap_or(false);
        fc.push_str(&format!("FEAT_{}={}\n", id, if on { "yes" } else { "no" }));
    }
    if let Err(e) = std::fs::write(features_path, fc) {
        return format!("Erreur features.conf: {}", e);
    }
    // 2) serveur OPDS : remplacer base_url / api_key dans kobo-companion.conf.
    if let Some(v) = form.get("base_url") {
        let _ = replace_conf_line(config_path, "base_url", v);
    }
    if let Some(v) = form.get("api_key") {
        let _ = replace_conf_line(config_path, "api_key", v);
    }
    // 3) pare-feu : ENABLE_FIREWALL dans netcut.conf + application immédiate.
    let fw_on = form.get("firewall").map(|v| v == "1").unwrap_or(false);
    set_firewall(config_path, fw_on);

    "Configuration enregistrée".to_string()
}

// Remplace (ou ajoute) une ligne "clé = valeur" dans un fichier INI, en préservant le reste.
fn replace_conf_line(path: &str, key: &str, value: &str) -> std::io::Result<()> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut out = String::new();
    let mut done = false;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with(key) && t[key.len()..].trim_start().starts_with('=') {
            out.push_str(&format!("{} = {}\n", key, value));
            done = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !done {
        out.push_str(&format!("{} = {}\n", key, value));
    }
    std::fs::write(path, out)
}

fn netcut_path(config_path: &str) -> PathBuf {
    Path::new(config_path).parent().unwrap_or(Path::new(".")).join("netcut.conf")
}
fn read_firewall(config_path: &str) -> bool {
    let p = netcut_path(config_path);
    std::fs::read_to_string(p)
        .map(|t| t.lines().any(|l| {
            let l = l.trim();
            l.starts_with("ENABLE_FIREWALL") && l.contains("yes")
        }))
        .unwrap_or(false)
}
fn set_firewall(config_path: &str, on: bool) {
    let p = netcut_path(config_path);
    let text = std::fs::read_to_string(&p).unwrap_or_default();
    let mut out = String::new();
    let mut done = false;
    for line in text.lines() {
        if line.trim_start().starts_with("ENABLE_FIREWALL") {
            out.push_str(&format!("ENABLE_FIREWALL=\"{}\"\n", if on { "yes" } else { "no" }));
            done = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !done {
        out.push_str(&format!("ENABLE_FIREWALL=\"{}\"\n", if on { "yes" } else { "no" }));
    }
    let _ = std::fs::write(&p, out);
    // Application immédiate (best-effort).
    let script = "/usr/local/Kobo/opds-netcut.sh";
    if Path::new(script).exists() {
        let _ = std::process::Command::new(script).arg(if on { "start" } else { "open" }).output();
    }
}

// ─────────────────────────── Jobs async ───────────────────────────

fn start_sync(config_path: String, features_path: String, status: Arc<Mutex<Status>>) {
    {
        if let Ok(mut s) = status.lock() {
            if s.syncing { return; }
            s.syncing = true;
        }
    }
    std::thread::spawn(move || {
        let res = run_sync(&config_path, &features_path);
        if let Ok(mut s) = status.lock() {
            s.syncing = false;
            s.last_sync = res;
        }
    });
}

fn run_sync(config_path: &str, features_path: &str) -> String {
    let c = match Config::load(config_path) { Ok(c) => c, Err(e) => return format!("config: {}", e) };
    let client = match kclient::build(&c.mtls) { Ok(x) => x, Err(e) => return format!("client: {}", e) };
    let feats = load_features(features_path);
    if feats.contains("opds") && !c.base_url.is_empty() && !c.api_key.is_empty() {
        let (n, seen) = opds::sync(&client, &c, false);
        format!("OPDS {} nouveau(x) / {} vus", n, seen)
    } else {
        "OPDS non configuré".to_string()
    }
}

fn start_panelize(config_path: String, status: Arc<Mutex<Status>>) {
    {
        if let Ok(mut s) = status.lock() {
            if s.panelizing { return; }
            s.panelizing = true;
        }
    }
    std::thread::spawn(move || {
        let res = match Config::load(&config_path) {
            Ok(c) => panelize::run(&c).unwrap_or_else(|e| format!("erreur: {}", e)),
            Err(e) => format!("config: {}", e),
        };
        if let Ok(mut s) = status.lock() {
            s.panelizing = false;
            s.last_panelize = res;
        }
    });
}

// ─────────────────────────── Utilitaires ───────────────────────────

fn html(body: String) -> Resp {
    let h = Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap();
    Response::from_string(body).with_header(h)
}
fn redirect(loc: &str) -> Resp {
    let h = Header::from_bytes(&b"Location"[..], loc.as_bytes()).unwrap();
    Response::from_string("").with_status_code(303).with_header(h)
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn parse_form(body: &str) -> std::collections::HashMap<String, String> {
    let mut m = std::collections::HashMap::new();
    for pair in body.split('&') {
        if pair.is_empty() { continue; }
        let mut it = pair.splitn(2, '=');
        let k = urldecode(it.next().unwrap_or(""));
        let v = urldecode(it.next().unwrap_or(""));
        m.insert(k, v);
    }
    m
}
fn query_get(query: &str, key: &str) -> String {
    for pair in query.split('&') {
        let mut it = pair.splitn(2, '=');
        if it.next() == Some(key) {
            return urldecode(it.next().unwrap_or(""));
        }
    }
    String::new()
}
fn urldecode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => { out.push(b' '); i += 1; }
            b'%' if i + 2 < b.len() => {
                let hi = (b[i + 1] as char).to_digit(16);
                let lo = (b[i + 2] as char).to_digit(16);
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                } else {
                    out.push(b[i]); i += 1;
                }
            }
            c => { out.push(c); i += 1; }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn tail_log(dest: &str, n: usize) -> String {
    let p = Path::new(dest).join("kobo-syncd.log");
    match std::fs::read_to_string(&p) {
        Ok(t) => {
            let lines: Vec<&str> = t.lines().collect();
            let start = lines.len().saturating_sub(n);
            lines[start..].join("\n")
        }
        Err(_) => "(pas de journal)".to_string(),
    }
}
