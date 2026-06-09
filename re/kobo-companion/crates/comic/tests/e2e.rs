// Test bout-en-bout : planche synthétique (grille 2x2 de cases) -> détection -> ordre ->
// sidecar + CBZ panelisé. Aucune dépendance externe (images générées via le crate `image`).
use comic::pipeline::{process_cbz, Opts};
use comic::render::RenderOpts;
use comic::{sidecar, cbz, Direction};
use image::{Rgb, RgbImage};
use std::io::Write;
use std::path::PathBuf;

const W: u32 = 1200;
const H: u32 = 1600;
const M: u32 = 40; // marge
const G: u32 = 40; // gouttière

// Quadrants attendus (TL, TR, BL, BR) en coordonnées [x0,y0,x1,y1).
fn quadrants() -> [(u32, u32, u32, u32); 4] {
    let pw = (W - 2 * M - G) / 2;
    let ph = (H - 2 * M - G) / 2;
    let xl = M;
    let xr = M + pw + G;
    let yt = M;
    let yb = M + ph + G;
    [
        (xl, yt, xl + pw, yt + ph), // TL
        (xr, yt, xr + pw, yt + ph), // TR
        (xl, yb, xl + pw, yb + ph), // BL
        (xr, yb, xr + pw, yb + ph), // BR
    ]
}

// Planche blanche avec 4 cases gris foncé (encre) séparées par des gouttières blanches.
fn synth_page() -> Vec<u8> {
    let mut img = RgbImage::from_pixel(W, H, Rgb([255, 255, 255]));
    for (x0, y0, x1, y1) in quadrants() {
        for y in y0..y1 {
            for x in x0..x1 {
                img.put_pixel(x, y, Rgb([40, 40, 40]));
            }
        }
    }
    let mut buf = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png).unwrap();
    buf
}

fn make_cbz(path: &PathBuf, n_pages: usize) {
    let f = std::fs::File::create(path).unwrap();
    let mut w = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let png = synth_page();
    for i in 1..=n_pages {
        w.start_file(format!("page{}.png", i), opts).unwrap();
        w.write_all(&png).unwrap();
    }
    w.finish().unwrap();
}

#[test]
fn panelize_grid_2x2() {
    let dir = std::env::temp_dir().join(format!("comic_e2e_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let input = dir.join("in.cbz");
    let output = dir.join("out.cbz");
    let side = dir.join("in.panels.json");
    make_cbz(&input, 2);

    let opts = Opts { direction: Direction::Ltr, render: RenderOpts::default(), force: false };
    let (sc, n) = process_cbz(&input, &output, &side, &opts).expect("process_cbz");

    // 2 pages, 4 cases chacune.
    assert_eq!(sc.pages.len(), 2, "2 pages attendues");
    for page in &sc.pages {
        assert_eq!(page.panels.len(), 4, "4 cases attendues, eu {}", page.panels.len());
    }
    // Sortie = 4 cases * 2 pages = 8 pages-images.
    assert_eq!(n, 8, "8 pages de sortie attendues, eu {}", n);

    // Ordre de lecture LTR : TL, TR, BL, BR.
    let mut panels = sc.pages[0].panels.clone();
    panels.sort_by_key(|p| p.order);
    let q = |b: &comic::Bbox| (b.cx() < W / 2, b.cy() < H / 2); // (gauche?, haut?)
    assert_eq!(q(&panels[0].bbox), (true, true), "case 0 = haut-gauche");
    assert_eq!(q(&panels[1].bbox), (false, true), "case 1 = haut-droite");
    assert_eq!(q(&panels[2].bbox), (true, false), "case 2 = bas-gauche");
    assert_eq!(q(&panels[3].bbox), (false, false), "case 3 = bas-droite");

    // Le CBZ de sortie contient bien 8 images.
    let out_pages = cbz::read_pages(&output).expect("read out");
    assert_eq!(out_pages.len(), 8, "8 images dans le CBZ panelisé");

    // Sidecar : hash présent + relecture OK.
    assert!(sc.source_hash.starts_with("sha256:"));
    let reread = sidecar::read_sidecar(&side).expect("relecture sidecar");
    assert_eq!(reread.pages.len(), 2);

    // Cache : 2e passe sans --force -> réutilise le sidecar, même hash, mêmes cases.
    let (sc2, n2) = process_cbz(&input, &output, &side, &opts).expect("2e passe");
    assert_eq!(sc2.source_hash, sc.source_hash);
    assert_eq!(n2, 8);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rtl_reverses_order() {
    let dir = std::env::temp_dir().join(format!("comic_rtl_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let input = dir.join("in.cbz");
    let output = dir.join("out.cbz");
    let side = dir.join("in.panels.json");
    make_cbz(&input, 1);

    let opts = Opts { direction: Direction::Rtl, render: RenderOpts::default(), force: true };
    let (sc, _) = process_cbz(&input, &output, &side, &opts).expect("process_cbz rtl");
    let mut panels = sc.pages[0].panels.clone();
    panels.sort_by_key(|p| p.order);
    // RTL : haut-droite d'abord.
    let q = |b: &comic::Bbox| (b.cx() < W / 2, b.cy() < H / 2);
    assert_eq!(q(&panels[0].bbox), (false, true), "RTL: case 0 = haut-droite");
    assert_eq!(q(&panels[1].bbox), (true, true), "RTL: case 1 = haut-gauche");

    let _ = std::fs::remove_dir_all(&dir);
}
