#!/bin/bash
# deploy.sh — patch / un-patch your Kobo in one command (run on your PC, Kobo plugged in over USB).
#
#   PATCH:    bash re/deploy.sh --ip 192.168.1.10:5000 --apikey XXXX [--subnet 192.168.1.0/24]
#             [--features] (interactive module menu) [--firewall]
#             [--client-cert F --client-key F --ca-cert F]  (Caddy mTLS identity)
#   UNPATCH:  bash re/deploy.sh --unpatch
#   Common:   --kobo <mountpoint>   -y/--yes
set -e
cd "$(dirname "$0")/.."
DIST=re/dist
ROOTFS=mnt
PAY="$DIST/onboard/.adds/kobo-companion"

IP=""; APIKEY=""; SUBNET="192.168.1.0/24"; KOBO=""; FIREWALL="no"; ASSUME_YES=0; UNPATCH=0
DO_FEATURES=0; CLIENT_CERT=""; CLIENT_KEY=""; CA_CERT=""
while [ $# -gt 0 ]; do
  case "$1" in
    --ip) IP="$2"; shift 2;;
    --apikey) APIKEY="$2"; shift 2;;
    --subnet) SUBNET="$2"; shift 2;;
    --kobo) KOBO="$2"; shift 2;;
    --firewall) FIREWALL="yes"; shift;;
    --features) DO_FEATURES=1; shift;;
    --client-cert) CLIENT_CERT="$2"; shift 2;;
    --client-key) CLIENT_KEY="$2"; shift 2;;
    --ca-cert) CA_CERT="$2"; shift 2;;
    --unpatch) UNPATCH=1; shift;;
    -y|--yes) ASSUME_YES=1; shift;;
    *) echo "unknown option: $1"; exit 1;;
  esac
done

detect_kobo() {
  [ -n "$KOBO" ] && return 0
  for base in "/media/$USER" /run/media/"$USER" /media /mnt /run/media; do
    [ -d "$base" ] || continue
    for d in "$base"/*; do [ -d "$d/.kobo" ] && KOBO="$d" && return 0; done
  done
  return 1
}
confirm() { [ "$ASSUME_YES" = 1 ] && return 0; read -rp "$1 [y/N] " a; case "$a" in y|Y|o|O) return 0;; *) echo "aborted."; exit 0;; esac; }

# =========================== UNPATCH ===========================
if [ "$UNPATCH" = 1 ]; then
  ORIG="$ROOTFS/usr/local/Kobo/memorylogger"
  [ -f "$ORIG" ] || { echo "ERROR: $ORIG not found (is the rootfs mounted?)"; exit 1; }
  mkdir -p "$DIST/unstage"; cp "$ORIG" "$DIST/unstage/memorylogger"; chmod 755 "$DIST/unstage/memorylogger"
  tar -czf "$DIST/Kobo-unpatch.tgz" -C "$DIST/unstage" memorylogger; rm -rf "$DIST/unstage"
  echo ">> Kobo-unpatch.tgz ready (restores the original memorylogger)."
  detect_kobo || { echo "!! Kobo not detected. Plug it in + tap Connect, then: bash re/deploy.sh --unpatch --kobo <mountpoint>"; exit 2; }
  echo ">> Kobo detected: $KOBO"
  confirm "Un-patch $KOBO (restores the original, KEEPS your books)?"
  cp -v "$DIST/Kobo-unpatch.tgz" "$KOBO/.kobo/Kobo.tgz"
  rm -rf "$KOBO/.adds/kobo-companion"
  sync
  echo; echo "DONE. Eject + unplug -> reboot: the original memorylogger is restored,"
  echo "      no more daemon/firewall/display, internet restored. Your books stay in Nickel."
  echo "      (If the display mode was active, also remove /etc/ld.so.preload via telnet, or update the firmware.)"
  exit 0
fi

# =========================== PATCH ===========================
echo ">> building the bundle..."
bash re/build_bundle.sh >/dev/null
echo "   OK ($DIST/Kobo.tgz + config)"

if [ -z "$IP" ]; then read -rp "Kavita server IP:port (e.g. 192.168.1.10:5000): " IP; fi
if [ -z "$APIKEY" ]; then read -rp "Kavita API key: " APIKEY; fi
[ -z "$IP" ] && { echo "IP required."; exit 1; }
[ -z "$APIKEY" ] && { echo "API key required."; exit 1; }

CONF="$PAY/kobo-companion.conf"
sed -i -E "s#^base_url *=.*#base_url = http://$IP#" "$CONF"
sed -i -E "s#^api_key *=.*#api_key = $APIKEY#" "$CONF"
sed -i -E "s#^LAN_SUBNET=.*#LAN_SUBNET=\"$SUBNET\"#" "$PAY/netcut.conf" 2>/dev/null || true
sed -i -E "s#^ENABLE_FIREWALL=.*#ENABLE_FIREWALL=\"$FIREWALL\"#" "$PAY/netcut.conf" 2>/dev/null || true
echo ">> config: base_url=http://$IP  subnet=$SUBNET  firewall=$FIREWALL"

# mTLS certs (the private key is never committed — only copied to the device).
# The config template ships with EMPTY mtls paths (= plain HTTP); set them here only
# when an identity is supplied, so kclient doesn't try to read non-existent cert files.
if [ -n "$CLIENT_CERT" ] && [ -n "$CLIENT_KEY" ] && [ -n "$CA_CERT" ]; then
  cp "$CLIENT_CERT" "$PAY/certs/client.crt"; cp "$CLIENT_KEY" "$PAY/certs/client.key"; cp "$CA_CERT" "$PAY/certs/ca.crt"
  chmod 600 "$PAY/certs/client.key"
  CDIR=/mnt/onboard/.adds/kobo-companion/certs
  sed -i -E "s#^client_cert *=.*#client_cert = $CDIR/client.crt#" "$CONF"
  sed -i -E "s#^client_key *=.*#client_key  = $CDIR/client.key#" "$CONF"
  sed -i -E "s#^ca_cert *=.*#ca_cert     = $CDIR/ca.crt#"        "$CONF"
  echo ">> mTLS identity copied into certs/ and enabled in config"
fi

# Feature menu (writes features.conf)
if [ "$DO_FEATURES" = 1 ]; then
  echo ">> Select modules (empty answer = default in brackets):"
  : > "$PAY/features.conf"
  echo "# Enabled modules (deploy.sh --features)" >> "$PAY/features.conf"
  ask_feat() {  # $1=id $2=label $3=default(yes/no)
    local def="$3" ans
    read -rp "   $2 [$def] ? (y/n) " ans
    case "$ans" in y|Y|o|O) echo "FEAT_$1=yes";; n|N) echo "FEAT_$1=no";; *) [ "$def" = yes ] && echo "FEAT_$1=yes" || echo "FEAT_$1=no";; esac
  }
  echo "# Group C (daemon)" >> "$PAY/features.conf"
  for spec in "opds:OPDS (Kavita):yes" "wallabag:Wallabag read-it-later:no" "annotations:Highlights sync:no" \
              "positions:Reading positions:no" "stats:Prometheus stats export:no" "wifi:Conditional Wi-Fi:no"; do
    id=${spec%%:*}; rest=${spec#*:}; lbl=${rest%:*}; def=${rest##*:}
    ask_feat "$id" "$lbl" "$def" >> "$PAY/features.conf"
  done
  echo "# Group B (at boot)" >> "$PAY/features.conf"
  for spec in "collections:Collections/series:no" "watch:Watched folder (metadata):no" "sortfilter:Series from titles:no"; do
    id=${spec%%:*}; rest=${spec#*:}; lbl=${rest%:*}; def=${rest##*:}
    ask_feat "$id" "$lbl" "$def" >> "$PAY/features.conf"
  done
  echo "# Group A (display, validate on device first)" >> "$PAY/features.conf"
  ask_feat "display" "Display engine (colour/waveform/dither/night)" "no" >> "$PAY/features.conf"
  echo ">> features.conf written."
fi

detect_kobo || { echo "!! Kobo not detected. Plug it in + tap Connect, re-run with --kobo <mountpoint>. (Bundle is ready in $DIST/.)"; exit 2; }
echo ">> Kobo detected: $KOBO"
confirm "Copy the patch to $KOBO?"
mkdir -p "$KOBO/.adds/kobo-companion" "$KOBO/.kobo"
cp -rv "$PAY/." "$KOBO/.adds/kobo-companion/"
cp -v "$DIST/Kobo.tgz" "$KOBO/.kobo/Kobo.tgz"
sync
echo; echo "DONE. On the reader: eject + unplug -> reboot."
echo "      Connect Wi-Fi -> the enabled modules start. Logs: .adds/kobo-companion/{boot.log,kobo-syncd.log}."
echo "      To revert everything: bash re/deploy.sh --unpatch"
