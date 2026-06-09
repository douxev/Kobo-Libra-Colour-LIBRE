// Sidecar JSON + hash source.
use crate::Sidecar;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

// Hash sha256 du fichier (lecture par blocs de 8 Kio), format "sha256:<hex minuscule>".
pub fn source_hash(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    Ok(format!("sha256:{:x}", digest))
}

// Écrit le sidecar en JSON pretty. Écriture atomique (.tmp puis rename).
pub fn write_sidecar(path: &Path, sc: &Sidecar) -> Result<(), String> {
    let json = serde_json::to_string_pretty(sc).map_err(|e| e.to_string())?;
    // Fichier temporaire à côté de la cible pour garantir un rename atomique (même volume).
    let tmp = path.with_extension("tmp");
    {
        let mut file = File::create(&tmp).map_err(|e| e.to_string())?;
        file.write_all(json.as_bytes()).map_err(|e| e.to_string())?;
        file.flush().map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(())
}

// Lit et désérialise un sidecar JSON.
pub fn read_sidecar(path: &Path) -> Result<Sidecar, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut contents = String::new();
    file.read_to_string(&mut contents).map_err(|e| e.to_string())?;
    serde_json::from_str(&contents).map_err(|e| e.to_string())
}
