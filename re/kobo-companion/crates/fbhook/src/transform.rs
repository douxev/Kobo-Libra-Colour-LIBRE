// Transformations appliquées à chaque mise à jour d'écran (Kobo Kaleido 3 / EPDC).
//
// Appelé par lib.rs::ioctl pour chaque MXCFB_SEND_UPDATE, AVANT de relayer l'ioctl réel.
// `upd` est mutable (waveform_mode / flags modifiables). `fd` est le descripteur du
// framebuffer (utilisable pour FBIOGET_VSCREENINFO/FBIOGET_FSCREENINFO + mmap).
//
// Tout est best-effort : cette fonction tourne dans nickel. En cas de doute on ne
// transforme rien (catch_unwind en garde-fou ultime) et on laisse l'ioctl réel passer.
use crate::{
    MxcfbUpdateData, EPDC_FLAG_ENABLE_INVERSION, EPDC_FLAG_USE_DITHERING_Y1,
    EPDC_FLAG_USE_DITHERING_Y4, WAVEFORM_MODE_AUTO, WAVEFORM_MODE_DU, WAVEFORM_MODE_GC16,
};
use libc::{c_int, c_ulong, c_void};
use std::sync::Once;

// ───────────────────────────── Configuration ─────────────────────────────

const DEFAULT_CONF: &str = "/mnt/onboard/.adds/kobo-companion/fbhook.conf";

#[derive(Clone, Copy)]
struct Conf {
    color: bool,
    waveform: bool,
    waveform_force: bool, // remplacer le waveform_mode même si l'appelant en a fixé un
    dither: bool,
    dither_y4: bool, // Y4 (4 bits) si vrai, sinon Y1 (1 bit)
    night: bool,
    saturation: f32,
    gamma: f32,
    night_gamma: f32, // documenté ; usage réel nécessiterait de transformer le fb
}

impl Default for Conf {
    fn default() -> Self {
        Conf {
            color: false,
            waveform: false,
            waveform_force: false,
            dither: false,
            dither_y4: true,
            night: false,
            saturation: 1.0,
            gamma: 1.0,
            night_gamma: 1.0,
        }
    }
}

// Lecture paresseuse de la config (une seule fois). On stocke dans un statique.
static CONF_INIT: Once = Once::new();
static mut CONF: Conf = Conf {
    color: false,
    waveform: false,
    waveform_force: false,
    dither: false,
    dither_y4: true,
    night: false,
    saturation: 1.0,
    gamma: 1.0,
    night_gamma: 1.0,
};

fn parse_bool(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "yes" | "y" | "true" | "1" | "on"
    )
}

fn parse_conf(text: &str) -> Conf {
    let mut c = Conf::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let (key, val) = match line.split_once('=') {
            Some((k, v)) => (k.trim(), v.trim()),
            None => continue,
        };
        match key.to_ascii_lowercase().as_str() {
            "color" => c.color = parse_bool(val),
            "waveform" => c.waveform = parse_bool(val),
            "waveform_force" | "force" => c.waveform_force = parse_bool(val),
            "dither" => c.dither = parse_bool(val),
            "dither_y4" => c.dither_y4 = parse_bool(val),
            "night" => c.night = parse_bool(val),
            "saturation" => {
                if let Ok(f) = val.parse::<f32>() {
                    c.saturation = f;
                }
            }
            "gamma" => {
                if let Ok(f) = val.parse::<f32>() {
                    c.gamma = f;
                }
            }
            "night_gamma" => {
                if let Ok(f) = val.parse::<f32>() {
                    c.night_gamma = f;
                }
            }
            _ => {}
        }
    }
    c
}

fn config() -> Conf {
    CONF_INIT.call_once(|| {
        let path = std::env::var("KOBO_FBHOOK_CONF").unwrap_or_else(|_| DEFAULT_CONF.to_string());
        // Lecture tolérante : fichier absent / illisible -> valeurs par défaut (tout désactivé).
        let conf = match std::fs::read_to_string(&path) {
            Ok(text) => parse_conf(&text),
            Err(_) => Conf::default(),
        };
        // SAFETY : écriture unique protégée par Once, avant toute lecture concurrente.
        unsafe {
            CONF = conf;
        }
    });
    // SAFETY : initialisé par le call_once ci-dessus, ensuite immuable.
    unsafe { CONF }
}

// ───────────────────────── Point d'entrée du hook ─────────────────────────

pub fn on_send_update(fd: c_int, upd: &mut MxcfbUpdateData) {
    // Garde-fou ultime : aucune panique ne doit remonter dans nickel.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let conf = config();

        // 1. Waveform auto selon la taille de la région.
        if conf.waveform {
            apply_waveform(conf, upd);
        }

        // 2. Dithering matériel via flags.
        if conf.dither {
            apply_dither(conf, upd);
        }

        // 3. Mode nuit : inversion matérielle (la vraie correction gamma nuit
        //    nécessiterait de transformer le contenu du framebuffer, cf. note plus bas).
        if conf.night {
            apply_night(conf, upd);
        }

        // 4. Correction couleur : transforme réellement les pixels du fb (mmap).
        if conf.color {
            apply_color(fd, conf, upd);
        }
    }));
}

// ───────────────────────────── 1. Waveform ───────────────────────────────

// Heuristique sur la TAILLE de la région : une grande surface (image/BD/photo) profite
// d'un waveform 16 niveaux (GC16) ; une région petite/étroite est probablement du texte
// et passe en DU (rapide, 2 niveaux). Seuils en pixels (panneau ~1264x1680).
fn apply_waveform(conf: Conf, upd: &mut MxcfbUpdateData) {
    // Ne touche que si l'appelant a demandé AUTO, sauf si "force".
    if upd.waveform_mode != WAVEFORM_MODE_AUTO && !conf.waveform_force {
        return;
    }
    let w = upd.update_region.width;
    let h = upd.update_region.height;
    let area = (w as u64).saturating_mul(h as u64);

    // Grande surface OU bloc nettement bidimensionnel -> contenu riche -> GC16.
    // Petite/étroite (bande de texte, curseur, barre) -> DU.
    const AREA_THRESHOLD: u64 = 200_000; // ~ 450x450 px
    const MIN_DIM: u32 = 200; // en deçà sur une dimension : probablement du texte/UI

    let large = area >= AREA_THRESHOLD && w >= MIN_DIM && h >= MIN_DIM;
    upd.waveform_mode = if large {
        WAVEFORM_MODE_GC16
    } else {
        WAVEFORM_MODE_DU
    };
}

// ───────────────────────────── 2. Dithering ──────────────────────────────

fn apply_dither(conf: Conf, upd: &mut MxcfbUpdateData) {
    upd.flags |= if conf.dither_y4 {
        EPDC_FLAG_USE_DITHERING_Y4
    } else {
        EPDC_FLAG_USE_DITHERING_Y1
    };
}

// ─────────────────────────────── 3. Nuit ─────────────────────────────────

// Inversion matérielle via flag EPDC. NOTE : c'est une simple inversion par l'EPDC ;
// une vraie correction gamma nuit (préservant les couleurs Kaleido et adoucissant les
// blancs) imposerait de réécrire le contenu du framebuffer pixel par pixel (comme la
// correction couleur ci-dessous), ce que ce flag ne fait pas. `night_gamma` est donc
// conservé en config pour cette évolution future.
fn apply_night(_conf: Conf, upd: &mut MxcfbUpdateData) {
    upd.flags |= EPDC_FLAG_ENABLE_INVERSION;
}

// ───────────────────────── 4. Correction couleur ─────────────────────────
//
// Structures fbdev standard (linux/fb.h). Layout #[repr(C)] obligatoire.

#[repr(C)]
#[derive(Clone, Copy)]
struct FbBitfield {
    offset: u32,
    length: u32,
    msb_right: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FbVarScreeninfo {
    xres: u32,
    yres: u32,
    xres_virtual: u32,
    yres_virtual: u32,
    xoffset: u32,
    yoffset: u32,
    bits_per_pixel: u32,
    grayscale: u32,
    red: FbBitfield,
    green: FbBitfield,
    blue: FbBitfield,
    transp: FbBitfield,
    nonstd: u32,
    activate: u32,
    height: u32,
    width: u32,
    accel_flags: u32,
    pixclock: u32,
    left_margin: u32,
    right_margin: u32,
    upper_margin: u32,
    lower_margin: u32,
    hsync_len: u32,
    vsync_len: u32,
    sync: u32,
    vmode: u32,
    rotate: u32,
    colorspace: u32,
    reserved: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FbFixScreeninfo {
    id: [u8; 16],
    smem_start: c_ulong,
    smem_len: u32,
    fb_type: u32,
    type_aux: u32,
    visual: u32,
    xpanstep: u16,
    ypanstep: u16,
    ywrapstep: u16,
    _pad: u16, // alignement du line_length sur 4 octets (3 x u16 + padding)
    line_length: u32,
    mmio_start: c_ulong,
    mmio_len: u32,
    accel: u32,
    capabilities: u16,
    reserved: [u16; 2],
}

// Numéros d'ioctl fbdev (linux/fb.h).
const FBIOGET_VSCREENINFO: c_ulong = 0x4600;
const FBIOGET_FSCREENINFO: c_ulong = 0x4602;

// Infos framebuffer mappé, calculées une seule fois.
struct FbMap {
    base: *mut u8,
    len: usize,
    line_length: usize,
    bytes_per_pixel: usize,
    xres: u32,
    yres: u32,
    // Décalages d'octets des composantes dans un pixel (selon bitfields var).
    r_byte: usize,
    g_byte: usize,
    b_byte: usize,
}

static FB_INIT: Once = Once::new();
static mut FB: Option<FbMap> = None;
// LUT 256 entrées par composante (correction saturation/gamma en espace OKLab).
static mut LUT: [[u8; 256]; 3] = [[0; 256], [0; 256], [0; 256]];

// SAFETY pour le partage entre threads : FbMap n'est écrit qu'une fois sous Once.
unsafe fn fb_map(fd: c_int, conf: Conf) -> Option<&'static FbMap> {
    FB_INIT.call_once(|| {
        let mut var: FbVarScreeninfo = core::mem::zeroed();
        let mut fix: FbFixScreeninfo = core::mem::zeroed();

        // On appelle directement libc::ioctl (et non le wrapper interposé) ; le fd reçu
        // est bien celui du framebuffer.
        if libc::ioctl(fd, FBIOGET_VSCREENINFO, &mut var as *mut _ as *mut c_void) != 0 {
            return;
        }
        if libc::ioctl(fd, FBIOGET_FSCREENINFO, &mut fix as *mut _ as *mut c_void) != 0 {
            return;
        }

        let bpp = var.bits_per_pixel;
        // On ne gère que 16/24/32 bpp ; sinon on s'abstient (ne rien faire).
        let bytes_per_pixel = match bpp {
            16 => 2,
            24 => 3,
            32 => 4,
            _ => return,
        };

        let len = fix.smem_len as usize;
        let line_length = fix.line_length as usize;
        if len == 0 || line_length == 0 || var.xres == 0 || var.yres == 0 {
            return;
        }
        // Cohérence minimale : la ligne doit contenir les pixels visibles.
        if line_length < var.xres as usize * bytes_per_pixel {
            return;
        }

        let ptr = libc::mmap(
            core::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd,
            0,
        );
        if ptr == libc::MAP_FAILED {
            return;
        }

        // Décalages d'octets des composantes pour 32/24 bpp (à partir des bitfields).
        // Pour 16 bpp (RGB565) on traite à part dans la boucle pixel.
        let (r_byte, g_byte, b_byte) = if bytes_per_pixel >= 3 {
            (
                (var.red.offset / 8) as usize,
                (var.green.offset / 8) as usize,
                (var.blue.offset / 8) as usize,
            )
        } else {
            (0, 0, 0)
        };
        // Garde-fou : des bitfields incohérents (offset >= bpp) feraient écrire hors du
        // pixel (et potentiellement hors mmap au dernier pixel). Dans ce cas on s'abstient
        // (et on libère le mmap puisqu'il ne sera pas conservé dans FB).
        if bytes_per_pixel >= 3
            && (r_byte >= bytes_per_pixel || g_byte >= bytes_per_pixel || b_byte >= bytes_per_pixel)
        {
            libc::munmap(ptr, len);
            return;
        }

        build_lut(conf);

        FB = Some(FbMap {
            base: ptr as *mut u8,
            len,
            line_length,
            bytes_per_pixel,
            xres: var.xres,
            yres: var.yres,
            r_byte,
            g_byte,
            b_byte,
        });
    });
    // SAFETY : initialisé sous Once ci-dessus.
    FB.as_ref()
}

// Construit la LUT par composante. La saturation est appliquée en espace OKLab
// (chroma = a/b) ; comme la LUT est mono-composante, on approxime la correction
// en passant chaque composante par sRGB->linéaire->gamma->sRGB, et on encode la
// saturation comme un gain de contraste autour du gris. La vraie saturation
// OKLab (qui dépend des 3 canaux simultanément) est appliquée par pixel dans la
// boucle ; la LUT sert ici à la correction gamma, et `oklab_saturate` à la chroma.
unsafe fn build_lut(conf: Conf) {
    let gamma = if conf.gamma > 0.0 { conf.gamma } else { 1.0 };
    for v in 0..256usize {
        let s = v as f32 / 255.0;
        // sRGB -> linéaire -> gamma -> sRGB.
        let lin = srgb_to_linear(s);
        let g = lin.powf(1.0 / gamma);
        let out = linear_to_srgb(g.clamp(0.0, 1.0));
        let b = (out * 255.0 + 0.5) as i32;
        let b = b.clamp(0, 255) as u8;
        LUT[0][v] = b;
        LUT[1][v] = b;
        LUT[2][v] = b;
    }
}

// ───────── Conversions sRGB <-> linéaire <-> OKLab ─────────

#[inline]
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[inline]
fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

// sRGB linéaire -> OKLab (Björn Ottosson).
#[inline]
fn linear_srgb_to_oklab(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b;
    let m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b;
    let s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b;

    let l_ = l.cbrt();
    let m_ = m.cbrt();
    let s_ = s.cbrt();

    (
        0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_,
        1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_,
        0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_,
    )
}

// OKLab -> sRGB linéaire.
#[inline]
fn oklab_to_linear_srgb(ll: f32, a: f32, b: f32) -> (f32, f32, f32) {
    let l_ = ll + 0.3963377774 * a + 0.2158037573 * b;
    let m_ = ll - 0.1055613458 * a - 0.0638541728 * b;
    let s_ = ll - 0.0894841775 * a - 1.2914855480 * b;

    let l = l_ * l_ * l_;
    let m = m_ * m_ * m_;
    let s = s_ * s_ * s_;

    (
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
    )
}

// Applique gamma (LUT) puis saturation OKLab à un pixel (octets 0..=255).
// Retourne les nouvelles composantes 0..=255.
#[inline]
unsafe fn transform_pixel(r: u8, g: u8, b: u8, sat: f32) -> (u8, u8, u8) {
    // 1. Gamma via LUT (sur chaque composante sRGB).
    let r = LUT[0][r as usize];
    let g = LUT[1][g as usize];
    let b = LUT[2][b as usize];

    // 2. Saturation en OKLab : on amplifie la chroma (a,b) autour de l'axe achromatique.
    if (sat - 1.0).abs() < 1.0e-3 {
        return (r, g, b);
    }
    let lr = srgb_to_linear(r as f32 / 255.0);
    let lg = srgb_to_linear(g as f32 / 255.0);
    let lb = srgb_to_linear(b as f32 / 255.0);

    let (ll, a, bb) = linear_srgb_to_oklab(lr, lg, lb);
    let a = a * sat;
    let bb = bb * sat;
    let (nr, ng, nb) = oklab_to_linear_srgb(ll, a, bb);

    let nr = linear_to_srgb(nr.clamp(0.0, 1.0));
    let ng = linear_to_srgb(ng.clamp(0.0, 1.0));
    let nb = linear_to_srgb(nb.clamp(0.0, 1.0));

    (
        (nr * 255.0 + 0.5).clamp(0.0, 255.0) as u8,
        (ng * 255.0 + 0.5).clamp(0.0, 255.0) as u8,
        (nb * 255.0 + 0.5).clamp(0.0, 255.0) as u8,
    )
}

// Applique la correction couleur sur les pixels de update_region dans le fb mmappé.
//
// NOTE Kaleido / CFA : l'écran Kaleido 3 utilise une matrice de filtres couleur (CFA)
// sous-pixel par-dessus le panneau N&B. Le mapping exact pixel-fb -> sous-pixel CFA
// (et l'effet réel des corrections) est inconnu sans RE matériel. On applique donc la
// correction en espace RGB pixel (ce que le fb expose), correct pour le contenu logique ;
// l'adaptation sous-pixel CFA (pondération par filtre R/G/B/W) reste à affiner sur device.
fn apply_color(fd: c_int, conf: Conf, upd: &mut MxcfbUpdateData) {
    // Rien à faire si la correction est neutre.
    if (conf.saturation - 1.0).abs() < 1.0e-3 && (conf.gamma - 1.0).abs() < 1.0e-3 {
        return;
    }

    // SAFETY : fb_map garantit un mmap valide ou None ; toutes les bornes sont vérifiées.
    unsafe {
        let fb = match fb_map(fd, conf) {
            Some(fb) => fb,
            None => return, // bpp inattendu ou mmap échoué -> ne rien faire.
        };

        let sat = conf.saturation;

        // Région à corriger, bornée à l'écran.
        let reg = upd.update_region;
        let x0 = reg.left.min(fb.xres);
        let y0 = reg.top.min(fb.yres);
        let x1 = reg.left.saturating_add(reg.width).min(fb.xres);
        let y1 = reg.top.saturating_add(reg.height).min(fb.yres);
        if x1 <= x0 || y1 <= y0 {
            return;
        }

        let bpp = fb.bytes_per_pixel;
        let stride = fb.line_length;

        for y in y0..y1 {
            let row = y as usize * stride;
            // Bornes de la ligne dans le mapping.
            for x in x0..x1 {
                let off = row + x as usize * bpp;
                // Vérif de bornes stricte (sécurité contre tout débordement).
                if off + bpp > fb.len {
                    break;
                }
                let p = fb.base.add(off);

                match bpp {
                    4 | 3 => {
                        let r = *p.add(fb.r_byte);
                        let g = *p.add(fb.g_byte);
                        let b = *p.add(fb.b_byte);
                        let (nr, ng, nb) = transform_pixel(r, g, b, sat);
                        *p.add(fb.r_byte) = nr;
                        *p.add(fb.g_byte) = ng;
                        *p.add(fb.b_byte) = nb;
                    }
                    2 => {
                        // RGB565 little-endian : RRRRRGGG GGGBBBBB.
                        let lo = *p as u16;
                        let hi = *p.add(1) as u16;
                        let px = lo | (hi << 8);
                        let r5 = ((px >> 11) & 0x1f) as u8;
                        let g6 = ((px >> 5) & 0x3f) as u8;
                        let b5 = (px & 0x1f) as u8;
                        // Étendre vers 8 bits.
                        let r = (r5 << 3) | (r5 >> 2);
                        let g = (g6 << 2) | (g6 >> 4);
                        let b = (b5 << 3) | (b5 >> 2);
                        let (nr, ng, nb) = transform_pixel(r, g, b, sat);
                        // Re-quantifier en 565.
                        let r5 = (nr >> 3) as u16;
                        let g6 = (ng >> 2) as u16;
                        let b5 = (nb >> 3) as u16;
                        let npx = (r5 << 11) | (g6 << 5) | b5;
                        *p = (npx & 0xff) as u8;
                        *p.add(1) = (npx >> 8) as u8;
                    }
                    _ => {}
                }
            }
        }

        // On laisse l'ioctl réel rafraîchir l'écran avec le contenu corrigé.
        let _ = munmap_noop();
    }
}

// Le mmap est conservé pour toute la durée de vie du process (pas de munmap explicite) :
// il est réutilisé à chaque update. Cette fonction documente ce choix sans rien faire.
#[inline]
fn munmap_noop() -> bool {
    false
}
