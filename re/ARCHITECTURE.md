# Feature architecture (à-la-carte patches)

Goal: add features that are **installable à la carte** (the deployer asks which ones), while
keeping **Nickel** as the reader, on the tolino-qt6 5.1.x firmware. Reminder: this firmware has
**no `KoboRoot.tgz`**, so delivery goes through `Kobo.tgz` (→ `/usr/local/Kobo`) plus editable
config on the USB partition (`/mnt/onboard/.adds/kobo-companion/`). Boot hook = the
`memorylogger` wrapper (already in place).

## Components

| Component | Type | Group | Role |
|---|---|---|---|
| `kclient` | Rust crate | shared | single **mTLS** HTTP client (reqwest + rustls, identity from your Caddy CA) |
| `convert` | Rust crate | shared (B+C) | HTML→EPUB (readability), CBR→CBZ, EPUB metadata/cover |
| `kobo-syncd` | Rust binary | C | **one daemon**: OPDS, Wallabag, annotations, positions (KOSync), conditional Wi-Fi, stats |
| `kobo-dbtool` | Rust binary | B | edits `KoboReader.sqlite`: collections/series, watched-folder metadata/covers, sort/filter |
| `libfbhook.so` | cdylib (LD_PRELOAD) | A | intercepts `ioctl()` on `/dev/fb0`: OKLab/CFA colour, auto waveform, dithering, night mode |

Everything is **static musl ARMv7 hard-float** (`cross`), like the binaries — except
`libfbhook.so`, which is a **glibc** shared library because it is injected into Nickel (a glibc
process). One Rust workspace: `re/kobo-companion/`.

## Group C — sync daemon (most natural to extend; OPDS already done)

`kobo-syncd` reads `features.conf` + `kobo-companion.conf` and runs the enabled modules:
- **OPDS + mTLS** — the original sync, on the shared `kclient`.
- **Wallabag** — pulls unread articles, converts HTML→EPUB (`convert`), drops them in the library.
- **Annotations/highlights** — reads `Bookmark` from `KoboReader.sqlite`, pushes to a
  self-hosted endpoint (mTLS), optional pull/merge.
- **Reading positions (KOSync-like)** — push/pull progress per book.
- **Conditional Wi-Fi** — airplane by default; enables Wi-Fi only inside allowed windows
  (hours/SSID); skips network modules otherwise.
- **Stats export** — writes a Prometheus/text file from the database (no network).

All share **`kclient`** (one mTLS client, identity loaded once).

## Group B — library/database (`kobo-dbtool`)

Edits `/mnt/onboard/.kobo/KoboReader.sqlite` (a timestamped backup is made before writing):
- **Collections/series** — fills `Shelf`/`ShelfContent` (Nickel collections) for sideloaded content.
- **Watched folder** — scans a folder, fills metadata + covers (via `convert`), without Calibre/USB.
- **Sort/filter** — fills `content.Series`/`SeriesNumber` (Nickel groups/sorts by series). Note:
  arbitrary UI sorting is not possible without patching Nickel; documented honestly.

Robust to schema differences (`PRAGMA table_info` before writing), idempotent.

## Group A — display engine (`libfbhook.so`, the trickiest)

`LD_PRELOAD`'d into Nickel; intercepts `ioctl(fd, MXCFB_SEND_UPDATE, arg)` on the framebuffer:
- **Comic colour** — OKLab LUT (saturation/gamma) adapted to the **Kaleido 3 CFA** (RGBW subpixels).
- **Auto waveform** — picks `GC16` (colour/comics) vs `DU`/`A2` (text) from the update region.
- **CFA dithering** — soft, or via `EPDC_FLAG_USE_DITHERING_*`.
- **Gamma-corrected night mode**.

⚠️ The EPDC ioctl numbers and the `mxcfb_update_data` layout follow the **NXP reference** and
**must be validated** on this MTK device (kernel headers or RE) before real use. Everything is
guarded: if the layout doesn't match, the ioctl is forwarded untransformed (no crash). Treat as
last, behind a flag, with a kill-switch (remove the `/etc/ld.so.preload` line).

## Shared brick — `kclient` (mTLS)

`reqwest` + `rustls` (ring backend), **client identity** (cert+key PEM) issued by your **Caddy**
internal CA. Cert + key + CA live in `/mnt/onboard/.adds/kobo-companion/certs/` (the private key
is **never** committed; `deploy.sh --client-cert/--client-key/--ca-cert` copies them). The CA also
lets the client trust a self-hosted server cert.

## Feature selection (`deploy.sh`)

`deploy.sh --features` shows a per-group menu and writes `features.conf` (`FEAT_*=yes/no`) into
the USB payload. The boot wrapper reads it and: starts `kobo-syncd` with the enabled C modules;
runs `kobo-dbtool` tasks before Nickel; enables the display preload if Group A is on.

## Build order

1. **`kclient` (mTLS)** — foundation for all of Group C.
2. **Group C** — OPDS exists; add modules one by one.
3. **`convert`** (HTML→EPUB for Wallabag; CBR→CBZ via the `unrar` crate).
4. **Group B** — `kobo-dbtool` (reuses `convert` for covers/metadata).
5. **Group A** — `libfbhook.so`, last (riskiest, after the kernel/CFA unknowns are resolved).
