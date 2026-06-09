# kobo-companion

Make a **Kobo Libra Colour** private and self-hosted **without replacing the Nickel reader**:
cut its network ties (telemetry, store) and sync your own books from a personal **OPDS
server** (Kavita, calibre-web…). Plus optional modules for highlights/positions sync,
reading stats, a watched folder, collections, and an e-ink display tuner.

> ⚠️ **Read this first.** This is an unofficial, community project. It is **not affiliated
> with Kobo/Rakuten**. It modifies your device's userspace; you do it **at your own risk**.
> It was built and tested against the **Kobo Libra Colour** on firmware **5.1.x (tolino-qt6
> branch)**. Other models/firmwares are untested and could behave differently. The Kobo
> firmware is **not included** here (copyright) — you extract it from your own device.
> No DRM is circumvented. Everything is reversible (`deploy.sh --unpatch`).

## What it does

- **Network layer** — an `iptables` firewall ("allow LAN, block internet") plus a
  `/etc/hosts` blocklist, to neutralise telemetry and the store **without touching the reader**.
- **`kobo-syncd`** — a small **Rust** daemon (single static ARM binary, no runtime deps) that
  walks your OPDS catalogue and drops books into internal storage; **Nickel imports them
  automatically**. Optional modules: Wallabag, highlights/positions sync, stats, conditional Wi-Fi.
- **`kobo-dbtool`** — organises the Nickel library (collections, watched folder + metadata,
  series) by editing `KoboReader.sqlite` (with an automatic backup first).
- **`libfbhook.so`** — an optional e-ink display tuner (colour/waveform/dithering/night),
  `LD_PRELOAD`'d into Nickel. ⚠️ Hardware-specific — validate on your device first.
- **`deploy.sh`** — installs or removes everything in **one command**, using Kobo's own
  `Kobo.tgz` mechanism. Reversible.

All on-device code is **Rust**, cross-compiled to static ARM binaries (the display library is
a glibc `.so`). It is **not** a KOReader/replacement OS — Nickel stays your reader.

## Requirements

- A **Kobo Libra Colour** (firmware 5.1.x, tolino-qt6). You must be comfortable extracting
  and re-mounting your own firmware image (see below).
- A PC with **Rust**, [`cross`](https://github.com/cross-rs/cross) and **Docker** (to build
  the ARM binaries), plus `zstd` and standard tools.
- An **OPDS server on your LAN** (Kavita is the reference; calibre-web/COPS also work).
- Optional: a **Caddy** (or step-ca) internal CA if you want **mTLS**.

## Quick start

```bash
# 1) Extract your firmware and mount its rootfs at ./mnt  (see docs/extract-firmware.md).
#    The boot wrapper needs your device's original /usr/local/Kobo/memorylogger, which the
#    build copies from the mounted rootfs. It is firmware-specific and never committed here.

# 2) Build the ARM binaries
cd re/kobo-companion
cross build --release --target armv7-unknown-linux-musleabihf -p kobo-syncd -p kobo-dbtool
cross build --release --target armv7-unknown-linux-gnueabihf -p fbhook --target-dir target/fbhook
cd -

# 3) Plug in the Kobo, tap "Connect", then:
bash re/deploy.sh --ip 192.168.1.10:5000 --apikey YOUR_KAVITA_KEY --features

# 4) Eject + unplug. The reader reboots and applies the patch.

# Revert everything (keeps your books):
bash re/deploy.sh --unpatch
```

Step-by-step guide: [re/kobo-companion/INSTALL.md](re/kobo-companion/INSTALL.md).

## Features (pick what you install)

Selected at deploy time (`deploy.sh --features`) and recorded in `features.conf`.

- **Group C — `kobo-syncd` daemon** (shared mTLS client `kclient`): OPDS, Wallabag
  (read-it-later → EPUB), highlights sync, reading positions (KOSync-style), conditional Wi-Fi
  (hours/SSID, airplane by default), stats export (Prometheus text).
- **Group B — `kobo-dbtool`** (edits `KoboReader.sqlite`, auto-backup first): collections/series,
  watched folder + metadata/covers (no Calibre), series-from-titles.
- **Group A — `libfbhook.so`** (`LD_PRELOAD` into Nickel): OKLab colour correction (Kaleido 3
  CFA), automatic waveform, dithering, night mode. ⚠️ EPDC ABI to validate on device.
- **Shared crates**: `convert` (HTML→EPUB readability, CBR→CBZ), `kclient` (one mTLS HTTP client).

Status: everything compiles and cross-compiles to ARM; OPDS+mTLS, stats, collections,
series-from-titles and HTML→EPUB are tested; **Group A still needs hardware validation**.

## How it works (and why it's safe-ish)

This firmware has **no `KoboRoot.tgz` → `/`** mechanism. Instead, files are delivered through
`Kobo.tgz`, which the firmware's own updater extracts into `/usr/local/Kobo` at boot. We ship a
thin **wrapper** over the non-essential `memorylogger` binary (launched at boot by
`/etc/rc.local`); it starts the firewall + daemon and `exec`s the real `memorylogger`. Nothing
in Nickel is modified. Reverting re-installs the original `memorylogger`.

See [re/RECONSTRUCTION.md](re/RECONSTRUCTION.md) for the reverse-engineering that established
all of this (network chokepoints, endpoints, the delivery mechanism), and
[re/ARCHITECTURE.md](re/ARCHITECTURE.md) for the module design.

## Safety, risk & recovery

**Can it brick the device?** A *permanent* brick is essentially ruled out: these patches never
touch the **HAB-signed boot chain** (`bl2`/`uboot`/`tee`/`kernel`) — touching *that* is what can
brick a Kobo, and the project never does it. Everything is delivered via Kobo's own `Kobo.tgz`
mechanism, which only writes userspace (`/usr/local/Kobo`); the rootfs is unsigned and always
re-flashable, so the firmware updater / recovery partition is the ultimate safety net. The boot
wrapper also always `exec`s the original `memorylogger` and does **not** gate Nickel's launch, so
a broken wrapper does not mean "no UI".

**Risk by module** (lowest → highest):

| Module | Risk | Worst case | Recovery |
|---|---|---|---|
| **Group C** (sync) + **firewall** | very low | no network (wrong subnet) | edit `netcut.conf`, or kill-switch `opds-netcut.sh open` |
| **Group B** (edits `KoboReader.sqlite`) | low | corrupted DB → Nickel rebuilds the library | a timestamped backup is made first; restore the `.bak` |
| **Group A** (display, `fbhook`) | **real** ⚠️ | crashing processes / **boot loop** | remove `/etc/ld.so.preload`, `--unpatch`, else re-flash firmware |

**Group A is the one to watch.** It injects via a **global `/etc/ld.so.preload`** (every process,
not just Nickel) and its **EPDC ABI is unvalidated** on this MTK SoC (it follows the NXP
reference). A bug there could crash processes and, worst case, cause a boot loop. It is still
**recoverable** (at worst by re-flashing the firmware), but it is the only module that can cause
real trouble. It is **off by default** (`FEAT_display=no`); validate it step by step on your
device before relying on it.

**Before you test — precautions:**
1. **Enable Kobo dev-mode / telnet first**, so you always have a shell to undo things.
2. Keep a **firmware `update.tar`** handy (recovery).
3. Test in this order: **Group C first** (sync) → enable the firewall once it works → then Group B.
   **Leave Group A disabled** until you've validated it.

**Recovery toolbox:**
- **Undo everything:** `bash re/deploy.sh --unpatch` (restores the original, keeps your books).
- **Re-enable the internet instantly** (telnet): `/usr/local/Kobo/opds-netcut.sh open`.
- **Disable the display preload:** remove the `libfbhook.so` line from `/etc/ld.so.preload`.
- **Last resort:** re-flash the firmware via `update.tar` (also wipes `/usr/local/Kobo`, so
  re-apply the patch afterwards if you still want it).

> Note: a **firmware update wipes `/usr/local/Kobo`**, removing the wrapper — re-apply afterwards.

## Project layout

```
re/kobo-companion/        The Rust workspace (on-device app)
  crates/kclient/         Shared mTLS HTTP client
  crates/convert/         HTML→EPUB, CBR→CBZ, EPUB metadata
  crates/fbhook/          Group A display library (LD_PRELOAD cdylib)
  bin/kobo-syncd/         Group C sync daemon (+ modules)
  bin/kobo-dbtool/        Group B library/database tool
  config/                 Config templates (copied to the device)
re/netcut/                Firewall + /etc/hosts blocklist + boot wrapper
re/build_bundle.sh        Assembles Kobo.tgz + the USB payload
re/deploy.sh              One-command install / --unpatch
re/RECONSTRUCTION.md      Reverse-engineering notes (libnickel)
re/ARCHITECTURE.md        Module/feature design
```
Generated artefacts (`re/out/`, `re/dist/`, Cargo `target/`, the Ghidra project) and the
firmware itself are not versioned — see `.gitignore`.

## Legal / ethics

A personal **interoperability and privacy** project for **your own device**. No DRM is
circumvented. No Kobo binary or decompiled code is redistributed here (excluded via
`.gitignore`). Use on hardware you own, at your own risk.

## License

[MIT](LICENSE) for the original code in this repo. (The Kobo firmware and any
reverse-engineered material are not covered and are not redistributed here.)
