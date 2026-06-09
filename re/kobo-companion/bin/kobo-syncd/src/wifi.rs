// Module wifi conditionnel — airplane par défaut, n'autorise le réseau que dans
// des fenêtres horaires (et éventuellement sur des SSID donnés). Pur std + Command.
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::Config;
use crate::log;

/// SSID courant via `iwgetid -r` (stdout trimé). None si indisponible.
fn current_ssid() -> Option<String> {
    let out = Command::new("iwgetid").arg("-r").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let ssid = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if ssid.is_empty() { None } else { Some(ssid) }
}

/// Parse une plage "a-b" (heures 0..23). Renvoie None si invalide.
fn parse_range(s: &str) -> Option<(i64, i64)> {
    let (a, b) = s.split_once('-')?;
    let a: i64 = a.trim().parse().ok()?;
    let b: i64 = b.trim().parse().ok()?;
    if !(0..=23).contains(&a) || !(0..=23).contains(&b) {
        return None;
    }
    Some((a, b))
}

/// L'heure `h` est-elle dans la plage [a, b] (gère le franchissement de minuit) ?
fn in_range(h: i64, a: i64, b: i64) -> bool {
    if a <= b {
        h >= a && h <= b
    } else {
        // plage qui enjambe minuit, ex. 22-6
        h >= a || h <= b
    }
}

/// Sommes-nous dans une fenêtre où le réseau est autorisé ?
pub fn allowed_now(c: &Config) -> bool {
    let allowed_hours = c.getlist("wifi", "allowed_hours");
    let allowed_ssids = c.getlist("wifi", "allowed_ssids");
    let default_airplane = c.getb("wifi", "default_airplane", true);
    let tz = c.getu("wifi", "tz_offset_hours", 0) as i64;

    // Aucune plage configurée -> suit la politique par défaut.
    if allowed_hours.is_empty() {
        return !default_airplane;
    }

    // Heure locale = ((epoch/3600 + tz) % 24), normalisée dans [0, 23].
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let hour = ((epoch / 3600 + tz) % 24 + 24) % 24;

    let in_window = allowed_hours
        .iter()
        .filter_map(|r| parse_range(r))
        .any(|(a, b)| in_range(hour, a, b));
    if !in_window {
        return false;
    }

    // Filtre SSID : appliqué uniquement si une liste est fournie ET le SSID connu.
    if !allowed_ssids.is_empty() {
        if let Some(ssid) = current_ssid() {
            return allowed_ssids.iter().any(|s| s == &ssid);
        }
        // SSID indéterminé -> on ignore le filtre SSID.
    }
    true
}

/// Force l'état wifi (up = activé, false = airplane/coupé). Best-effort, ne panique jamais.
pub fn ensure(c: &Config, up: bool) {
    let action = if up { "up" } else { "down" };
    // Sur Kobo : interface wlan0 pilotée via ifconfig.
    let result = Command::new("ifconfig")
        .arg("wlan0")
        .arg(action)
        .output();
    match result {
        Ok(o) if o.status.success() => {
            log(&c.dest, &format!("wifi: wlan0 {}", action));
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            eprintln!("wifi: échec ifconfig wlan0 {} ({})", action, err.trim());
            log(&c.dest, &format!("wifi: échec wlan0 {}", action));
        }
        Err(e) => {
            eprintln!("wifi: ifconfig indisponible: {}", e);
        }
    }
}
