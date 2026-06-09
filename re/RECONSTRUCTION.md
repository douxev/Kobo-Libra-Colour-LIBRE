# Reconstruction C++ de libnickel.so — rapport de décompilation

Firmware : Kobo Libra Colour (N428 / kobo11 / monza), 5.1.186250, branche **tolino-qt6**.
Binaire : `usr/local/Kobo/libnickel.so.1.0.0` — ELF32 **ARM EABI5 hard-float**, stripped
(`.symtab` absente) mais `.dynsym` riche (82 803 symboles).
Tout est issu du binaire réel ; aucune source Kobo n'existe publiquement.

---

## 1. Outillage mis en place (reproductible)

| Outil | Rôle | Emplacement |
|---|---|---|
| venv `capstone`+`pyelftools` | désassemblage ARM/Thumb + ELF | `re/.revenv` (→ `/home/maelle/fw/.revenv`) |
| JDK 21 Temurin (portable, **sans sudo**) | requis par Ghidra 12 | `jdk-21.0.11+10/` |
| Ghidra 12.1.2 | décompilation pseudo-C | `ghidra_12.1.2_PUBLIC/` |
| Scripts `re/0*.py` | extraction symboles/RTTI/headers/endpoints | `re/` |
| `re/ghidra_scripts/DecompileTargets.java` | décompilation headless ciblée | idem |

Pipeline : `01_symbols.py` → `02_rtti.py` → `03_headers.py` → `05_funcinfo.py` →
`06_ancestry.py` → import/analyse Ghidra (`re/run_ghidra.sh`, ~25 min) →
`DecompileTargets.java` → `07_endpoints.py`. Sorties dans `re/out/`.

---

## 2. Ce qui a été reconstruit (et avec quelle fiabilité)

### 2.1 Cartographie symbolique — **fiable à 100 %**
- 62 394 fonctions définies (58 886 **Thumb** / 3 508 ARM), 17 043 objets, 3 360 imports.
- 3 215 vtables → **3 215 classes nommées**, 3 705 typeinfo (RTTI).
- Racines applicatives majeures : `ReadingView`, `TolinoEngine`, `ApplicationSettings`,
  `WebRequester`, `WirelessWorkflowManager`, `NetworkAccessManager`, `Volume`, `Device`…
- Fichier : `re/out/symbols.tsv`, `re/out/namespaces.tsv`.

### 2.2 Graphe d'héritage C++ (RTTI Itanium) — **fiable**
3 779 relations d'héritage extraites des structures `__si/__vmi_class_type_info`
via les relocations (`re/out/inheritance.tsv`). Exemples vérifiés :
```
NetworkAccessManager      : QNetworkAccessManager
WirelessWorkflowManager   : InternetProvider : QObject
WebRequester              : QObject
OneStoreServiceSettings   : Settings
WebEngineRenderer         : QWebEngineView, GestureReceiver, GestureDelegate,
                            KepubMarkupDelegate, HighlightDelegate, IDogEarDelegate, IReader
```
530 classes à héritage multiple, 2 555 à héritage simple.

### 2.3 Headers C++ (interfaces) — **fiable pour les signatures, partiel pour les types de retour**
21 classes reconstruites dans `re/out/headers/*.hpp` avec :
- base(s) correcte(s) (depuis le RTTI),
- **table virtuelle dans l'ordre exact des slots** (on distingue les virtuals
  surchargés des hérités, ex. `OneStoreServiceSettings` réutilise `Settings::saveSetting`),
- signatures complètes des méthodes (depuis le mangling),
- **types de retour** récupérés pour 98 méthodes via 828 chaînes `Q_FUNC_INFO`
  embarquées dans `.rodata` (le mangling Itanium n'encode PAS le type de retour).

Limites : pas de **layout mémoire** (offsets/types des membres données — non encodés
nulle part) ; types de retour manquants pour les méthodes sans chaîne `Q_FUNC_INFO`.

### 2.4 Décompilation ciblée — **structure fiable, détail variable**
31 fonctions réseau/télémétrie visées → **29 pseudo-C Ghidra propres** (`re/out/decomp/`)
+ **2 via capstone** (gonflées sous Ghidra). Voir §4 pour les limites de qualité.

---

## 3. Verdict de faisabilité (réponse directe : « peut-on reconstruire le C++ ? »)

| Niveau | Faisable ? | Détail |
|---|---|---|
| Interfaces (classes, héritage, méthodes, vtables) | ✅ **Oui** | démontré ci-dessus, à grande échelle |
| Logique d'une fonction donnée (pseudo-C lisible) | 🟡 **En partie** | OK pour les fonctions à frontière propre ; coûteux à nettoyer méthode par méthode |
| **Source recompilable** de Nickel | ❌ **Non** | et ce n'est pas qu'« il faut du temps » — raisons structurelles ci-dessous |

Pourquoi pas de source recompilable :
1. **Frontières ARM/Thumb instables.** 58 886 fonctions Thumb mêlées à 3 508 ARM +
   pools de constantes inline ⇒ l'auto-analyse Ghidra crée des fonctions aux
   **frontières gonflées** qui en avalent d'autres (erreurs `function body repair
   overlap`). Il a fallu, pour chaque cible, supprimer la fonction parasite, forcer
   `TMode=Thumb`, redésassembler la plage exacte (taille connue via `.dynsym`), puis
   garde-fou anti-gonflement. Non automatisable proprement sur 62 000 fonctions.
2. **Paramètres non récupérés** après re-création de fonction (le décompileur affiche
   `param_N`, `unaff_rN`, `in_stack_NNNN`) ; toute re-analyse globale **re-casse** les
   frontières (testé : `analyzeChanges` regonfle `internetIsAccessible` de 20 o → 206).
3. **Pas de layout des structures** : impossible de régénérer les `class { ... membres }`.
4. **Templates Qt massifs** (QList/QMap/QString inline partout), **ICF** (6 652 adresses
   partagées par ≥2 symboles), **Rust statique** (regex, serde, smartstring…) — du bruit
   inextricable mêlé au code applicatif.
5. **Couplage signaux/slots Qt** résolu au runtime : l'« UI » n'est pas un module saisissable.
6. **Verrou de version** : tout résultat est lié à 5.1.186250 et casse à chaque update.

➡️ Conclusion pratique inchangée vs analyse initiale, mais **précisée et prouvée** :
le reverse **ciblé** (localiser/comprendre/patcher une fonction nommée) est faisable et
utile ; la **réécriture de Nickel** ne l'est pas. Pour une UI custom → KOReader (cf.
plan principal), pas la recompilation de Nickel.

---

## 4. Qualité réelle de la décompilation (honnête)

Ce que le pseudo-C Ghidra donne **bien** : le **flot de contrôle** et la **structure**
des fonctions bien bornées (ex. `createRequest` 4 596 o → 277 lignes proportionnées,
`execute` 4 900 o → 439 lignes, `createQuery` 3 246 o → 317 lignes).

Ce qu'il donne **mal** sur ce binaire : les **types de paramètres** (souvent `(void)`
+ `in_stack_`) et la **résolution des chaînes/endpoints** (références non rétablies après
la chirurgie de frontières).

➡️ **capstone comble exactement ces deux trous** (vérité terrain). Exemple — la même
fonction `internetIsAccessible()` (20 o), illisible sous Ghidra (corps gonflé), est
limpide en désassemblage annoté :
```
push {r4,lr}; blx getProvider; cbz r0, ret0      // pas de provider → false
ldr r3,[r0]; pop {r4,lr}; ldr r3,[r3,#0x30]; bx r3 // sinon tail-call virtuel slot 0x30
ret0: pop {r4,pc}                                  // return false
  ≡  return provider ? provider->vmethod_0x30() : false;
```
Les endpoints (§5) viennent tous de capstone, pas de Ghidra.

---

## 5. Intelligence réseau extraite (le payoff pour l'objectif hors-ligne)

### 5.1 Architecture : un goulot unique
- **`NetworkAccessManager : QNetworkAccessManager`** surcharge `createRequest()`
  (`0xb41c01`) → **tout** GET/POST/PUT y transite. Piloté par un `InternetProvider*`
  (`setInternetProvider`).
- **`WirelessWorkflowManager : InternetProvider`** gère l'auto-connexion et le
  « phone-home » de connectivité : `performConnectivityTest()`, `performConfigUrlCheck()`,
  `attemptSilentConnection()`, `isInternetAccessible()`.

### 5.2 Endpoints en clair (décompilés, par fonction) — `re/out/endpoints.md`
| Fonction | Endpoint / header |
|---|---|
| `GoogleAnalyticsRequester::apiRequest` | `https://ssl.google-analytics.com/collect` |
| `GoogleAnalyticsHandler::createQuery` | payload GA Measurement Protocol : `tid`,`cid`,`uid`,`cd%1`,`cm%1`,`%1x%2` |
| `WebRequester::getCurrentCity` | `https://api.ipinfodb.com/v3/ip-city/?key=…` (clé API embarquée) |
| `WebRequester::getTimeZone` | `https://worldtimeapi.org/api/ip` |
| `UpgradeCheckCommand::execute` | en-tête `X-Kobo-Accept-Preview` (+ `api.kobobooks.com/1.0/UpgradeCheck/…`) |
| `NetworkAccessManager::createRequest` | en-têtes `x-ak-clientip`, refs `m.facebook.com` |

### 5.3 Inventaire complet `.rodata` (80 hosts/URLs) — `re/out/rodata_urls.txt`, extraits :
```
http://captive.apple.com/hotspot-detect.html        <- check de connectivité (à neutraliser)
https://api.kobobooks.com/1.0/UpgradeCheck/%1/%2/%3/%4/%5
https://ssl.google-analytics.com/collect            <- télémétrie GA
http://hb.afl.rakuten.co.jp/hgc/%1/?pc=%2&m=%2       <- heartbeat affilié Rakuten
http://mobile.kobobooks.com/mobileRequest.ashx
https://storeapi.kobo.com   https://readingservices.kobo.com
https://oauth.kobo.com   https://oauthstage.kobo.com   https://ereaderfiles.kobo.com
https://management.mytolino.com   https://opensource.mytolino.com
https://graph.facebook.com/me…   https://m.facebook.com/dialog/oauth…   (partage social)
https://getpocket.com…  https://text.getpocket.com/v3beta/text          (Pocket)
```

### 5.4 Pipeline de télémétrie reconstruit
`AnalyticsEvent` → `AnalyticsEventManager::save()` (stockage **SQLite** local, table
analytics) → `GoogleAnalyticsRequester::{addToQueue,saveQueue,loadQueue,pumpQueue}`
(**file d'attente offline persistée**) → `apiRequest()` POST vers `…/collect`.
La file offline signifie que couper le réseau *accumule* puis *flushe* à la reconnexion :
la neutralisation réseau doit être **permanente**, pas intermittente.

---

## 6. Leviers concrets pour l'indépendance réseau (plus chirurgicaux que DNS/iptables)

1. **`NetworkAccessManager::createRequest`** = point unique : un patch binaire y forçant
   un retour d'erreur pour tout host ≠ OPDS couperait *tout* le trafic d'un seul endroit.
   (Patch lié à la version ; KoboPatch est l'outil idoine pour le déclaratif.)
2. **Check de connectivité** `captive.apple.com/hotspot-detect.html` + `performConfigUrlCheck`
   → à blackholer pour éviter le « phone-home » à chaque association wifi.
3. **Défense en profondeur conservée** : la couche `/etc/hosts` + iptables du plan
   principal reste la voie **sans patch binaire** (réversible, non liée à la version) — à
   privilégier. Le reverse ci-dessus confirme/complète la liste d'hôtes à blackholer.

---

## 7. Fichiers livrés dans `re/out/`
- `symbols.tsv` (82 803 symboles démanglés), `namespaces.tsv`, `classes.txt`, `vtables.tsv`
- `inheritance.tsv` (3 779 arêtes), `ancestry.txt`
- `headers/*.hpp` (21 classes reconstruites), `methods_by_class.tsv`, `funcinfo.tsv`
- `decomp/*.c` (29 fonctions pseudo-C Ghidra)
- `endpoints.md` (endpoints+appels par fonction), `rodata_urls.txt` (inventaire global)
