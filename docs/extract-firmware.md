# Extracting and mounting your firmware

The build needs your device's **original `memorylogger` binary** (the boot wrapper `exec`s it).
That binary is firmware-specific and copyrighted, so it is **not** shipped in this repo — you
provide it by extracting your own firmware and mounting its rootfs at `./mnt`.

> Only the `bl2`/`uboot`/`tee`/`kernel` images are HAB-signed (do not reflash those). The
> **rootfs is not verified** at boot or runtime, so reading/mounting it is safe and reversible.

## 1. Get the firmware update package

Kobo firmware is distributed as an `update.tar`. Download the version that matches your device
from Kobo's CDN (the exact URL depends on model/region/version — see the Kobo/MobileRead
community for the link to **5.1.x, tolino-qt6** for the Libra Colour). Save it as `update.tar`.

## 2. Extract and mount the rootfs

From the repo root:

```bash
mkdir -p mnt
tar -xf update.tar                         # -> rootfs.img, kernel images, driver.sh, ...
zstd -d rootfs.img -o rootfs.ext4          # the rootfs is a zstd-compressed ext4 image (~1 GiB)
sudo mount -o loop,ro rootfs.ext4 mnt      # read-only is enough for building the bundle
```

Verify it worked:

```bash
ls mnt/usr/local/Kobo/memorylogger         # the binary the build needs
ls mnt/usr/local/Kobo/libnickel.so.1.0.0   # the main Nickel library
```

You can now build and deploy (see [../re/kobo-companion/INSTALL.md](../re/kobo-companion/INSTALL.md)).

## Notes

- Mount **read-only** (`-o loop,ro`) — the build never writes to your firmware.
- The extracted firmware, `rootfs.ext4`, `rootfs.img` and the `mnt/` mountpoint are all
  git-ignored; nothing copyrighted ends up in the repo.
- To unmount when done: `sudo umount mnt`.
