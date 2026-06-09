#!/bin/bash
# Assemble the "kobo-companion" bundle for the Kobo Libra Colour (tolino-qt6 firmware).
# Produces:
#   re/dist/Kobo.tgz                         -> goes in .kobo/ (boot: firewall + dbtool + daemon + display)
#   re/dist/onboard/.adds/kobo-companion/    -> goes on the USB partition (editable config + mTLS certs)
set -e
cd "$(dirname "$0")/.."          # -> repo root
ROOTFS=mnt
DIST=re/dist
NETCUT=re/netcut
KC=re/kobo-companion
MUSL=$KC/target/armv7-unknown-linux-musleabihf/release
GLIBC=$KC/target/fbhook/armv7-unknown-linux-gnueabihf/release

KS=$MUSL/kobo-syncd
KD=$MUSL/kobo-dbtool
FB=$GLIBC/libfbhook.so
ORIG="$ROOTFS/usr/local/Kobo/memorylogger"

for f in "$KS" "$KD" "$FB" "$ORIG"; do
  [ -f "$f" ] || { echo "ERROR: missing: $f"; echo "  (build first: cd re/kobo-companion && cross build --release --target armv7-unknown-linux-musleabihf -p kobo-syncd -p kobo-dbtool && cross build --release --target armv7-unknown-linux-gnueabihf -p fbhook --target-dir target/fbhook)"; exit 1; }
done

rm -rf "$DIST"; mkdir -p "$DIST/stage" "$DIST/onboard/.adds/kobo-companion/certs"

# --- 1) Kobo.tgz (extracted by /etc/init.d/ota into /usr/local/Kobo) ---
cp "$ORIG"                "$DIST/stage/memorylogger.bin"  # original binary
cp "$NETCUT/memorylogger" "$DIST/stage/memorylogger"      # boot wrapper
cp "$NETCUT/opds-netcut"  "$DIST/stage/opds-netcut.sh"    # firewall
cp "$KS"                  "$DIST/stage/kobo-syncd"        # Group C daemon (static musl)
cp "$KD"                  "$DIST/stage/kobo-dbtool"       # Group B tool (static musl)
cp "$FB"                  "$DIST/stage/libfbhook.so"      # Group A library (glibc .so)
chmod 755 "$DIST/stage/"*
tar -czf "$DIST/Kobo.tgz" -C "$DIST/stage" memorylogger memorylogger.bin opds-netcut.sh kobo-syncd kobo-dbtool libfbhook.so
echo "[OK] $DIST/Kobo.tgz"
tar -tzf "$DIST/Kobo.tgz" | sed 's/^/      /'

# --- 2) USB payload (.adds/kobo-companion/): editable config + certs ---
P="$DIST/onboard/.adds/kobo-companion"
cp "$KC/config/kobo-companion.conf" "$P/"
cp "$KC/config/features.conf"       "$P/"
cp "$KC/config/fbhook.conf"         "$P/"
cp "$KC/config/netcut.conf"         "$P/"
cat > "$P/certs/README.txt" <<'EOF'
Put your Caddy-issued mTLS identity here (the private key is NEVER committed to git):
  client.crt   (client certificate)
  client.key   (client private key)
  ca.crt       (Caddy CA, to trust the server)
Paths are configured in the [mtls] section of kobo-companion.conf.
EOF
echo "[OK] $P/ :"; ls -1 "$P" | sed 's/^/      /'

rm -rf "$DIST/stage"
echo
echo "ARM sizes: kobo-syncd $(( $(stat -c%s "$KS")/1024 ))KB · kobo-dbtool $(( $(stat -c%s "$KD")/1024 ))KB · libfbhook.so $(( $(stat -c%s "$FB")/1024 ))KB"
echo "Deploy: bash re/deploy.sh   (see re/kobo-companion/INSTALL.md)"
