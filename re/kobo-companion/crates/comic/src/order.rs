// Ordre de lecture — regroupement en bandes horizontales puis tri intra-bande.
use crate::{Bbox, Direction};

// Trie les boîtes dans l'ordre de lecture.
// Heuristique :
//  - grouper en bandes horizontales : deux boîtes sont dans la même bande si leur recouvrement
//    vertical (Bbox::y_overlap) dépasse ~50% de la plus petite hauteur ;
//  - trier les bandes de haut en bas (par y0 min de la bande) ;
//  - dans chaque bande : Ltr -> trier par x0 croissant ; Rtl (manga) -> par x1 décroissant ;
//  - aplatir dans cet ordre.
// Robuste, déterministe. Renvoie toutes les boîtes (mêmes éléments, réordonnés).
pub fn reading_order(boxes: Vec<Bbox>, dir: Direction) -> Vec<Bbox> {
    // Cas triviaux : rien à réordonner.
    if boxes.len() <= 1 {
        return boxes;
    }

    // Tri primaire par y0 croissant (puis x0 pour déterminisme) afin d'agréger les bandes
    // en balayant de haut en bas.
    let mut sorted = boxes;
    sorted.sort_by(|a, b| a.y0.cmp(&b.y0).then(a.x0.cmp(&b.x0)));

    // Agrégation en bandes. Chaque bande est ancrée sur sa boîte de référence : la première
    // qu'elle reçoit (la plus haute, car `sorted` est trié par y0). On compare la boîte
    // candidate à cette ANCRE (pairwise, via Bbox::y_overlap) et non à l'union des boîtes :
    // cela respecte la spec ("y_overlap >= 50% de la plus petite hauteur" entre deux boîtes)
    // et évite la dérive en escalier (chaînage transitif d'union qui fusionnerait à tort
    // des boîtes ne se recouvrant pas).
    struct Band {
        anchor: Bbox, // boîte de référence (la plus haute de la bande)
        items: Vec<Bbox>,
    }
    let mut bands: Vec<Band> = Vec::new();

    for b in sorted {
        let bh = b.height();
        let mut placed = false;
        for band in bands.iter_mut() {
            // Recouvrement vertical pairwise entre la candidate et l'ancre de la bande.
            let overlap = band.anchor.y_overlap(&b);
            // Plus petite hauteur entre la candidate et l'ancre.
            let min_h = bh.min(band.anchor.height());
            // min_h == 0 (boîte dégénérée, hauteur nulle) -> on exige un recouvrement strict
            // pour éviter de tout fusionner ; sinon seuil à 50% de la plus petite hauteur.
            let ok = if min_h == 0 {
                overlap > 0
            } else {
                // overlap >= 0.5 * min_h, en arithmétique entière sans flottant.
                (overlap as u64) * 2 >= min_h as u64
            };
            if ok {
                band.items.push(b);
                placed = true;
                break;
            }
        }
        if !placed {
            bands.push(Band { anchor: b, items: vec![b] });
        }
    }

    // Tri des bandes de haut en bas, par y0 de l'ancre (puis y1 pour stabilité déterministe).
    bands.sort_by(|a, b| {
        a.anchor.y0.cmp(&b.anchor.y0).then(a.anchor.y1.cmp(&b.anchor.y1))
    });

    // Tri intra-bande selon la direction, puis aplatissement.
    let mut out: Vec<Bbox> = Vec::new();
    for mut band in bands {
        match dir {
            // Gauche -> droite : x0 croissant (x1 puis y0 en départage déterministe).
            Direction::Ltr => band.items.sort_by(|a, b| {
                a.x0.cmp(&b.x0).then(a.x1.cmp(&b.x1)).then(a.y0.cmp(&b.y0))
            }),
            // Droite -> gauche (manga) : x1 décroissant (x0 décroissant puis y0 en départage).
            Direction::Rtl => band.items.sort_by(|a, b| {
                b.x1.cmp(&a.x1).then(b.x0.cmp(&a.x0)).then(a.y0.cmp(&b.y0))
            }),
        }
        out.extend(band.items);
    }

    out
}
