// comic — détection de cases + rendu "panel-par-page" (guided view), préprocesseur desktop.
//
// Pipeline : CBZ -> détection des cases (projection/XY-cut + composantes connexes) ->
// ordre de lecture (ltr/rtl) -> sidecar JSON (contrat/cache) -> CBZ panelisé (1 case = 1 page)
// que Nickel lit comme une BD normale. Aucun calcul sur la liseuse.
//
// Ce fichier fige le CONTRAT (types + signatures). Les modules en remplissent le corps.
use serde::{Deserialize, Serialize};

pub mod cbz;
pub mod detect;
pub mod order;
pub mod sidecar;
pub mod render;
pub mod pipeline;

/// Boîte englobante en pixels de l'image source. x1/y1 EXCLUSIFS. Sérialisée en [x0,y0,x1,y1].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "[u32; 4]", into = "[u32; 4]")]
pub struct Bbox {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl Bbox {
    pub fn new(x0: u32, y0: u32, x1: u32, y1: u32) -> Self {
        Bbox { x0: x0.min(x1), y0: y0.min(y1), x1: x0.max(x1), y1: y0.max(y1) }
    }
    pub fn width(&self) -> u32 { self.x1.saturating_sub(self.x0) }
    pub fn height(&self) -> u32 { self.y1.saturating_sub(self.y0) }
    pub fn area(&self) -> u64 { self.width() as u64 * self.height() as u64 }
    pub fn cx(&self) -> u32 { (self.x0 + self.x1) / 2 }
    pub fn cy(&self) -> u32 { (self.y0 + self.y1) / 2 }
    /// Recouvrement vertical (en pixels) entre deux boîtes — utile pour grouper en bandes.
    pub fn y_overlap(&self, o: &Bbox) -> u32 {
        let lo = self.y0.max(o.y0);
        let hi = self.y1.min(o.y1);
        hi.saturating_sub(lo)
    }
}
impl From<[u32; 4]> for Bbox {
    fn from(a: [u32; 4]) -> Self { Bbox { x0: a[0], y0: a[1], x1: a[2], y1: a[3] } }
}
impl From<Bbox> for [u32; 4] {
    fn from(b: Bbox) -> Self { [b.x0, b.y0, b.x1, b.y1] }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Panel {
    pub id: u32,
    pub order: u32,
    pub bbox: Bbox,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Page {
    pub index: u32,
    pub width: u32,
    pub height: u32,
    pub panels: Vec<Panel>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Ltr, // franco-belge / comics US
    Rtl, // manga
}

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sidecar {
    pub schema_version: u32,
    pub source_hash: String,        // "sha256:<hex>" du CBZ source (invalidation de cache)
    pub reading_direction: Direction,
    pub pages: Vec<Page>,
}
