// Orchestration : CBZ -> (sidecar, CBZ panelisé).
use crate::render::RenderOpts;
use crate::{Direction, Page, Panel, Sidecar, SCHEMA_VERSION};
use std::path::Path;

#[derive(Clone, Debug)]
pub struct Opts {
    pub direction: Direction,
    pub render: RenderOpts,
    pub force: bool, // recalculer la détection même si le cache (sidecar) est valide
}

// Traite un CBZ et écrit le CBZ panelisé. Renvoie (sidecar, nombre de pages de sortie).
pub fn process_cbz(
    input: &Path,
    output: &Path,
    sidecar_path: &Path,
    opts: &Opts,
) -> Result<(Sidecar, usize), String> {
    // 1) Hash du CBZ source (invalidation de cache).
    let hash = crate::sidecar::source_hash(input)?;

    // 2) Lecture des pages images (toujours nécessaire pour le rendu).
    let pages = crate::cbz::read_pages(input)?;
    if pages.is_empty() {
        return Err("aucune image lisible dans le CBZ".to_string());
    }

    // 3) Sidecar : réutiliser le cache si valide, sinon recalculer la détection.
    let reuse = match crate::sidecar::read_sidecar(sidecar_path) {
        Ok(prev)
            if prev.schema_version == SCHEMA_VERSION
                && prev.source_hash == hash
                && !opts.force =>
        {
            Some(prev)
        }
        _ => None,
    };

    let sc = match reuse {
        Some(prev) => prev,
        None => {
            // Détection des cases page par page.
            let mut out_pages: Vec<Page> = Vec::with_capacity(pages.len());
            for (i, page) in pages.iter().enumerate() {
                let g = page.image.to_luma8();
                let mut boxes = crate::detect::detect_panels(&g);
                boxes = crate::order::reading_order(boxes, opts.direction);
                let panels: Vec<Panel> = boxes
                    .iter()
                    .enumerate()
                    .map(|(k, b)| Panel {
                        id: k as u32,
                        order: k as u32,
                        bbox: *b,
                    })
                    .collect();
                out_pages.push(Page {
                    index: i as u32,
                    width: page.image.width(),
                    height: page.image.height(),
                    panels,
                });
            }
            let sc = Sidecar {
                schema_version: SCHEMA_VERSION,
                source_hash: hash,
                reading_direction: opts.direction,
                pages: out_pages,
            };
            // Échec d'écriture du sidecar = avertissement, pas fatal.
            if let Err(e) = crate::sidecar::write_sidecar(sidecar_path, &sc) {
                eprintln!("avertissement : écriture du sidecar impossible : {e}");
            }
            sc
        }
    };

    // 4) Rendu : pour chaque image, retrouver la Page correspondante du sidecar.
    let mut out_pages: Vec<(String, Vec<u8>)> = Vec::new();
    for (i, page) in pages.iter().enumerate() {
        // Retrouver la Page i du sidecar (par index logique).
        let meta = sc.pages.iter().find(|p| p.index == i as u32);

        // Bboxes ordonnées par .order ; repli page entière si métadonnée absente/vide.
        let bboxes: Vec<crate::Bbox> = match meta {
            Some(p) if !p.panels.is_empty() => {
                let mut sorted = p.panels.clone();
                sorted.sort_by_key(|pl| pl.order);
                sorted.into_iter().map(|pl| pl.bbox).collect()
            }
            _ => vec![crate::Bbox::new(0, 0, page.image.width(), page.image.height())],
        };

        let imgs = crate::render::panelize_page(&page.image, &bboxes, &opts.render);
        for (k, img) in imgs.iter().enumerate() {
            let bytes = crate::render::encode_jpeg(img, opts.render.jpeg_quality)?;
            let name = format!("p{:04}_{:03}.jpg", i, k);
            out_pages.push((name, bytes));
        }
    }

    // 5) Écriture du CBZ panelisé.
    crate::cbz::write_cbz(output, &out_pages)?;

    // 6) Renvoyer (sidecar, total pages de sortie).
    Ok((sc, out_pages.len()))
}
