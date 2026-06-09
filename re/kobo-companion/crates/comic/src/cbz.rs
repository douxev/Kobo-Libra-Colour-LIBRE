// Lecture/écriture des CBZ (zip d'images).
use image::DynamicImage;
use std::cmp::Ordering;
use std::io::{Read, Write};
use std::path::Path;

pub struct PageImage {
    pub name: String,
    pub image: DynamicImage,
}

/// Extensions d'images reconnues (en minuscules, sans le point).
const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "webp"];

/// Renvoie le nom de fichier final (sans le chemin du dossier).
fn base_name(name: &str) -> &str {
    // Les zip utilisent '/' comme séparateur ; on prend ce qui suit le dernier.
    match name.rfind('/') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

/// Vrai si l'entrée est une image à garder (extension valide, non cachée, hors __MACOSX/).
fn is_kept_image(name: &str) -> bool {
    if name.starts_with("__MACOSX/") || name.contains("/__MACOSX/") {
        return false;
    }
    let base = base_name(name);
    // Ignorer les entrées cachées (commençant par '.', ex. ressources AppleDouble "._x").
    if base.starts_with('.') || base.is_empty() {
        return false;
    }
    // Extension en minuscules.
    let ext = match base.rfind('.') {
        Some(i) => base[i + 1..].to_ascii_lowercase(),
        None => return false,
    };
    IMAGE_EXTS.contains(&ext.as_str())
}

/// Comparaison "naturelle" : on découpe en blocs de chiffres / non-chiffres,
/// et les blocs numériques sont comparés par valeur ("p2" < "p10").
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    // Comparer deux blocs de chiffres par valeur numérique.
                    let na: String = collect_digits(&mut ai);
                    let nb: String = collect_digits(&mut bi);
                    // Comparer en ignorant les zéros de tête, puis longueur, puis lexicographique.
                    let ta = na.trim_start_matches('0');
                    let tb = nb.trim_start_matches('0');
                    let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                    if ord != Ordering::Equal {
                        return ord;
                    }
                    // Égaux en valeur : départager par nombre de zéros de tête (stable, déterministe).
                    let ord = na.len().cmp(&nb.len());
                    if ord != Ordering::Equal {
                        return ord;
                    }
                } else {
                    // Comparer deux blocs non-numériques, caractère par caractère, insensible à la casse.
                    let sa = collect_non_digits(&mut ai);
                    let sb = collect_non_digits(&mut bi);
                    let la = sa.to_ascii_lowercase();
                    let lb = sb.to_ascii_lowercase();
                    let ord = la.cmp(&lb).then_with(|| sa.cmp(&sb));
                    if ord != Ordering::Equal {
                        return ord;
                    }
                }
            }
        }
    }
}

fn collect_digits(it: &mut std::iter::Peekable<std::str::Chars>) -> String {
    let mut s = String::new();
    while let Some(&c) = it.peek() {
        if c.is_ascii_digit() {
            s.push(c);
            it.next();
        } else {
            break;
        }
    }
    s
}

fn collect_non_digits(it: &mut std::iter::Peekable<std::str::Chars>) -> String {
    let mut s = String::new();
    while let Some(&c) = it.peek() {
        if !c.is_ascii_digit() {
            s.push(c);
            it.next();
        } else {
            break;
        }
    }
    s
}

/// Ouvre le CBZ, décode et trie (ordre naturel) les pages images.
/// Tolérant : une image illisible est seulement signalée sur stderr, sans échec global.
pub fn read_pages(cbz_path: &Path) -> Result<Vec<PageImage>, String> {
    let file = std::fs::File::open(cbz_path)
        .map_err(|e| format!("ouverture {}: {}", cbz_path.display(), e))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("lecture zip {}: {}", cbz_path.display(), e))?;

    // 1) Collecter (nom, octets) des entrées images valides.
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..archive.len() {
        let mut entry = match archive.by_index(i) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("warn: entrée zip #{} illisible: {}", i, e);
                continue;
            }
        };
        if !entry.is_file() {
            continue; // dossiers ignorés
        }
        let name = entry.name().to_string();
        if !is_kept_image(&name) {
            continue;
        }
        let mut buf = Vec::new();
        if let Err(e) = entry.read_to_end(&mut buf) {
            eprintln!("warn: lecture de '{}' échouée: {}", name, e);
            continue;
        }
        entries.push((name, buf));
    }

    // 2) Trier par ordre naturel du nom AVANT décodage (ordre déterministe).
    entries.sort_by(|a, b| natural_cmp(&a.0, &b.0));

    // 3) Décoder ; une image illisible est sautée (warn).
    let mut pages: Vec<PageImage> = Vec::with_capacity(entries.len());
    for (name, bytes) in entries {
        match image::load_from_memory(&bytes) {
            Ok(img) => pages.push(PageImage { name, image: img }),
            Err(e) => eprintln!("warn: décodage de '{}' échoué: {}", name, e),
        }
    }

    Ok(pages)
}

/// Écrit un CBZ (zip Stored) de façon atomique : fichier temporaire puis rename.
pub fn write_cbz(out_path: &Path, pages: &[(String, Vec<u8>)]) -> Result<(), String> {
    let tmp_path = out_path.with_extension("tmp");

    // Bloc d'écriture isolé pour fermer le fichier avant le rename.
    {
        let tmp_file = std::fs::File::create(&tmp_path)
            .map_err(|e| format!("création {}: {}", tmp_path.display(), e))?;
        let mut writer = zip::ZipWriter::new(tmp_file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        for (name, bytes) in pages {
            writer
                .start_file(name.as_str(), opts)
                .map_err(|e| format!("ajout '{}': {}", name, e))?;
            writer
                .write_all(bytes)
                .map_err(|e| format!("écriture '{}': {}", name, e))?;
        }
        writer
            .finish()
            .map_err(|e| format!("finalisation {}: {}", tmp_path.display(), e))?;
    }

    // Rename atomique vers la destination finale.
    std::fs::rename(&tmp_path, out_path).map_err(|e| {
        // Nettoyage best-effort du temporaire en cas d'échec.
        let _ = std::fs::remove_file(&tmp_path);
        format!(
            "renommage {} -> {}: {}",
            tmp_path.display(),
            out_path.display(),
            e
        )
    })?;

    Ok(())
}
