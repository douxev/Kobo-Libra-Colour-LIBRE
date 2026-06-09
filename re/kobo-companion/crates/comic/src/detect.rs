// Détection des cases d'une page — implémentation Rust pure (sans OpenCV).
//
// Pipeline : binarisation (Otsu) -> rognage des marges -> XY-cut récursif ->
// secours composantes connexes sur les feuilles trop grandes -> nettoyage final.
// Constantes calibrées pour des pages ~1600x2400. Ne panique jamais (indices bornés).
use crate::Bbox;
use image::GrayImage;

// --- Constantes de réglage (commentées) ---
// Seuil de repli si Otsu échoue : un luma < 200 est considéré comme "encre".
const FALLBACK_THRESHOLD: u8 = 200;
// Ratio d'encre max d'une ligne/colonne pour la considérer "vide" (gouttière). 0.5%.
const GUTTER_EPS: f32 = 0.005;
// Largeur min d'une gouttière, en fraction de la dimension correspondante. 1.5%.
const MIN_GUTTER_FRAC: f32 = 0.015;
// Taille min d'un panel, en fraction de la dimension page (largeur ET hauteur). 8%.
const MIN_PANEL_FRAC: f32 = 0.08;
// Aire min finale d'une boîte, en fraction de l'aire page. 2%.
const MIN_AREA_FRAC: f64 = 0.02;
// Seuil de marge externe : une bande de bord est rognée si ratio d'encre < 0.3%.
const MARGIN_EPS: f32 = 0.003;
// Taille de bloc pour le sous-échantillonnage des composantes connexes (vitesse).
const CC_BLOCK: u32 = 8;
// Une feuille XY-cut est "grande" (donc candidate aux composantes connexes) si elle
// couvre au moins 70% de l'aire de la région rognée initiale.
const LARGE_LEAF_FRAC: f64 = 0.70;

/// Détecte les cases d'une page. Renvoie des boîtes NON ordonnées.
/// Repli : si rien de fiable -> une seule boîte = page entière.
pub fn detect_panels(gray: &GrayImage) -> Vec<Bbox> {
    let w = gray.width();
    let h = gray.height();
    let full = Bbox::new(0, 0, w, h);
    // Page dégénérée -> repli immédiat.
    if w == 0 || h == 0 {
        return vec![full];
    }

    // 1) Seuil binaire (Otsu, repli seuil fixe).
    let threshold = otsu_threshold(gray).unwrap_or(FALLBACK_THRESHOLD);

    // Masque "encre" bool (true = encre). Bornes garanties par width*height.
    let ink: Vec<bool> = gray
        .as_raw()
        .iter()
        .map(|&p| p < threshold)
        .collect();
    // Sécurité si le buffer ne fait pas exactement w*h (multi-canaux improbable pour Luma).
    if ink.len() != (w as usize).saturating_mul(h as usize) {
        return vec![full];
    }

    // 2) Rognage des marges externes uniformes.
    let region = trim_margins(&ink, w, h);
    if region.width() == 0 || region.height() == 0 {
        return vec![full];
    }

    // 3) XY-cut récursif sur la région rognée.
    let min_panel_w = ((w as f32) * MIN_PANEL_FRAC) as u32;
    let min_panel_h = ((h as f32) * MIN_PANEL_FRAC) as u32;
    let mut leaves: Vec<Bbox> = Vec::new();
    xy_cut(&ink, w, h, region, min_panel_w, min_panel_h, 0, &mut leaves);

    // 4) Secours composantes connexes sur les feuilles restées ~aussi grandes que la région.
    let region_area = region.area();
    let mut boxes: Vec<Bbox> = Vec::new();
    for leaf in &leaves {
        let is_large = region_area > 0
            && (leaf.area() as f64) >= LARGE_LEAF_FRAC * (region_area as f64);
        if is_large {
            let cc = connected_components(&ink, w, h, *leaf);
            // On n'accepte le découpage CC que s'il produit >1 boîte (sinon rien gagné).
            if cc.len() > 1 {
                boxes.extend(cc);
                continue;
            }
        }
        boxes.push(*leaf);
    }

    // 5) Nettoyage final.
    let page_area = (w as u64) * (h as u64);
    let min_area = (page_area as f64 * MIN_AREA_FRAC) as u64;

    // Retire les boîtes trop petites et clamp.
    let mut cleaned: Vec<Bbox> = boxes
        .into_iter()
        .map(|b| clamp_bbox(b, w, h))
        .filter(|b| b.width() > 0 && b.height() > 0 && b.area() >= min_area)
        .collect();

    // Fusionne les boîtes qui se chevauchent fortement (issues du secours CC notamment).
    cleaned = merge_overlapping(cleaned);

    // Repli : 0 boîte, ou 1 seule boîte ~= page entière.
    if cleaned.is_empty() {
        return vec![full];
    }
    if cleaned.len() == 1 {
        let b = cleaned[0];
        // ~= page entière si elle couvre >= 90% de l'aire page.
        if (b.area() as f64) >= 0.90 * (page_area as f64) {
            return vec![full];
        }
    }
    cleaned
}

/// Seuil d'Otsu sur l'histogramme du luma. None si image vide ou variance nulle.
fn otsu_threshold(gray: &GrayImage) -> Option<u8> {
    let mut hist = [0u64; 256];
    let mut total: u64 = 0;
    for &p in gray.as_raw().iter() {
        hist[p as usize] += 1;
        total += 1;
    }
    if total == 0 {
        return None;
    }
    // Somme pondérée totale.
    let mut sum_total: u64 = 0;
    for (i, &c) in hist.iter().enumerate() {
        sum_total += (i as u64) * c;
    }
    let mut w_back: u64 = 0; // poids classe "fond" (intensités basses)
    let mut sum_back: u64 = 0;
    let mut best_var: f64 = -1.0;
    // On suit le PLATEAU de variance maximale (premier..dernier t) et on renvoie son milieu.
    // Indispensable pour un histogramme bimodal "à trou" (encre vs papier) : sinon Otsu garde
    // le premier t (= la valeur d'encre elle-même), un mauvais séparateur.
    let mut t_first: usize = FALLBACK_THRESHOLD as usize;
    let mut t_last: usize = FALLBACK_THRESHOLD as usize;
    for t in 0..256usize {
        w_back += hist[t];
        if w_back == 0 {
            continue;
        }
        let w_fore = total - w_back;
        if w_fore == 0 {
            break;
        }
        sum_back += (t as u64) * hist[t];
        let mean_back = sum_back as f64 / w_back as f64;
        let mean_fore = (sum_total - sum_back) as f64 / w_fore as f64;
        // Variance inter-classes (à maximiser).
        let diff = mean_back - mean_fore;
        let between = (w_back as f64) * (w_fore as f64) * diff * diff;
        if between > best_var {
            best_var = between;
            t_first = t;
            t_last = t;
        } else if between == best_var {
            // Plateau (typiquement le "trou" entre les deux pics) -> on l'étend.
            t_last = t;
        }
    }
    if best_var < 0.0 {
        None
    } else {
        Some(((t_first + t_last) / 2) as u8)
    }
}

/// Indexe le masque encre. true = encre. Bornes vérifiées par l'appelant.
#[inline]
fn is_ink(ink: &[bool], w: u32, x: u32, y: u32) -> bool {
    let idx = (y as usize) * (w as usize) + (x as usize);
    matches!(ink.get(idx), Some(true))
}

/// Compte d'encre sur une ligne y, dans [x0,x1).
fn ink_in_row(ink: &[bool], w: u32, y: u32, x0: u32, x1: u32) -> u32 {
    let mut c = 0u32;
    let mut x = x0;
    while x < x1 {
        if is_ink(ink, w, x, y) {
            c += 1;
        }
        x += 1;
    }
    c
}

/// Compte d'encre sur une colonne x, dans [y0,y1).
fn ink_in_col(ink: &[bool], w: u32, x: u32, y0: u32, y1: u32) -> u32 {
    let mut c = 0u32;
    let mut y = y0;
    while y < y1 {
        if is_ink(ink, w, x, y) {
            c += 1;
        }
        y += 1;
    }
    c
}

/// Rogne les bandes de bord quasi sans encre (haut/bas/gauche/droite).
fn trim_margins(ink: &[bool], w: u32, h: u32) -> Bbox {
    let mut x0 = 0u32;
    let mut y0 = 0u32;
    let mut x1 = w;
    let mut y1 = h;
    // Seuils d'encre absolus dérivés de l'epsilon de marge.
    let row_thr = ((w as f32) * MARGIN_EPS).ceil() as u32;
    let col_thr = ((h as f32) * MARGIN_EPS).ceil() as u32;

    // Haut.
    while y0 < y1 && ink_in_row(ink, w, y0, x0, x1) <= row_thr {
        y0 += 1;
    }
    // Bas.
    while y1 > y0 && ink_in_row(ink, w, y1 - 1, x0, x1) <= row_thr {
        y1 -= 1;
    }
    // Gauche.
    while x0 < x1 && ink_in_col(ink, w, x0, y0, y1) <= col_thr {
        x0 += 1;
    }
    // Droite.
    while x1 > x0 && ink_in_col(ink, w, x1 - 1, y0, y1) <= col_thr {
        x1 -= 1;
    }
    if x1 <= x0 || y1 <= y0 {
        // Tout est "vide" -> on garde la page entière comme région.
        return Bbox::new(0, 0, w, h);
    }
    Bbox::new(x0, y0, x1, y1)
}

/// Profil d'encre par ligne (ratio) pour la région r.
fn row_profile(ink: &[bool], w: u32, r: Bbox) -> Vec<f32> {
    let span = r.width().max(1) as f32;
    let mut v = Vec::with_capacity(r.height() as usize);
    let mut y = r.y0;
    while y < r.y1 {
        let c = ink_in_row(ink, w, y, r.x0, r.x1);
        v.push(c as f32 / span);
        y += 1;
    }
    v
}

/// Profil d'encre par colonne (ratio) pour la région r.
fn col_profile(ink: &[bool], w: u32, r: Bbox) -> Vec<f32> {
    let span = r.height().max(1) as f32;
    let mut v = Vec::with_capacity(r.width() as usize);
    let mut x = r.x0;
    while x < r.x1 {
        let c = ink_in_col(ink, w, x, r.y0, r.y1);
        v.push(c as f32 / span);
        x += 1;
    }
    v
}

/// Trouve la plus large gouttière INTÉRIEURE dans un profil.
/// Renvoie (centre_relatif, largeur) de la meilleure bande contiguë sous eps,
/// en excluant les bandes touchant les bords (gouttières externes).
fn best_gutter(profile: &[f32], min_gutter: u32) -> Option<(usize, u32)> {
    let n = profile.len();
    if n == 0 {
        return None;
    }
    let mut best: Option<(usize, u32)> = None;
    let mut i = 0usize;
    while i < n {
        if profile[i] < GUTTER_EPS {
            // Étend la bande vide.
            let start = i;
            let mut j = i;
            while j < n && profile[j] < GUTTER_EPS {
                j += 1;
            }
            let end = j; // exclusif
            let width = (end - start) as u32;
            // Intérieure : ne touche aucun bord du profil.
            let interior = start > 0 && end < n;
            if interior && width >= min_gutter {
                let center = start + (end - start) / 2;
                match best {
                    Some((_, bw)) if width <= bw => {}
                    _ => best = Some((center, width)),
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    best
}

/// XY-cut récursif. Empile les feuilles dans `out`. Profondeur bornée pour la sûreté.
fn xy_cut(
    ink: &[bool],
    w: u32,
    h: u32,
    r: Bbox,
    min_w: u32,
    min_h: u32,
    depth: u32,
    out: &mut Vec<Bbox>,
) {
    // Garde-fou contre toute récursion pathologique.
    if depth > 64 {
        out.push(r);
        return;
    }
    // Trop petite pour être coupée -> feuille.
    if r.width() <= min_w || r.height() <= min_h {
        out.push(r);
        return;
    }

    let min_gutter_y = (((r.height() as f32) * MIN_GUTTER_FRAC) as u32).max(1);
    let min_gutter_x = (((r.width() as f32) * MIN_GUTTER_FRAC) as u32).max(1);

    let rprof = row_profile(ink, w, r); // gouttières horizontales -> coupe en Y
    let cprof = col_profile(ink, w, r); // gouttières verticales   -> coupe en X
    let gy = best_gutter(&rprof, min_gutter_y); // (offset depuis y0, largeur)
    let gx = best_gutter(&cprof, min_gutter_x); // (offset depuis x0, largeur)

    // Choisit la gouttière la plus large (X vs Y). En cas d'égalité, on prend Y.
    let cut_y = gy.map(|(_, ww)| ww).unwrap_or(0);
    let cut_x = gx.map(|(_, ww)| ww).unwrap_or(0);

    if cut_y == 0 && cut_x == 0 {
        // Pas de gouttière franche -> feuille.
        out.push(r);
        return;
    }

    if cut_y >= cut_x {
        if let Some((off, _)) = gy {
            let split = r.y0 + off as u32;
            // Sécurité : split strictement intérieur.
            if split > r.y0 && split < r.y1 {
                let top = Bbox::new(r.x0, r.y0, r.x1, split);
                let bot = Bbox::new(r.x0, split, r.x1, r.y1);
                xy_cut(ink, w, h, top, min_w, min_h, depth + 1, out);
                xy_cut(ink, w, h, bot, min_w, min_h, depth + 1, out);
                return;
            }
        }
        out.push(r);
    } else if let Some((off, _)) = gx {
        let split = r.x0 + off as u32;
        if split > r.x0 && split < r.x1 {
            let left = Bbox::new(r.x0, r.y0, split, r.y1);
            let right = Bbox::new(split, r.y0, r.x1, r.y1);
            xy_cut(ink, w, h, left, min_w, min_h, depth + 1, out);
            xy_cut(ink, w, h, right, min_w, min_h, depth + 1, out);
        } else {
            out.push(r);
        }
    } else {
        out.push(r);
    }
}

/// Composantes connexes 8-connexité sur une grille de blocs CC_BLOCK x CC_BLOCK
/// (moyennage par "présence d'encre" dans le bloc) pour la vitesse.
/// Renvoie des bounding boxes en coordonnées pixel, filtrées par aire min.
fn connected_components(ink: &[bool], w: u32, _h: u32, r: Bbox) -> Vec<Bbox> {
    let rw = r.width();
    let rh = r.height();
    if rw == 0 || rh == 0 {
        return Vec::new();
    }
    // Dimensions de la grille de blocs (arrondi sup).
    let bw = ((rw + CC_BLOCK - 1) / CC_BLOCK) as usize;
    let bh = ((rh + CC_BLOCK - 1) / CC_BLOCK) as usize;
    if bw == 0 || bh == 0 {
        return Vec::new();
    }

    // Marque un bloc "encre" s'il contient au moins un pixel d'encre (légère dilatation
    // implicite : on regroupe les pixels proches via la maille de blocs).
    let mut block_ink = vec![false; bw * bh];
    for by in 0..bh {
        for bx in 0..bw {
            let px0 = r.x0 + (bx as u32) * CC_BLOCK;
            let py0 = r.y0 + (by as u32) * CC_BLOCK;
            let px1 = (px0 + CC_BLOCK).min(r.x1);
            let py1 = (py0 + CC_BLOCK).min(r.y1);
            let mut found = false;
            let mut y = py0;
            'outer: while y < py1 {
                let mut x = px0;
                while x < px1 {
                    if is_ink(ink, w, x, y) {
                        found = true;
                        break 'outer;
                    }
                    x += 1;
                }
                y += 1;
            }
            block_ink[by * bw + bx] = found;
        }
    }

    // Étiquetage 8-connexité par BFS sur les blocs encre.
    let mut visited = vec![false; bw * bh];
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut comps: Vec<Bbox> = Vec::new();
    // Aire min d'une composante, en blocs (dérivée de MIN_AREA_FRAC sur la région).
    let region_blocks = (bw * bh) as f64;
    let min_comp_blocks = (region_blocks * MIN_AREA_FRAC).max(1.0) as u64;

    for sy in 0..bh {
        for sx in 0..bw {
            if visited[sy * bw + sx] || !block_ink[sy * bw + sx] {
                continue;
            }
            // BFS/DFS sur cette composante.
            stack.clear();
            stack.push((sx, sy));
            visited[sy * bw + sx] = true;
            let mut min_bx = sx;
            let mut max_bx = sx;
            let mut min_by = sy;
            let mut max_by = sy;
            let mut count: u64 = 0;
            while let Some((cx, cy)) = stack.pop() {
                count += 1;
                if cx < min_bx { min_bx = cx; }
                if cx > max_bx { max_bx = cx; }
                if cy < min_by { min_by = cy; }
                if cy > max_by { max_by = cy; }
                // 8 voisins.
                let cxi = cx as isize;
                let cyi = cy as isize;
                for dy in -1isize..=1 {
                    for dx in -1isize..=1 {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let nx = cxi + dx;
                        let ny = cyi + dy;
                        if nx < 0 || ny < 0 || nx >= bw as isize || ny >= bh as isize {
                            continue;
                        }
                        let ni = (ny as usize) * bw + (nx as usize);
                        if !visited[ni] && block_ink[ni] {
                            visited[ni] = true;
                            stack.push((nx as usize, ny as usize));
                        }
                    }
                }
            }
            if count < min_comp_blocks {
                continue; // bruit
            }
            // Reconvertit la boîte de blocs en pixels (clamp à la région).
            let px0 = r.x0 + (min_bx as u32) * CC_BLOCK;
            let py0 = r.y0 + (min_by as u32) * CC_BLOCK;
            let px1 = (r.x0 + ((max_bx as u32) + 1) * CC_BLOCK).min(r.x1);
            let py1 = (r.y0 + ((max_by as u32) + 1) * CC_BLOCK).min(r.y1);
            comps.push(Bbox::new(px0, py0, px1.min(r.x1), py1.min(r.y1)));
        }
    }

    // Fusionne les composantes qui se chevauchent.
    merge_overlapping(comps)
}

/// Fusionne itérativement les boîtes qui se chevauchent (union de rectangles).
fn merge_overlapping(mut boxes: Vec<Bbox>) -> Vec<Bbox> {
    if boxes.len() <= 1 {
        return boxes;
    }
    let mut changed = true;
    while changed {
        changed = false;
        let mut out: Vec<Bbox> = Vec::with_capacity(boxes.len());
        'next: for b in boxes.drain(..) {
            for o in out.iter_mut() {
                if overlaps(&b, o) {
                    *o = union(o, &b);
                    changed = true;
                    continue 'next;
                }
            }
            out.push(b);
        }
        boxes = out;
    }
    boxes
}

/// Deux boîtes se chevauchent-elles (intersection d'aire non nulle) ?
fn overlaps(a: &Bbox, b: &Bbox) -> bool {
    let ix0 = a.x0.max(b.x0);
    let iy0 = a.y0.max(b.y0);
    let ix1 = a.x1.min(b.x1);
    let iy1 = a.y1.min(b.y1);
    ix1 > ix0 && iy1 > iy0
}

/// Union englobante de deux boîtes.
fn union(a: &Bbox, b: &Bbox) -> Bbox {
    Bbox::new(
        a.x0.min(b.x0),
        a.y0.min(b.y0),
        a.x1.max(b.x1),
        a.y1.max(b.y1),
    )
}

/// Clamp une boîte à [0,w]x[0,h].
fn clamp_bbox(b: Bbox, w: u32, h: u32) -> Bbox {
    Bbox::new(
        b.x0.min(w),
        b.y0.min(h),
        b.x1.min(w),
        b.y1.min(h),
    )
}
