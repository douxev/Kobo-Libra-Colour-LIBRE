// Tris / filtres custom — déduction de séries depuis les titres.
//
// Réalité Nickel :
//  - Nickel n'expose PAS de tri arbitraire pilotable par la base : l'ordre d'affichage
//    (par titre, auteur, date, série…) est codé dans le binaire libnickel. Le seul levier
//    « propre » est de remplir content.Series / content.SeriesNumber, que Nickel sait
//    afficher et regrouper. Un tri UI totalement arbitraire nécessiterait un patch binaire
//    de Nickel — hors de portée de ce module (documenté dans le résumé).
//  - config : cfg["sortfilter.series_from_title"] = yes/true -> pour chaque content
//    (ContentType='6') sans Series, on tente de déduire Série + Numéro depuis le Title.
//  - Idempotent (on n'écrit que les entrées sans Series) et robuste (jamais de panique).
use rusqlite::Connection;
use std::collections::HashMap;

pub fn run(conn: &Connection, cfg: &HashMap<String, String>) -> Result<String, String> {
    // Drapeau d'activation : accepte yes/true/1/on (insensible à la casse).
    let flag = cfg
        .get("sortfilter.series_from_title")
        .map(|v| v.trim().to_lowercase())
        .unwrap_or_default();
    let enabled = matches!(flag.as_str(), "yes" | "true" | "1" | "on");
    if !enabled {
        return Ok(
            "sortfilter.series_from_title désactivé ; aucune modification. \
             Note : Nickel ne permet pas de tri UI arbitraire via la base (patch binaire requis)."
                .to_string(),
        );
    }

    // Vérifie que les colonnes Series / SeriesNumber existent dans `content`.
    if !columns_exist(conn, "content", &["Series", "SeriesNumber"])? {
        return Ok(
            "Colonnes Series/SeriesNumber absentes de `content` : firmware trop ancien, \
             rien à faire. Note : un tri UI arbitraire nécessiterait un patch binaire de Nickel."
                .to_string(),
        );
    }

    // Sélection des livres sideloadés sans série renseignée.
    let mut stmt = conn
        .prepare(
            "SELECT ContentID, Title FROM content \
             WHERE ContentType = '6' \
               AND Title IS NOT NULL AND Title <> '' \
               AND (Series IS NULL OR Series = '')",
        )
        .map_err(|e| format!("préparation SELECT : {}", e))?;

    let rows = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let title: String = row.get(1)?;
            Ok((id, title))
        })
        .map_err(|e| format!("lecture content : {}", e))?;

    // Collecte d'abord (on ne peut pas tenir le statement de lecture ouvert pendant l'UPDATE).
    let mut candidates: Vec<(String, String)> = Vec::new();
    for r in rows {
        match r {
            Ok(pair) => candidates.push(pair),
            Err(e) => return Err(format!("itération content : {}", e)),
        }
    }
    drop(stmt);

    let mut updated = 0usize;
    for (id, title) in candidates {
        if let Some((series, number)) = parse_series(&title) {
            let res = conn.execute(
                "UPDATE content SET Series = ?1, SeriesNumber = ?2 \
                 WHERE ContentID = ?3 AND (Series IS NULL OR Series = '')",
                rusqlite::params![series, number, id],
            );
            match res {
                Ok(n) if n > 0 => updated += 1,
                Ok(_) => {} // déjà renseigné entre-temps : idempotent
                Err(e) => return Err(format!("UPDATE {} : {}", id, e)),
            }
        }
    }

    Ok(format!(
        "{} séries déduites depuis les titres. \
         Note honnête : ceci remplit content.Series/SeriesNumber (que Nickel sait afficher) ; \
         un tri/filtre UI réellement arbitraire resterait impossible sans patch binaire de Nickel.",
        updated
    ))
}

/// Vérifie que toutes les colonnes demandées existent dans la table via PRAGMA table_info.
fn columns_exist(conn: &Connection, table: &str, wanted: &[&str]) -> Result<bool, String> {
    // table_info ne supporte pas le bind de paramètre pour le nom de table ; on contrôle
    // donc le nom (ici constante interne « content ») et on l'injecte sans guillemet utilisateur.
    let sql = format!("PRAGMA table_info({})", table);
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("PRAGMA table_info : {}", e))?;
    let names = stmt
        .query_map([], |row| row.get::<_, String>(1)) // colonne 1 = name
        .map_err(|e| format!("lecture PRAGMA : {}", e))?;
    let mut found: Vec<String> = Vec::new();
    for n in names {
        match n {
            Ok(name) => found.push(name),
            Err(e) => return Err(format!("itération PRAGMA : {}", e)),
        }
    }
    Ok(wanted
        .iter()
        .all(|w| found.iter().any(|f| f.eq_ignore_ascii_case(w))))
}

/// Déduit (Série, Numéro) depuis un titre, sans crate regex.
/// Motifs reconnus (sur le SUFFIXE du titre) :
///   "Nom T03" / "Nom t3"          -> ("Nom", "3")
///   "Nom, tome 3" / "Nom tome 3"  -> ("Nom", "3")
///   "Nom #3"                      -> ("Nom", "3")
///   "Nom Vol. 3" / "Nom vol 3"    -> ("Nom", "3")
/// Le numéro est normalisé (zéros de tête retirés). Renvoie None si rien ne matche
/// ou si le nom de série déduit est vide.
fn parse_series(title: &str) -> Option<(String, String)> {
    let t = title.trim();
    if t.is_empty() {
        return None;
    }

    // On découpe le titre sur le dernier séparateur plausible et on teste chaque motif.
    // Chaque tentative renvoie (debut_du_nom, chaine_numero_brute).
    let attempt = match_marker(t)?;
    let (name_part, num_raw) = attempt;

    let name = name_part.trim().trim_end_matches([',', '-', ':', ' ']).trim();
    if name.is_empty() {
        return None;
    }
    let num = normalize_number(num_raw)?;
    Some((name.to_string(), num))
}

/// Cherche un marqueur de tome en fin de titre et renvoie (texte_avant_marqueur, numéro_brut).
fn match_marker(t: &str) -> Option<(&str, &str)> {
    // 1) "#3" : on cherche le dernier '#' suivi de chiffres jusqu'à la fin.
    if let Some(pos) = t.rfind('#') {
        let after = &t[pos + 1..];
        if is_all_digits(after.trim()) && !after.trim().is_empty() {
            return Some((&t[..pos], after.trim()));
        }
    }

    // Pour les marqueurs alphabétiques, on parcourt les « mots » et on repère le dernier
    // mot-clé (tome/t/vol/vol./volume) ; le numéro est soit collé (T03) soit le mot suivant.
    // IMPORTANT : on minuscule en ASCII uniquement (A–Z -> a–z), ce qui préserve EXACTEMENT
    // la longueur en octets et donc l'alignement des offsets avec `t`. Un to_lowercase()
    // Unicode pourrait changer la taille (ex. « İ » -> « i̇ ») et désaligner les slices `t.get(..)`.
    let lower = t.to_ascii_lowercase();
    let bytes = lower.as_bytes();

    // Liste des préfixes de marqueur, du plus long au plus court pour éviter les faux-positifs.
    const MARKERS: [&str; 5] = ["volume", "vol.", "vol", "tome", "t"];

    // On cherche la dernière occurrence d'un marqueur en frontière de mot.
    let mut best: Option<(usize, usize, &str)> = None; // (debut_marqueur, fin_marqueur, marqueur)
    for m in MARKERS.iter() {
        let mut from = 0usize;
        while let Some(rel) = lower[from..].find(m) {
            let start = from + rel;
            let end = start + m.len();
            // Frontière gauche : début ou non-alphanumérique juste avant.
            let left_ok = start == 0
                || !bytes
                    .get(start - 1)
                    .map(|b| b.is_ascii_alphanumeric())
                    .unwrap_or(false);
            if left_ok {
                // Garder la plus tardive (fin la plus grande).
                if best.map(|(_, e, _)| start > e).unwrap_or(true) {
                    best = Some((start, end, m));
                }
            }
            from = start + 1;
        }
    }

    let (mstart, mend, _marker) = best?;
    // Récupère ce qui suit le marqueur : éventuel '.', espaces, puis chiffres jusqu'au bout.
    let tail = t.get(mend..)?; // sur le titre original (casse préservée pour le nom)
    let tail_trim = tail.trim_start_matches(['.', ' ', '\t']);
    // Le numéro doit aller jusqu'à la fin du titre (marqueur en suffixe).
    let num = tail_trim.trim();
    if num.is_empty() || !is_all_digits(num) {
        return None;
    }
    let name_part = t.get(..mstart)?;
    Some((name_part, num))
}

fn is_all_digits(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

/// Normalise un numéro : enlève les zéros de tête, garde au moins "0" si tout est zéro.
fn normalize_number(raw: &str) -> Option<String> {
    let digits = raw.trim();
    if !is_all_digits(digits) {
        return None;
    }
    let trimmed = digits.trim_start_matches('0');
    if trimmed.is_empty() {
        Some("0".to_string())
    } else {
        Some(trimmed.to_string())
    }
}
