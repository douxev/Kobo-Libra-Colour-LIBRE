// Module panelization BD : lance le binaire comic-panelize en batch sur le dossier des BD.
// Utilisé par l'auto-panelization (après sync si FEAT_panelize) et par le bouton du panneau web.
use crate::config::Config;
use std::path::Path;
use std::process::Command;

/// Panelise toutes les BD (.cbz) du dossier configuré via comic-panelize --batch.
/// Renvoie un résumé ("N BD panelisée(s)") ou une erreur (dégrade proprement).
pub fn run(c: &Config) -> Result<String, String> {
    let bin = c.gets("panelize", "bin", "/usr/local/Kobo/comic-panelize");
    if !Path::new(&bin).exists() {
        return Err(format!("binaire introuvable: {}", bin));
    }
    // Dossier des BD : par défaut le dossier de destination OPDS.
    let dir = c.gets("panelize", "comics_dir", &c.dest);
    if !Path::new(&dir).is_dir() {
        return Err(format!("dossier introuvable: {}", dir));
    }

    let mut cmd = Command::new(&bin);
    cmd.arg("--batch").arg(&dir);
    cmd.arg("--direction").arg(c.gets("panelize", "direction", "ltr"));
    cmd.arg("--canvas").arg(c.gets("panelize", "canvas", "1264x1680"));
    cmd.arg("--quality").arg(c.getu("panelize", "quality", 88).to_string());
    if c.getb("panelize", "full_page", false) {
        cmd.arg("--full-page");
    }
    if c.getb("panelize", "no_upscale", false) {
        cmd.arg("--no-upscale");
    }
    // Dossier de sortie optionnel (sinon à côté des originaux).
    let out_dir = c.gets("panelize", "out_dir", "");
    if !out_dir.is_empty() {
        cmd.arg("--out-dir").arg(&out_dir);
    }

    let out = cmd.output().map_err(|e| format!("exec {}: {}", bin, e))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let n = stdout.lines().filter(|l| l.contains(" -> ")).count();
    if out.status.success() {
        Ok(format!("{} BD panelisée(s) [{}]", n, dir))
    } else {
        Err(format!(
            "comic-panelize a échoué: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}
