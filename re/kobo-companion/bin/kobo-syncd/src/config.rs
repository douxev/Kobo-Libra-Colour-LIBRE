// Config (INI) + features.conf. Cœur typé (server/sync/mtls) + accès générique
// (chaque module lit ses propres clés via c.gets/getb/getu/getlist).
use std::collections::{HashMap, HashSet};
use std::fs;
use kclient::MtlsConfig;

pub struct Config {
    pub raw: HashMap<String, String>, // "section.clé" -> valeur (accès générique pour les modules)
    pub base_url: String,
    pub api_key: String,
    pub opds_path: String,
    pub dest: String,
    pub formats: Vec<String>,
    pub overwrite: bool,
    pub max_books: u32,
    pub interval: u64,
    pub mtls: MtlsConfig,
}

pub fn parse_ini(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
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
            if val.len() >= 2
                && ((val.starts_with('"') && val.ends_with('"'))
                    || (val.starts_with('\'') && val.ends_with('\'')))
            {
                val = val[1..val.len() - 1].to_string();
            }
            map.insert(format!("{}.{}", section, key), val);
        }
    }
    map
}

impl Config {
    pub fn load(path: &str) -> Result<Config, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("config {} : {}", path, e))?;
        let raw = parse_ini(&text);
        let get = |k: &str, d: &str| raw.get(k).cloned().unwrap_or_else(|| d.to_string());
        let getb = |k: &str, d: bool| match raw.get(k).map(|s| s.to_lowercase()) {
            Some(v) => v == "true" || v == "yes" || v == "1",
            None => d,
        };
        let getu = |k: &str, d: u64| raw.get(k).and_then(|s| s.parse().ok()).unwrap_or(d);
        Ok(Config {
            base_url: get("server.base_url", "").trim_end_matches('/').to_string(),
            api_key: get("server.api_key", "").trim().to_string(),
            opds_path: get("server.opds_path", "/api/opds/{api_key}"),
            dest: get("sync.dest", "/mnt/onboard/OPDS"),
            formats: get("sync.formats", "")
                .split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect(),
            overwrite: getb("sync.overwrite", false),
            max_books: getu("sync.max_books", 0) as u32,
            interval: getu("net.interval", 900),
            mtls: MtlsConfig {
                client_cert: get("mtls.client_cert", ""),
                client_key: get("mtls.client_key", ""),
                ca_cert: get("mtls.ca_cert", ""),
                insecure: getb("net.insecure", false),
                timeout_secs: getu("net.timeout", 30),
                user_agent: "kobo-syncd/0.1".to_string(),
            },
            raw,
        })
    }

    pub fn root_url(&self) -> String {
        format!("{}{}", self.base_url, self.opds_path.replace("{api_key}", &self.api_key))
    }

    // --- accès générique pour les modules (clé = "section.clé") ---
    pub fn gets(&self, section: &str, key: &str, default: &str) -> String {
        self.raw.get(&format!("{}.{}", section, key)).cloned().unwrap_or_else(|| default.to_string())
    }
    pub fn getb(&self, section: &str, key: &str, default: bool) -> bool {
        match self.raw.get(&format!("{}.{}", section, key)).map(|s| s.to_lowercase()) {
            Some(v) => v == "true" || v == "yes" || v == "1",
            None => default,
        }
    }
    pub fn getu(&self, section: &str, key: &str, default: u64) -> u64 {
        self.raw.get(&format!("{}.{}", section, key)).and_then(|s| s.parse().ok()).unwrap_or(default)
    }
    /// Liste séparée par virgules.
    pub fn getlist(&self, section: &str, key: &str) -> Vec<String> {
        self.gets(section, key, "").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
    }
    /// Chemin de la base Nickel (défaut standard).
    pub fn kobo_db(&self) -> String {
        self.gets("paths", "kobo_db", "/mnt/onboard/.kobo/KoboReader.sqlite")
    }
}

/// Modules activés (features.conf : lignes `FEAT_<id>=yes`).
pub fn load_features(path: &str) -> HashSet<String> {
    let mut set = HashSet::new();
    if let Ok(text) = fs::read_to_string(path) {
        for line in text.lines() {
            let l = line.trim();
            if l.is_empty() || l.starts_with('#') { continue; }
            if let Some(eq) = l.find('=') {
                let k = l[..eq].trim();
                let v = l[eq + 1..].trim().trim_matches('"').to_lowercase();
                if let Some(id) = k.strip_prefix("FEAT_") {
                    if v == "yes" || v == "true" || v == "1" { set.insert(id.to_lowercase()); }
                }
            }
        }
    }
    set
}
