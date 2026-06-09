// kclient — client HTTP mTLS partagé (reqwest bloquant + rustls).
// Identité client (cert + clé PEM) émise par ta CA Caddy ; CA serveur ajoutée aux racines.
use std::fs;
use std::time::Duration;

pub use reqwest::blocking::{Client, RequestBuilder, Response};

#[derive(Clone, Default)]
pub struct MtlsConfig {
    pub client_cert: String, // PEM (chaîne de cert client) — vide = pas de mTLS
    pub client_key: String,  // PEM (clé privée client)
    pub ca_cert: String,     // PEM (CA Caddy à faire confiance pour le serveur) — vide = racines système
    pub insecure: bool,      // accepter un certificat invalide (debug uniquement)
    pub timeout_secs: u64,
    pub user_agent: String,
}

/// Construit un client reqwest configuré pour le mTLS.
pub fn build(cfg: &MtlsConfig) -> Result<Client, String> {
    let ua = if cfg.user_agent.is_empty() { "kobo-syncd/0.1".to_string() } else { cfg.user_agent.clone() };
    let mut b = Client::builder()
        .use_rustls_tls()
        .timeout(Duration::from_secs(if cfg.timeout_secs == 0 { 30 } else { cfg.timeout_secs }))
        .user_agent(ua);

    // Identité client (mTLS) : reqwest attend un PEM unique = clé privée + cert(s).
    if !cfg.client_cert.is_empty() && !cfg.client_key.is_empty() {
        let mut pem = fs::read(&cfg.client_key)
            .map_err(|e| format!("lecture clé client {} : {}", cfg.client_key, e))?;
        pem.push(b'\n');
        pem.extend(fs::read(&cfg.client_cert)
            .map_err(|e| format!("lecture cert client {} : {}", cfg.client_cert, e))?);
        let id = reqwest::Identity::from_pem(&pem)
            .map_err(|e| format!("identité mTLS invalide : {}", e))?;
        b = b.identity(id);
    }

    // CA serveur (Caddy) ajoutée aux racines de confiance.
    if !cfg.ca_cert.is_empty() {
        let pem = fs::read(&cfg.ca_cert).map_err(|e| format!("lecture CA {} : {}", cfg.ca_cert, e))?;
        for cert in reqwest::Certificate::from_pem_bundle(&pem)
            .map_err(|e| format!("CA Caddy illisible : {}", e))? {
            b = b.add_root_certificate(cert);
        }
    }

    if cfg.insecure {
        b = b.danger_accept_invalid_certs(true);
    }
    b.build().map_err(|e| format!("construction client : {}", e))
}

/// Indique si une config mTLS a bien une identité client.
pub fn has_identity(cfg: &MtlsConfig) -> bool {
    !cfg.client_cert.is_empty() && !cfg.client_key.is_empty()
}
