// fbhook — lib LD_PRELOAD injectée dans nickel. Intercepte ioctl() sur le framebuffer EPD
// pour appliquer : correction couleur (OKLab/CFA Kaleido), waveform auto, dithering, mode nuit.
//
// ⚠️ ABI EPDC dépendante du noyau : les numéros d'ioctl et le layout `MxcfbUpdateData`
// ci-dessous suivent l'ABI mxcfb/EPDC NXP de référence. Sur ce device (MTK monza) ils DOIVENT
// être validés (headers kernel ou RE) avant usage réel. Tout est gardé : si le layout ne
// correspond pas, on relaie l'ioctl sans transformer (aucun crash).
//
// Cette lib cible la glibc (armv7-unknown-linux-gnueabihf) car elle vit dans nickel (glibc).
#![allow(non_camel_case_types)]
use libc::{c_int, c_ulong, c_void};
use std::sync::Once;

mod transform;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MxcfbRect {
    pub top: u32,
    pub left: u32,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MxcfbAltBufferData {
    pub phys_addr: u32,
    pub width: u32,
    pub height: u32,
    pub alt_update_region: MxcfbRect,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MxcfbUpdateData {
    pub update_region: MxcfbRect,
    pub waveform_mode: u32,
    pub update_mode: u32,
    pub update_marker: u32,
    pub temp: c_int,
    pub flags: u32,
    pub dither_mode: c_int,
    pub quant_bit: c_int,
    pub alt_buffer_data: MxcfbAltBufferData,
}

// Modes waveform EPDC (référence NXP)
pub const WAVEFORM_MODE_AUTO: u32 = 257;
pub const WAVEFORM_MODE_DU: u32 = 1;
pub const WAVEFORM_MODE_GC16: u32 = 2;
pub const WAVEFORM_MODE_GL16: u32 = 3;
pub const WAVEFORM_MODE_A2: u32 = 4;

// Flags EPDC (référence NXP)
pub const EPDC_FLAG_ENABLE_INVERSION: u32 = 0x01;
pub const EPDC_FLAG_FORCE_MONOCHROME: u32 = 0x02;
pub const EPDC_FLAG_USE_DITHERING_Y1: u32 = 0x2000;
pub const EPDC_FLAG_USE_DITHERING_Y4: u32 = 0x4000;

// Numéro d'ioctl MXCFB_SEND_UPDATE = _IOW('F', 0x2E, struct mxcfb_update_data) (référence NXP).
// _IOW(type,nr,size) = (1<<30) | (size<<16) | (type<<8) | nr
const fn iow(typ: u32, nr: u32, size: u32) -> c_ulong {
    ((1u32 << 30) | (size << 16) | (typ << 8) | nr) as c_ulong
}
pub fn mxcfb_send_update() -> c_ulong {
    iow(b'F' as u32, 0x2E, core::mem::size_of::<MxcfbUpdateData>() as u32)
}

type RealIoctl = unsafe extern "C" fn(c_int, c_ulong, *mut c_void) -> c_int;
static mut REAL: Option<RealIoctl> = None;
static INIT: Once = Once::new();

unsafe fn real_ioctl() -> RealIoctl {
    INIT.call_once(|| {
        let sym = libc::dlsym(libc::RTLD_NEXT, b"ioctl\0".as_ptr() as *const _);
        if !sym.is_null() {
            REAL = Some(core::mem::transmute::<*mut c_void, RealIoctl>(sym));
        }
    });
    REAL.expect("ioctl original introuvable")
}

/// Symbole interposé (LD_PRELOAD). On suppose le 3e argument = pointeur (cas des ioctl FB).
#[no_mangle]
pub unsafe extern "C" fn ioctl(fd: c_int, request: c_ulong, arg: *mut c_void) -> c_int {
    // On ne transforme que les mises à jour d'écran connues, et jamais on ne casse l'appel.
    let is_update = request == mxcfb_send_update();
    if is_update && !arg.is_null() {
        let upd = &mut *(arg as *mut MxcfbUpdateData);
        // Hook (implémenté dans transform.rs) : peut modifier upd (waveform/flags) et/ou le fb.
        transform::on_send_update(fd, upd);
    }
    real_ioctl()(fd, request, arg)
}
