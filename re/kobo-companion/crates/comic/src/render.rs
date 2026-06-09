// Rendu "panel-par-page" — implémentation complète.
use crate::Bbox;
use image::{DynamicImage, GenericImageView, RgbImage};

#[derive(Clone, Copy, Debug)]
pub struct RenderOpts {
    pub canvas_w: u32,       // largeur écran cible (Libra Colour ~1264)
    pub canvas_h: u32,       // hauteur écran cible (~1680)
    pub padding: u32,        // marge ajoutée autour de chaque case (bulles débordantes)
    pub upscale: bool,       // autoriser l'agrandissement des petites cases
    pub include_full_page: bool, // préfixer la planche entière (contexte) avant ses cases
    pub jpeg_quality: u8,    // qualité JPEG de sortie (ex. 88)
}

impl Default for RenderOpts {
    fn default() -> Self {
        RenderOpts {
            canvas_w: 1264,
            canvas_h: 1680,
            padding: 12,
            upscale: true,
            include_full_page: false,
            jpeg_quality: 88,
        }
    }
}

// Rend une seule bbox (déjà clampée) en page-image plein écran centrée sur fond blanc.
fn render_bbox(page: &DynamicImage, b: &Bbox, opts: &RenderOpts) -> RgbImage {
    // Dimensions du canvas garanties >= 1 pour éviter toute panique.
    let cw = opts.canvas_w.max(1);
    let ch = opts.canvas_h.max(1);

    // Largeur/hauteur de la case (au moins 1px).
    let pw = b.width().max(1);
    let ph = b.height().max(1);

    // Recadrage de la sous-image (indices déjà bornés par le clamp en amont).
    let sub = page.crop_imm(b.x0, b.y0, pw, ph);

    // Calcul du facteur d'échelle pour TENIR dans (cw, ch) en gardant le ratio.
    let scale_w = cw as f64 / pw as f64;
    let scale_h = ch as f64 / ph as f64;
    let mut scale = scale_w.min(scale_h);
    // Pas d'agrandissement si non autorisé.
    if scale > 1.0 && !opts.upscale {
        scale = 1.0;
    }

    // Dimensions cibles (>= 1).
    let tw = ((pw as f64 * scale).round() as u32).max(1).min(cw);
    let th = ((ph as f64 * scale).round() as u32).max(1).min(ch);

    // Redimensionnement Lanczos3 -> RgbImage.
    let resized = image::imageops::resize(
        &sub.to_rgb8(),
        tw,
        th,
        image::imageops::FilterType::Lanczos3,
    );

    // Canvas blanc puis collage centré (letterbox).
    let mut canvas = RgbImage::from_pixel(cw, ch, image::Rgb([255, 255, 255]));
    let ox = ((cw.saturating_sub(tw)) / 2) as i64;
    let oy = ((ch.saturating_sub(th)) / 2) as i64;
    image::imageops::overlay(&mut canvas, &resized, ox, oy);
    canvas
}

// Rend les cases (déjà ordonnées) d'une page en pages-images plein écran (1 case = 1 page).
pub fn panelize_page(page: &DynamicImage, panels: &[Bbox], opts: &RenderOpts) -> Vec<RgbImage> {
    let (w, h) = page.dimensions();
    let mut out: Vec<RgbImage> = Vec::new();

    // Bbox couvrant la page entière.
    let full = Bbox::new(0, 0, w, h);

    // Page entière en premier si demandé (sans padding).
    if opts.include_full_page {
        out.push(render_bbox(page, &full, opts));
    }

    // Si aucune case : rendre la page entière (sauf si déjà ajoutée).
    if panels.is_empty() {
        if !opts.include_full_page {
            out.push(render_bbox(page, &full, opts));
        }
        return out;
    }

    // Une page-image par case.
    for p in panels {
        // Étendre du padding puis clamp à [0,w]x[0,h].
        let x0 = p.x0.saturating_sub(opts.padding).min(w);
        let y0 = p.y0.saturating_sub(opts.padding).min(h);
        let x1 = p.x1.saturating_add(opts.padding).min(w);
        let y1 = p.y1.saturating_add(opts.padding).min(h);
        let b = Bbox::new(x0, y0, x1, y1);
        out.push(render_bbox(page, &b, opts));
    }

    out
}

// Encode une image RGB en JPEG (image::codecs::jpeg::JpegEncoder, qualité donnée).
pub fn encode_jpeg(img: &RgbImage, quality: u8) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality)
        .encode_image(img)
        .map_err(|e| format!("encodage JPEG échoué: {e}"))?;
    Ok(buf)
}
