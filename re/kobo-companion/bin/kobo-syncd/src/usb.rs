// USB-aware guard.
//
// Why: on Kobo/tolino, entering USB mass storage means Nickel must UNMOUNT /mnt/onboard to
// hand the raw partition to the PC. If kobo-syncd is mid-sync (a book file open on that
// partition), the unmount fails and the host never sees the storage ("Failed to start the
// USBMS session" — the device shows "connecting" but the computer sees nothing).
//
// Fix: whenever a USB cable is present, we skip starting a sync and abort an in-progress one,
// so kobo-syncd holds no handle under /mnt/onboard while the user plugs into a PC. Sync
// resumes on the next cycle once unplugged.
//
// Detection: scan /sys/class/power_supply for a USB-type supply reporting online=1. This also
// matches a plain wall charger (we can't always tell PC-data from charge at this layer), so a
// sync is paused while charging too — acceptable for an e-reader (it syncs on battery during
// the normal cycle). Disable with [net] pause_on_usb = false.
use crate::config::Config;
use std::fs;

/// True if a USB power supply reports a cable as connected (PC or charger).
pub fn cable_connected() -> bool {
    let entries = match fs::read_dir("/sys/class/power_supply") {
        Ok(e) => e,
        Err(_) => return false, // not a Kobo / sysfs absent → never pause
    };
    for ent in entries.flatten() {
        let dir = ent.path();
        let name = ent.file_name().to_string_lossy().to_lowercase();
        let kind = fs::read_to_string(dir.join("type")).unwrap_or_default();
        let is_usb = kind.trim().starts_with("USB") || name.contains("usb");
        if !is_usb {
            continue;
        }
        if fs::read_to_string(dir.join("online"))
            .map(|s| s.trim() == "1")
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

/// Honour the [net] pause_on_usb switch (default on) AND a connected cable.
pub fn should_pause(c: &Config) -> bool {
    c.getb("net", "pause_on_usb", true) && cable_connected()
}
