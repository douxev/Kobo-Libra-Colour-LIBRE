# Installation & usage

Step-by-step guide to patch a **Kobo Libra Colour** (firmware 5.1.x, tolino-qt6) with
kobo-companion. Keeps **Nickel** as the reader. Everything is reversible.

> Safety: do this on your own device, at your own risk. A firmware update will wipe the
> patch. Keep a way to access a shell (Kobo dev-mode telnet) before enabling the firewall or
> the display module, so you can recover.

## 0. Prerequisites

- PC with **Rust**, [`cross`](https://github.com/cross-rs/cross), **Docker** (running, your
  user in the `docker` group), plus `tar`/`zstd`.
- Your firmware extracted and its rootfs mounted at `./mnt` — see
  [../../docs/extract-firmware.md](../../docs/extract-firmware.md). This is required: the boot
  wrapper needs your device's original `/usr/local/Kobo/memorylogger`, which the build copies
  from `mnt/`.
- An **OPDS server on your LAN** (Kavita reference). Note its **IP:port** and **API key**
  (Kavita → Account settings → API key / OPDS URL).
- Your **LAN subnet** (often `192.168.1.0/24`).
- Optional, for **mTLS**: a client cert + key + CA from your Caddy/step-ca PKI.

## 1. Build the ARM binaries

```bash
cd re/kobo-companion
cross build --release --target armv7-unknown-linux-musleabihf -p kobo-syncd -p kobo-dbtool
cross build --release --target armv7-unknown-linux-gnueabihf -p fbhook --target-dir target/fbhook
cd -
```
This produces two static musl binaries (`kobo-syncd`, `kobo-dbtool`) and one glibc shared
library (`libfbhook.so`).

## 2. Deploy (one command)

Plug in the Kobo and tap **Connect** (USB mass storage). Then:

```bash
bash re/deploy.sh --ip 192.168.1.10:5000 --apikey YOUR_KAVITA_KEY --features
```

`deploy.sh` builds the bundle, writes your config, runs the **feature menu**, detects the
Kobo, and copies everything. Useful flags:

| Flag | Meaning |
|---|---|
| `--ip HOST:PORT` | Kavita server (writes `base_url`) |
| `--apikey KEY` | Kavita API key |
| `--subnet CIDR` | LAN subnet for the firewall (default `192.168.1.0/24`) |
| `--features` | interactive module selection |
| `--firewall` | enable the internet cut immediately (default: off, enable after testing) |
| `--client-cert F --client-key F --ca-cert F` | install your mTLS identity (the key is copied to the device only) |
| `--kobo PATH` | mountpoint, if auto-detection fails |
| `-y` | no confirmation prompt |

Then **eject and unplug**. The reader reboots and the firmware applies `Kobo.tgz`.

## 3. First run & validation

1. Reboot, connect Wi-Fi.
2. The daemon waits until the server is reachable, then downloads (tip: set `max_books = 5`
   in `kobo-companion.conf` for the first test).
3. Plug in over USB and check **`<KOBO>/OPDS/`** for the books, and the logs
   **`<KOBO>/.adds/kobo-companion/{boot.log,kobo-syncd.log}`**.
4. Unplug → Nickel scans and adds the books.

### Manual run (optional, via telnet)
The device ships `telnetd` (Kobo dev-mode). With a shell:
```sh
CONF=/mnt/onboard/.adds/kobo-companion/kobo-companion.conf
/usr/local/Kobo/kobo-syncd --config $CONF --features /mnt/onboard/.adds/kobo-companion/features.conf --once
/usr/local/Kobo/kobo-dbtool collections --config $CONF
```

## 4. Cut the internet (once sync works)

Edit `<KOBO>/.adds/kobo-companion/netcut.conf` → `ENABLE_FIREWALL="yes"`, set `LAN_SUBNET`,
reboot. Now only the LAN is reachable (Kavita works; telemetry/store/Google/Rakuten are blocked).
(Or pass `--firewall` to `deploy.sh` once you're confident.)

## 5. Configuration reference

All config lives in `<KOBO>/.adds/kobo-companion/` and is editable over USB.

- **`kobo-companion.conf`** — server, sync, mTLS, and every module's settings (see the inline
  comments). For self-signed HTTPS, prefer mTLS with your CA, or use plain HTTP on the LAN.
- **`features.conf`** — which modules are enabled (`FEAT_*=yes/no`).
- **`netcut.conf`** — firewall: `LAN_SUBNET`, optional `OPDS_HOST/OPDS_PORT` hardening,
  `ENABLE_FIREWALL`.
- **`fbhook.conf`** — display module options (Group A).
- **`certs/`** — your mTLS `client.crt` / `client.key` / `ca.crt` (private key never leaves
  your devices).

### Feature notes
- **Wallabag**: set `[wallabag] url` + OAuth2 (`client_id/secret/username/password`) or a `token`.
- **Annotations / positions**: set the `endpoint` (your self-hosted sync server); `pull = true`
  to also merge back.
- **Wi-Fi conditional**: `default_airplane = true`, `allowed_hours = 7-9,18-23`, optional
  `allowed_ssids`. The daemon skips network modules outside the window.
- **Group B (collections/watch/sortfilter)**: run once at boot, before Nickel opens the DB.
  A timestamped backup of `KoboReader.sqlite` is made automatically.
- **Group A (display)**: validate on device first; the EPDC ioctl numbers/struct follow the
  NXP reference and may differ on this MTK SoC. If anything looks wrong, disable it.

## 6. Reverting & troubleshooting

- **Undo everything:** `bash re/deploy.sh --unpatch` (restores the original, keeps your books).
- **Re-open the internet now** (telnet): `/usr/local/Kobo/opds-netcut.sh open`.
- **Disable the display preload**: remove the `libfbhook.so` line from `/etc/ld.so.preload`
  (or set `FEAT_display=no` and reboot, or `--unpatch`).
- **Logs**: `/mnt/onboard/.adds/kobo-companion/{boot.log,kobo-syncd.log,opds-netcut.log}`.

### Known limitations (honest)
- The wrapper replaces `/usr/local/Kobo/memorylogger` (a non-essential diagnostic binary). A
  **firmware update overwrites it** → re-copy `Kobo.tgz` after each update.
- `connman` (Wi-Fi) also touches iptables; it *could* interfere with the firewall on connect.
  Validate; if needed, the firewall can be re-applied periodically.
- Self-signed HTTPS without mTLS is not supported by the binary; use HTTP on the cut LAN, or a
  valid cert, or mTLS.
- "Custom sort/filter" is limited to what Nickel reads from the database (series/collections);
  arbitrary UI sorting would require patching Nickel itself.
