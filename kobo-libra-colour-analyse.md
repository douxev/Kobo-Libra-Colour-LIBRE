# Kobo Libra Colour — Analyse firmware & plan d'indépendance

## Objectif
Rendre la liseuse indépendante : couper tout lien réseau avec Kobo/Rakuten/Google,
et utiliser une UI de lecture orientée OPDS (serveur OPDS perso comme seule source).
Mises à jour faites manuellement.

---

## Matériel / firmware
- **Modèle** : Kobo Libra Colour (N428), code interne `kobo11` / `monza`.
- **SoC** : NXP i.MX8 (32-bit ARM EABI5 sur l'userspace).
- **Firmware analysé** : 5.1.186250 (branche Qt6).
- **Secure Boot** : HAB (High Assurance Boot) NXP actif sur la chaîne de boot.

### URL firmware (hors whitelist réseau du sandbox, à télécharger soi-même)
```
https://ereaderfiles.kobo.com/firmwares/kobo11/Jul2024/tolino-qt6-update-5.1.186250/update.tar
```

### Procédure d'extraction
```bash
curl -L -o update.tar "<url ci-dessus>"
mkdir fw && tar -xf update.tar -C fw && cd fw
zstd -d rootfs.img -o rootfs.ext4
mkdir mnt && sudo mount -o loop,ro rootfs.ext4 mnt
```

---

## Structure du paquet update.tar
- `driver.sh` — script d'update (stage1 / stage2)
- `decompressor` — wrapper `unzstd`
- `sha2-256sums` — checksums (packés zstd)
- `bl2.img`, `uboot.img`, `tee.img` — chaîne de boot (SIGNÉS HAB — NE PAS toucher)
- `monza/`, `spa-bw/`, `spa-colour/` — par device, chacun : `kernel.img`, `ntxfw.img`
- `rootfs.img` — ext4 packé zstd (image décompressée = 1 GiB)

### Schéma de partitions (A/B)
`system_a` (rootfs), `boot_a` (kernel), `tee_a`, `recovery`, `vendor`, `ntxfw`, `bl2`, `UBOOT`, `hwcfg`.
Sélection de la partition de boot via `ntx_hwconfig ... BootPartNo`.

---

## CONCLUSIONS CLÉS DE SÉCURITÉ (vérifiées sur le rootfs réel)

### 1. Aucune vérification d'intégrité runtime du rootfs
- **Pas de dm-verity** : recherches `verity`/`veritysetup`/`*.hashtree` dans
  `etc/`, `usr/local/Kobo/`, kernel → toutes vides.
- **Pas d'IMA/EVM** : `ima_policy`/`ima_appraise`/`CONFIG_IMA` → rien.
- **uboot bootargs** : pas de `root=...verity`.
=> Le rootfs n'est vérifié ni au boot ni au runtime. **Son contenu est librement modifiable.**

### 2. L'updater ne vérifie QUE des checksums en clair (pas de signature)
`driver.sh` → `check_checksums()` compare chaque image à un SHA-256 contenu dans
la MÊME archive. Le flash est un `dd` brut :
```
tar -xOf ARCHIVE img | decompressor | dd of=/dev/disk/by-partlabel/LABEL
```
Pas de signature, pas de clé, aucun `.sig`/`.csf` détaché.
=> Pour reflasher une rootfs modifiée : éditer le contenu, recalculer le SHA-256,
   mettre à jour la ligne dans `sha2-256sums` (repacké zstd), repacker `update.tar`.

### 3. Secure Boot ne bloque PAS l'objectif
HAB protège `bl2/uboot/tee/kernel` uniquement. Le rootfs (`system_a`) est hors
périmètre vérifié. Donc :
- Modifier le userspace (couper réseau, changer la liseuse) = **OK, sans casser HAB**.
- NE JAMAIS reflasher un `kernel/bl2/uboot/tee` modifié (fuses SRK probablement
  brûlés → brick au niveau recovery).

### 4. Canaux de modif userspace — CORRIGÉ (vérifié sur `/etc/init.d/ota`)
- ⚠️ **PAS de `KoboRoot.tgz → /`** sur cette branche tolino-qt6 (grep KoboRoot = vide).
  Le plan initial se trompait : ce mécanisme classique n'existe pas ici.
- **`Kobo.tgz`** (léger) : extrait UNIQUEMENT dans **`/usr/local/Kobo`** (pas `/etc`),
  puis supprimé. Suffisant pour des scripts/wrappers, pas pour `/etc/hosts` ni init.d.
- **rootfs.img complète reflashée** (version finale figée) : seul canal pour `/etc/*` ;
  recalcul SHA-256 + repack `update.tar` requis.
- Hook de boot garanti = **`/etc/rc.local`** (rc5.d/S99, lance nickel).
- → Couche réseau générée dans `re/netcut/` (hosts blackhole + pare-feu iptables + README).

---

## ANALYSE DE libnickel.so.1.0.0

### Nature du binaire
- ELF 32-bit ARM, **stripped** (`file` confirme, pas de `.symtab`).
- MAIS `.dynsym` énorme : **82 809 symboles exportés**, dont **61 824 `_ZN`** (C++ mangled).
- L'export Qt (meta-object) expose beaucoup de classes nommées : `TouchDropDown`,
  `RenderEngineType::Enum`, `PowerSettings`, `SyncState`, `DeviceDiscoverer`, etc.
- **Présence de Rust** lié statiquement : `core::`, `alloc::`, `aho_corasick`,
  `regex_automata`, `serde_json`, `bytes`, `safer_ffi`, `time`, `smartstring`.
- **Module DRM Rust `fucrl`** : `unzip_{lcp,kobo,tolino}_drm_epub_file`,
  `decrypt_*_content_key`, `test_passphrase_on_lcpl` (Readium LCP). HORS SUJET +
  contournement DRM exclu — ne pas explorer.
- Grosses libs statiques (non-UI) : `dropboxQt::*` (851+), `rxcpp::*`, `marisa::*`,
  `zip/zstd/flate2/bzip2`, `gimli`, `memchr`, `aes`.

### Verdict reverse engineering (IMPORTANT) — CONFIRMÉ PAR DÉCOMPILATION RÉELLE
> Décompilation effectivement réalisée (Ghidra 12.1 + capstone). Rapport détaillé +
> livrables : **`/home/maelle/fw/re/RECONSTRUCTION.md`** et `/home/maelle/fw/re/out/`.

- **Fait et fiable** : table de symboles démanglée (82 803), **graphe d'héritage RTTI**
  (3 779 relations), **headers C++ reconstruits** (21 classes, vtables ordonnées, types
  de retour via chaînes `Q_FUNC_INFO`), décompilation **ciblée** de 31 fonctions réseau.
- **Possible mais coûteux** : pseudo-C lisible d'une fonction donnée (structure OK ;
  paramètres/chaînes à compléter à la main — capstone comble ces trous).
- **PAS possible** : source recompilable de Nickel. Raisons PROUVÉES, pas supposées :
  - Frontières **ARM/Thumb** instables (58 886 fn Thumb + pools inline) → Ghidra gonfle
    des fonctions qui en avalent d'autres ; toute re-analyse globale re-casse (testé).
  - Paramètres/layout des structures non récupérables ; **ICF** (6 652 adresses partagées),
    templates Qt + Rust statique = bruit inextricable.
  - Couplage signaux/slots runtime (UI non saisissable) ; **verrou de version** (casse à chaque update).
- **Outil communautaire** : KoboPatch (pgaskin) = patchs déclaratifs chirurgicaux.

---

## ENDPOINTS RÉSEAU (DÉCOMPILÉS de libnickel — liste définitive)
> Inventaire complet (80 hosts) : `/home/maelle/fw/re/out/rodata_urls.txt`.
> Endpoints par fonction (capstone) : `/home/maelle/fw/re/out/endpoints.md`.

### Architecture : UN goulot unique
- **`NetworkAccessManager::createRequest()`** (`0xb41c01`, sous-classe de `QNetworkAccessManager`)
  = point de passage de **tout** le trafic. Piloté par `InternetProvider*`.
- **`WirelessWorkflowManager`** (`: InternetProvider`) fait le « phone-home » de connectivité
  à chaque association wifi : `performConnectivityTest` / `performConfigUrlCheck` →
  **`http://captive.apple.com/hotspot-detect.html`** (à blackholer en priorité).

### À NEUTRALISER (télémétrie / store) — paths exacts décompilés
- `ssl.google-analytics.com/collect` — GA (`GoogleAnalyticsRequester::apiRequest`) ; payload
  Measurement Protocol monté par `createQuery` (`tid`,`cid`,`uid`,`cd%1`,`cm%1`). File offline
  persistée (`addToQueue`/`saveQueue`/`pumpQueue`) ⇒ neutralisation à rendre **permanente**.
- `hb.afl.rakuten.co.jp/hgc/%1/?pc=%2&m=%2` — heartbeat affiliate Rakuten
- `mobile.kobobooks.com/mobileRequest.ashx` — reporting legacy
- `storeapi.kobo.com` · `readingservices.kobo.com` (sync progression = donnée comportementale)
- `oauth.kobo.com` / `oauthstage.kobo.com` — auth
- `api.kobobooks.com/1.0/UpgradeCheck/%1/%2/%3/%4/%5` — check firmware (en-tête `X-Kobo-Accept-Preview`)
- `ereaderfiles.kobo.com` — téléchargement firmware · `www.kobo.com` / `kobobooks.com`
- `api.ipinfodb.com/v3/ip-city/?key=…` (géoloc IP, **clé API embarquée**) · `worldtimeapi.org/api/ip`
- `graph.facebook.com` / `m.facebook.com/dialog/oauth` (partage social) · `getpocket.com` (Pocket)
- `management.mytolino.com` / `opensource.mytolino.com` (branche tolino)
- `ConfigurationRequest` XML (PlatformID/OS/DeviceModel/Affiliate) = fingerprint device

---

## PLAN D'EXÉCUTION

### Décision d'architecture UI : KOReader (recommandé)
- Nickel = binaire Qt fermé non recompilable → mauvais support pour « UI custom ».
- **KOReader** : open-source (Lua + C), recompilable, **OPDS natif**, UI entièrement
  modifiable. Répond directement à l'objectif.
- Deux variantes :
  - **B1 (recommandé pour démarrer)** : KOReader lancé via KFMon, Nickel reste
    installé mais réseau coupé. Réversible, peu risqué.
  - **B2 (défricheur)** : KOReader remplace Nickel au boot (hook init custom).
    Risqué (boot cassé → dépend de recovery). KSM déprécié, non supporté sur Qt6.
- **RISQUE À VÉRIFIER** : compat KFMon + KOReader sur firmware 5.1.x **Qt6**.
  La doc KOReader cible historiquement 4.x. Vérifier sur fil MobileRead
  « One-Click Install Packages » / issues KFMon pour le firmware exact AVANT install.

### Composants KOReader (méthode KFMon)
- KFMon via `KoboRoot.tgz` (watchdog qui lance KOReader sur fichier-déclencheur).
- KOReader extrait dans `.adds/koreader` (paquet `koreader-kobo-*.zip`).
- Empêcher Nickel de scanner les dossiers cachés — dans
  `.kobo/Kobo/Kobo eReader.conf` :
  ```
  [FeatureSettings]
  ExcludeSyncFolders=(\\.(?!kobo|adobe).+|([^.][^/]*/)+\\..+)
  ```

### Couche réseau (défense en profondeur — choix retenu)
Vit dans le rootfs (poussable via KoboRoot.tgz ou rootfs.img reflashée).

**Couche 1 — /etc/hosts (blackhole DNS)**
```
127.0.0.1   ssl.google-analytics.com
127.0.0.1   www.google-analytics.com
127.0.0.1   hb.afl.rakuten.co.jp
127.0.0.1   mobile.kobobooks.com
127.0.0.1   storeapi.kobo.com
127.0.0.1   readingservices.kobo.com
127.0.0.1   oauth.kobo.com
127.0.0.1   api.kobobooks.com
127.0.0.1   ereaderfiles.kobo.com
127.0.0.1   www.kobo.com
```

**Couche 2 — iptables DROP par défaut (rootfs embarque iptables complet)**
Script init (ex. /etc/init.d/opds-firewall) :
```sh
#!/bin/sh
OPDS_HOST="192.168.1.10"   # <-- IP serveur OPDS (À DÉFINIR)
OPDS_PORT="443"            # <-- port (À DÉFINIR)
iptables -F OUTPUT
iptables -P OUTPUT DROP
iptables -A OUTPUT -o lo -j ACCEPT
iptables -A OUTPUT -m state --state ESTABLISHED,RELATED -j ACCEPT
iptables -A OUTPUT -p udp --dport 53 -d 192.168.1.1 -j ACCEPT   # DNS local si OPDS par nom
iptables -A OUTPUT -p tcp -d "$OPDS_HOST" --dport "$OPDS_PORT" -j ACCEPT
```
À AJUSTER selon topologie réseau (LAN direct / Tailscale / domaine) — non encore fournie.

---

## QUESTIONS OUVERTES À TRANCHER
1. Topologie du serveur OPDS (LAN ? Tailscale ? domaine public ?) → règles iptables exactes.
2. B1 vs B2 (KOReader à côté de Nickel, ou à la place).
3. Confirmer compat KFMon/KOReader sur la version Qt6 exacte (5.1.x) avant toute install.

## NOTES
- Ne PAS découper le « store » du binaire Nickel (pas un exécutable séparé ; ce sont
  des écrans dans libnickel). Neutraliser par réseau, pas par chirurgie binaire.
- Alternative OS complet : InkBox (open-source Qt pour Kobo) — support Libra Colour /
  i.MX8 / Qt6 très incertain, projet lourd. À ne considérer que si indépendance totale
  prime sur stabilité.
- Contournement de DRM (module fucrl) : exclu, et sans rapport avec l'objectif OPDS.
