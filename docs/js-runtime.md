# Runtime JavaScript : Wasm, format des résultats, CMaps, tests

Comment le cœur Rust (`crates/`) est servi à JavaScript : la crate
`crates/wasm` (ABI, encodage des résultats, erreurs, diagnostics, CMaps) et le
paquet `js/` (chargement, API publique, mode worker). L'API publique est
décrite dans [`public-api.md`](public-api.md), les règles générales de la
frontière dans [`architecture.md`](architecture.md) (« Frontière
JavaScript »).

## Vue d'ensemble

```
JS (js/src)                                    Rust (crates/wasm)
open(input) ── platform.readInput ─► octets
            ── core.call("doc_open", …, data) ─► pp_alloc ×2, pp_call ─► ops::Core::call
                                                   requête PJB1 [op, handle, args, niveau]
            ◄─ decodeResponse ◄── réponse PJR1 (statut, charge PJB1|JSON, diagnostics)
statut 2 (CMaps manquantes) : fetch/fs des fichiers, "cmap_add", même requête relancée
```

- **Pas de `wasm-bindgen`** : le module n'importe rien et exporte `memory`,
  `pp_alloc(len) -> ptr`, `pp_free(ptr)`, `pp_call(req, data) -> statut`,
  `pp_result_ptr()`, `pp_result_len()`, `pp_result_free()`, `pp_buffers()`
  (`crates/wasm/src/abi.rs`). Les noms sont préfixés pour qu'un build natif
  n'exporte pas un `free` qui remplacerait celui de la libc.
- **Mémoire** : `pp_alloc` crée directement les octets partagés que le
  cœur garde (`Rc<[u8]>`, ce que lit `pycompat::io::BytesIO`), dans une
  table indexée par leur adresse ; JS y écrit, puis `pp_call` *prend* le
  tampon : le PDF est une fois dans la mémoire du module (mesuré : 200 Mio
  ouverts font grandir la mémoire de 200 Mio ; une copie la doublerait, et la
  mémoire linéaire ne rétrécit jamais). La réponse reste dans un
  tampon de Rust jusqu'à l'appel suivant ; JS la décode en place puis appelle
  `pp_result_free`. Aucun bloc `unsafe` : JS lit et écrit la mémoire, Rust ne
  donne que des adresses de tampons qu'il possède. La crate a ses propres
  lints (`unsafe_code = "deny"` au lieu du `forbid` de l'espace de travail)
  pour que seul `abi.rs` autorise les `#[no_mangle]`, que le lint couvre.
- **Handles** (`crates/wasm/src/handles.rs`) : documents, pages et pages
  dérivées, tables, text maps. Un handle `u32` = index (20 bits) + génération (12 bits) : un
  handle libéré puis réutilisé est refusé (`ValueError("invalid handle")`).
- **Requêtes** : `crates/wasm/src/ops.rs` ; une requête est la liste PJB1
  `[op, handle, args, niveau]` plus un tampon de données optionnel. La
  réponse : en-tête `PJR1` (statut 0 ok / 1 exception / 2 CMaps manquantes,
  format de la charge 0 PJB1 / 1 JSON, drapeau de correction JSON), charge,
  bloc des diagnostics, chacun aligné sur 8 octets.
- **Panique** : le profil `dist` a `panic = "abort"` ; une panique (un bogue)
  devient un `unreachable` Wasm. Toute exception qui sort des exports (ce
  piège, un dépassement de pile : `RangeError` dans V8 et JavaScriptCore,
  `InternalError` dans SpiderMonkey, une allocation impossible) laisse l'état
  de Rust indéfini (un `RefCell` emprunté pour toujours) : JS lève
  `CoreCrash`, marque l'instance comme morte, et le prochain `open()`
  instancie un nouveau cœur. Un PDF trop grand pour la mémoire de wasm32
  (4 Gio) finit ainsi, là où Python lèverait `MemoryError` (pas encore de
  chemin récupérable).
- **Profondeur** : le cœur lit, écrit et libère les valeurs sans récursion
  (`PyValue` se libère avec une pile de travail, comme la « trashcan » de
  CPython ; `PDFObject` de même) : une liste imbriquée sur 200 000 niveaux
  traverse la frontière dans les deux sens. L'interpréteur, lui, récurse en
  Rust là où Python récurse (Form XObjects imbriquées), jusqu'à la limite
  de Python : la pile du module est de 8 Mio (`crates/wasm/build.rs` ; le
  défaut de `wasm-ld`, 1 Mio, ne suffit pas au profil `dist`, voir
  `architecture.md`, « Profondeur de récursion »).

## Format des résultats

Deux encodages ont été implémentés et mesurés (`crates/wasm/src/wire/`) :

- **PJB1** (binaire) : valeurs étiquetées (petits entiers en `i32`, entiers
  ≤ 2^53 en `f64`, au-delà en décimal → `BigInt`, flottants sur leurs 8 octets
  IEEE : NaN, ±inf, -0.0 passent tels quels), une **table de chaînes** (chaque
  `str` une fois ; les chaînes sans surrogate forment un seul blob UTF-8 que
  `TextDecoder` décode d'un coup, découpé par leurs fins en unités UTF-16 ;
  les chaînes à surrogate isolé sont stockées en UTF-16), et des **tables** :
  une liste de dicts (les objets d'une page) est écrite par colonnes, groupée
  par séquence de clés (64 formes au plus, une forme pour deux lignes au plus),
  chaque colonne typée quand toutes ses cellules le permettent (`f64`,
  `f64`-ou-`None`, indices de chaînes, booléens, `None`, séquences de nombres
  comme `matrix` et les couleurs, sinon valeurs génériques). JS construit les
  lignes d'une forme avec une fonction générée (`new Function`, littéral
  objet aux clés dans l'ordre ; repli sans génération de code si une CSP
  l'interdit, testé), environ six fois plus vite que des affectations par
  clé dans V8.
- **JSON** : JSON ordinaire (nombres au plus court, relus exactement par
  `JSON.parse`, `-0` compris) ; ce que JSON n'a pas (NaN, ±inf, grands
  entiers, `bytes`, objets pdfminer) devient un objet à clé unique `"\u0000"`,
  et une vraie clé qui commence par U+0000 reçoit un U+0000 de plus ; un
  drapeau de la réponse dit si JS doit parcourir la valeur pour défaire ces
  échappements.

Les deux donnent les mêmes objets JS (vérifié sur tout le corpus de mesure
et par les tests aléatoires de `crates/wasm/tests/boundary.rs`).

- **Typé** (vague 5, base ; format `2` de la réponse, `set_format("typed")`) :
  PJB1 sans tables (`BinWriter::without_tables`), où chaque valeur garde
  son étiquette : `int` (`I32`, `INT_F64`, `BIGINT`) et `float` (`F64`),
  `list` et `tuple`, l'ordre des clés d'un `dict`. JS le décode en valeurs
  typées (`decodeBinary(.., true)`, `js/src/core/typed.ts` : `PyFloat`,
  `PyTuple`, `PyDict`) et rend à l'API leur projection (`project`), les
  valeurs publiques ; la valeur typée du dernier résultat est
  `core.lastTyped`. C'est le format du harnais qui écrit les traces des
  étapes par l'API (`js/trace/`), jamais celui de l'API.
- **Flux par référence** (vague 5, BOUNDARY, protocole 1 : une sorte de
  valeur ajoutée) : PJB1 `0x13` (`TAG_STREAM_REF`), JSON `"streamref"`,
  `Writer::stream_ref` : le handle du flux, puis `objid`, `genno`,
  `attrs` ; ses octets se lisent à la demande (« Documents et pages »,
  flux). Un flux s'écrit par référence quand le cœur lui a donné un handle
  (`set_stream_refs`, que le runtime JS demande), par valeur sinon
  (`0x0f` : `rawdata` et `data` copiés) ; le format typé du harnais écrit
  toujours par valeur. Les décodeurs JS font le `PDFStream` par le
  « fabricant » du cœur qui décode (`withStreamRefs`, `StreamRefs`).
- **Drapeaux de la réponse** : bit 0, le JSON demande la passe de
  correction ; bit 1 (`FLAG_HISTORY_LONG`), l'historique des documents
  dépasse sa limite (« Documents et pages », historique borné).

### Mesures

Corpus : les `chars` et `edges` des 215 pages réelles de
`oracle/unit/text_tables_real.py` (229 555 caractères, 39 236 bords, objets
de pdfplumber), chargés dans le cœur par `crates/wasm/examples/bench_corpus.rs`.
Une passe encode chaque page (un appel par page, le cas typique) ; « cœur » =
appel Wasm (encodage Rust, enveloppe, copie), « décodage » = construction des
objets JS. Médianes de 5 passes, en ms, `dist/pdfplumber.wasm` optimisé
(`wasm-opt -O3`). La machine est partagée (8 cœurs, d'autres projets
tournent) : les écarts d'un run à l'autre atteignent ±30 %, les rapports entre
formats sont stables.

Un appel par page (215 appels) :

| Moteur | PJB1 cœur | PJB1 décodage | **PJB1 total** | JSON cœur | JSON décodage | **JSON total** |
|---|---|---|---|---|---|---|
| Node 26.11 | 579 | 394 | **973** | 2 712 | 2 575 | **5 287** |
| Chromium 148 | 529 | 498 | **1 027** | 1 981 | 1 214 | **3 195** |
| Firefox 158 (système) | 939 | 553 | **1 492** | 2 408 | 1 836 | **4 244** |
| Firefox 150 (Nightly de Playwright) | 1 622 | 1 269 | **2 891** | 15 786 | 5 632 | **21 418** |
| WebKit 26.4 | 1 113 | 509 | **1 622** | 3 909 | 3 006 | **6 915** |

Tout le corpus en un appel (total) : PJB1 1 425 / 1 026 / 1 557 / 2 517 /
1 939 ms, JSON 5 985 / 3 186 / 4 679 / 19 069 / 5 798 ms (même ordre).
Chaque moteur a décodé les 215 pages en objets identiques dans les deux
formats. Le « cœur » du JSON est surtout l'écriture des flottants au plus
court ; son décodage, `JSON.parse` des clés répétées de chaque objet.

Taille des charges : binaire 52,2 Mo, JSON 122,5 Mo (≈ 194 et 456 octets
par objet).

**Décision : PJB1.** Il est 2,8 à 7 fois plus rapide de bout en bout selon
le moteur (3 à 5 fois hors Nightly), 2,3 fois plus petit, et son décodeur est itératif (une valeur
imbriquée sur 10^5 niveaux passe partout). Le JSON reste disponible
(`set_format`, benchmark, débogage) et testé.

Vague 5, chantier PERF (`docs/architecture.md`, « Chantier PERF :
réalisation ») : l'écriture des tables lit les cellules une fois, rangée
par rangée, écrit les colonnes de nombres et d'indices en place et
retrouve la `str` d'une cellule par son adresse ; le décodeur lit les
cellules génériques peu profondes sans les cadres de `value()`. Mêmes
octets qu'avant (empreinte des blocs de `crates/wasm/examples/encode_phases.rs`
sur `quick`). Même corpus, Node 26.12, médianes de 5 passes, `wave5/base` →
`wave5/perf` dans le même créneau : PJB1 un appel par page 602 → 425 ms
(cœur 365 → 236, décodage 236 → 189), tout en un appel 820 → 577 ms ; JSON
2 847 → 2 786 ms (inchangé). Les phases de pdfplumber par l'API publique
dans les navigateurs se mesurent avec `js/bench/browser-phases.mjs`.

Reproduire :

```sh
cargo run --release -p pdfplumber-wasm --example bench_corpus -- $PDFPLUMBER_ROOT/.cache/bench/pages_real.pjb
node scripts/build.mjs
node js/bench/node.mjs --passes 5
node js/bench/browsers.mjs --passes 5   # Chromium, WebKit, Firefox de Playwright, Firefox du système
```

## Valeurs

| Python | JavaScript |
|---|---|
| `None`, `bool` | `null`, booléen |
| `int` | `number` si \|n\| ≤ 2^53, sinon `bigint` |
| `float` | `number` (NaN, ±Infinity, -0 compris) ; `3` et `3.0` donnent le même nombre |
| `str` | chaîne ; un surrogate isolé est conservé (voir ci-dessous) |
| `bytes` | `Uint8Array` |
| `list`, `tuple` | `Array` |
| `dict` | objet ordinaire, clés dans l'ordre Python (sauf les clés « entières », que JS met en tête) ; `__proto__` est une propriété propre |
| `PSLiteral`, `PSKeyword`, `PDFObjRef` | classes `PSLiteral`, `PSKeyword`, `PDFObjRef` |
| `PDFStream` | `PDFStream` par référence (`objid`, `genno`, `attrs` ; `rawdata`, `data`, `getData()`, `getRawdata()` lus à la demande), ou instantané (format typé, `new PDFStream(...)`) |
| `Page` de pdfplumber (`annot["data"]["P"]`, `getattr(pdf, "pages")`) | `PageRef` (`handle`, `page_number`) |

Une `Page` dans un résultat est écrite comme une sorte à elle (PJB1
`0x12`, JSON `"page"`) : le handle de la page, puis son `page_number`.
L'opération donne ce handle (`Core::label_pages`, avant l'écriture) : le
premier handle vivant de cette page, sinon un nouveau, que l'opération
crée (un rejeu le refait), dans l'ordre où la valeur s'écrit ;
`oracle/unit/js_pages_run.py` nomme la page de même (le premier objet de
la session qui est cette page et n'a pas été libéré).

Arguments (JS → Rust, PJB1 sans tables) : entier sûr → `int`, autre nombre
→ `float`, `bigint` → `int`, `PyFloat(x)` force un `float`, `PyTuple([...])`
un `tuple`, tableau → `list`, objet ou `Map` à clés chaînes → `dict`
(propriétés `undefined` omises), `Uint8Array`/`ArrayBuffer` → `bytes`.
Les objets d'un résultat rendus en argument redeviennent les objets
qu'ils représentent (vague 5, `binary::decode_args`, `Core::object_arg`) :
un `PSLiteral` ou un `PSKeyword` est interné (`LIT`, `KWD` : le même objet
que pdfminer a fait), un `PDFObjRef` est un nouveau `PDFObjRef` lié au
document de la cible de la requête (le décodage est refait une fois
celle-ci connue ; pdfminer n'en compare que l'identité, que rien ne lit),
un `PageRef` est la page de son handle, un `PDFStream` par référence le
flux de son handle (le même objet) ; un `PDFStream` instantané n'est pas
un argument (`TypeError`). `oracle/unit/js_pages_run.py` fait de même
(`bind_args` : `{"$obj": k}` pour un objet de la session).
L'encodeur (`encodeValue`) n'est pas récursif : toute profondeur passe ; un
conteneur qui se contient lui-même est une `TypeError` (le même objet deux
fois ne l'est pas).

Les fonctions qui construisent les lignes d'une table sont gardées par
forme, sous la clé `JSON.stringify([types, clés])`, sans ambiguïté quelles
que soient les clés (U+0000 compris).

**Chaînes et surrogates.** Une `str` Python peut contenir un surrogate
isolé (`chr(0xd800)`), qu'une `String` Rust ne peut pas porter. L'écriture
des résultats accepte une `str` sous toutes les formes qu'un module porté
peut garder (`strings::PyStrRef` : `&str`, UTF-8 généralisé = le
`surrogatepass` de Python, unités UTF-16, points de code) et l'envoie en
unités UTF-16 ; testé avec des tampons faits à la main, dans tous les
moteurs. Seule perte, côté JS : deux points de code « surrogate haut, puis
bas » deviennent le caractère astral qu'ils codent. Dans l'autre sens, JS
encode en UTF-8 généralisé, que le cœur lit en `pycompat::PyStr` (la `str`
du modèle d'objets) : une `str` d'argument à surrogate isolé, valeur ou clé
de dict, arrive telle quelle (aller-retour testé en Rust et en JS). Le
message d'une exception est `str(e)` (`PyErr::py_message`), surrogates
compris.

## Erreurs

Une exception (`pycompat::PyErr`) arrive avec son nom de classe, son module,
son `__qualname__`, son MRO (linéarisation C3 des bases d'`ExcKind`, avec
`builtins.object`) et `str(e)` ; JS en fait un `PDFPlumberError` dont `name`
est le nom Python (`errors.ts`). Le MRO de chacune des 71 classes d'`ExcKind`
est comparé à celui de CPython (`oracle/unit/js_runtime.py`,
`exceptions.jsonl`), en Rust et dans chaque moteur. `NotPorted` (un chemin
non porté) devient l'erreur JS `NotImplemented` (qui garde module, classe
et MRO). L'exception qu'enveloppe une exception de pdfplumber
(`PdfminerException(e)`, `MalformedPDFException(e)` : `PyErr::cause`)
arrive sous la clé `cause`, au même format (imbriqué ; `None` sans
cause) : c'est l'`error.cause` du `PDFPlumberError`.

## Diagnostics

`pycompat::diagnostics` est le canal des `logging` et des `warnings.warn`
: un module porté appelle `diagnostics::emit(source, niveau, || message)`
(le message n'est formaté que si quelqu'un écoute). Transmis : les deux
`logger.warning` de pdfplumber (métadonnées, classe d'attributs
inconnue), son `warnings.warn` (`Page.annots`), celui-ci par
`pycompat::warnings`, qui suit les filtres par défaut de Python : montré
une fois par texte, catégorie et ligne dans le processus (le registre est
de l'état du processus : un appel nul le remet), et (vague 5, BOUNDARY,
protocole 2) chaque `log.warning` de pdfminer (`pdfinterp`, `pdfdevice`,
`pdffont`, `cmapdb`, `pdfpage`, `pdfdocument`, `pdftypes`) :
`pycompat::logging::Logger::log` prend les cadres de l'appel comme avant
(logging coupé : la référence, qui décide où tombe une `RecursionError`),
puis donne le texte de l'enregistrement, `msg % args` comme
`record.getMessage()` le formate (`%r` des opérandes par le `repr` porté,
`pdfminer::log_text`), seulement si quelqu'un le collecte ; un texte dont
le formatage lève n'est pas émis (`logging` jette l'enregistrement). Le
texte ne change aucun résultat ni aucune récursion : le rejeu de
`pages.jsonl` passe une fois sans collecte et une fois en collectant tout
(niveau `debug`, fenêtres de profondeur comprises ; en natif le second
format, sous WASI le binaire deux fois). Les `log.debug` de pdfminer (un par jeton,
par opérateur, par objet lu) ne comptent que leurs cadres : ils ne sont
pas transmis (`logLevel: "debug"` ne montre que les niveaux `info` et
plus de pdfminer, aucun aujourd'hui). `diagnostics.jsonl` de
`oracle/unit/js_runtime.py` (ce que Python montre, avertissements et
enregistrements des loggers de pdfplumber et de pdfminer au niveau
`WARNING`, dans un processus neuf, pour des documents faits pour produire
chaque sorte d'enregistrement : opérandes invalides et étiquettes
valides, qui n'écrivent rien, boîtes de page, y compris celles dont
l'analyse lève une autre erreur que `PDFValueError`, que pdfminer laisse
passer, polices, table xref, flux Flate corrompu) est rejoué par
`js_runtime_unit.rs` et, par l'API publique (`onWarning`), par
`js/test/shared/tests.mjs`. La frontière exécute chaque requête dans un
`diagnostics::collect` au niveau demandé par JS (0 : rien n'est collecté ni
formaté), au plus 1 000 entrées par appel (les autres sont comptées), et les
renvoie avec la réponse, y compris quand l'appel lève. Un appel nul (CMaps
manquantes, ci-dessous) ne renvoie aucun diagnostic : sa relance émet ceux
de Python, une fois. JS les passe à
`onWarning` (`api.ts`, `diagnosticsPolicy`) avec le nom du niveau et, plus
tard, le numéro de page. Le coût en cadres Python d'un appel de log (qui
décide où tombe une `RecursionError`) reste l'affaire du module qui porte
l'appel.

## CMaps

Les CMaps prédéfinies de pdfminer (`pdfminer/cmap/*.json.gz`, 7,9 Mo) ne sont
pas dans le Wasm. `oracle/codegen/cmaps.py` les convertit au format compact
lu par `cmapdb::CMapModule::parse` (148 fichiers, 3,9 Mo) ;
`scripts/build-cmaps.mjs` les compresse en zlib dans
`dist/cmaps/<nom>.pdfcmap.zlib` (3,0 Mo en tout ; un document japonais
charge `UniJIS-UCS2-H` + `to-unicode-Adobe-Japan1` : 156 Ko) et écrit la liste
des noms dans `js/src/generated/cmap-names.ts` (versionné ; `--check-names`
dans `check.sh`).

- **zlib plutôt que gzip ou brotli** : le cœur inflate lui-même les fichiers
  avec son port de zlib (`pycompat::zlib`), donc tous les environnements
  décodent avec le même code, sans `DecompressionStream`. Brotli ferait
  2,1 Mo au lieu de 3,0 Mo, mais demanderait un décodeur brotli dans le cœur
  ou un `DecompressionStream("brotli")` que tous les navigateurs n'ont pas.
  Un fichier déjà décompressé (serveur qui applique son propre
  `Content-Encoding`) est accepté tel quel.
- **Chargement paresseux et relance déterministe** (`crates/wasm/src/cmaps.rs`,
  `WasmCore.call`) : JS donne au cœur la liste des noms qui existent
  (`cmap_configure`). Quand le cœur demande une CMap connue mais pas encore
  fournie, le fournisseur la note et répond « absente », comme une
  installation sans ce fichier : le code porté continue (pdfminer remplace
  une CMap introuvable par un `CMap()` vide, que le cache des polices du
  document garde). L'appel est alors nul (`ops.rs`, « Void calls ») : la
  frontière jette son résultat et ses diagnostics, retire les handles qu'il
  a créés et ses appels de l'historique, remet l'état modélisé du
  processus Python tel qu'il était avant l'appel (`pdfminer::process_state`
  : noms internés, motifs `re` compilés, indices de `typing` en cache,
  caches des loggers, sites d'appel et de `LOAD_ATTR` spécialisés, caches
  de `CMapDB` ; sans quoi la relance trouverait, par exemple, compilé à la
  page 2 de l'appel nul un motif que l'exécution unique de Python compile
  plus profond à la page 1, et une `RecursionError` disparaîtrait), et
  remet chaque document que l'appel a atteint dans l'état où il était
  avant lui (ci-dessous, « Documents et pages ») ; puis il répond « CMaps
  manquantes » avec les noms. JS les récupère (`fetch` dans le navigateur et
  le worker, `fs` sous Node, ou la fonction `cmaps.load` de `configure`), les
  passe au cœur (`cmap_add`) et relance la même requête, qui voit les CMaps
  dès le début : son résultat est celui de Python avec toutes les CMaps
  installées. Testé avec une requête (`doc_cmap_decode`) qui atteint son
  document par un argument, y met la CMap en cache comme une police, et la
  manque : le résultat est celui de pdfminer (`cmaps.jsonl`), aucun handle
  ne reste, et un avertissement émis avant le manque arrive une fois ; une
  CMap chargée par l'appel nul avant le manque n'est plus dans les caches
  de `CMapDB` après lui (`a_void_call_puts_the_process_state_back`), et
  `crates/pdftrace/tests/lazy_cmap.rs` vérifie la relance à travers
  l'interpréteur, à la frontière de récursion. Chaque nom est demandé à JS
  une fois (le fournisseur garde les fichiers reçus ; le cœur relit depuis
  eux ce que la remise de l'état a retiré des caches) ; un nom demandé deux
  fois est une erreur (boucle).
  Un nom hors de la liste est `CMapNotFound` sans requête réseau. Avec
  `cmaps: false`, toutes les CMaps sont absentes, comme une installation de
  pdfminer sans son dossier `cmap`.
- **Préchargement au premier manque** (vague 5,
  `crates/wasm/src/cmap_scan.rs`) : le premier appel nul d'un document
  demande aussi toutes les CMaps prédéfinies que ses polices nomment :
  le cœur relit le document à part (un nouveau `PDFDocument` des mêmes
  octets et du même mot de passe, son arbre de pages et les polices des
  ressources de chaque nœud ; aucun flux de contenu ni XObject, dont les
  données seraient copiées pour rien) et ajoute aux noms manquants
  l'`/Encoding` de chaque police `Type0` (son `/Subtype`, comme
  `PDFResourceManager.get_font` le lit) et la carte unicode
  `to-unicode-<Registry>-<Ordering>` du `/CIDSystemInfo` de chacune de ses
  descendantes (chaînes Latin-1 sans espaces autour, comme `PDFCIDFont`), parmi celles que JS peut fournir et n'a pas
  données. Un appel nul par document au lieu d'un par CMap ; un document
  qui n'en manque aucune ne coûte rien (mesuré sur
  `pdfjs-pdf-reference-1-7.pdf`, 1 310 pages : une analyse à l'ouverture
  coûtait 243 ms pour une ouverture de 320 ms, d'où ce choix). Sans effet
  Python : l'état du processus est remis après l'analyse, ses diagnostics
  sont jetés, ses erreurs l'arrêtent avec ce qu'elle a trouvé, elle
  n'entre pas dans l'historique et ne charge aucune CMap ; une CMap
  présente dès le départ est Python avec toutes ses CMaps : résultats et
  récursion inchangés. C'est une estimation : une CMap qu'elle manque (une
  police d'un Form XObject) est chargée par un appel nul suivant, une de
  trop l'est pour rien. Testé :
  `the_first_void_call_of_a_document_asks_for_every_cmap_its_fonts_name`
  et `tests.mjs` (un seul appel nul par document CJK, `WasmCore.voidCalls`).
- Alternative écartée pour l'instant : un `PyErr` hors hiérarchie Python
  qui interromprait l'appel au lieu de le laisser finir. Elle éviterait de
  jeter un calcul, mais demande de changer le trait `CMapProvider` (autre
  chantier) et de prouver qu'aucun état partiel ne survit ; la relance est
  sûre par construction.
- Les cas `cmaps.jsonl` de `oracle/unit/js_runtime.py` (décodage et
  `get_unichr` sur dix CMaps et quatre collections) passent par ce protocole
  en Rust (`crates/wasm/tests/js_runtime_unit.rs`, fichiers zlib et
  décompressés) et dans chaque moteur. `documents.jsonl` garde deux documents
  CJK (`noembed-sjis.pdf`, `pr-136-example.pdf`) avec le texte de pdfplumber
  et les CMaps chargées : le test JS les ouvre, prend `pdf.pages` et le
  texte de la page par les opérations de la frontière, et compare le texte
  et les fichiers demandés (chacun une fois) ; la session `cjk` de
  `pages.jsonl` (ci-dessous) les rejoue avec des pages dérivées.

## Documents et pages

`crates/wasm/src/pages.rs` (vague 4, jonction) : le `PDF` de pdfplumber
et ses pages derrière des handles.

- **Document** : `doc_open` (les octets et les mots-clés de
  `pdfplumber.open`) exécute `pdfplumber.open(io.BytesIO(data),
  **options)` : les erreurs d'ouverture sont celles de Python (mot de passe,
  `/Root` absent, mot-clé inconnu), et le handle garde le `PDF`
  (`Document::state`), son `PDFResourceManager` (le cache des polices du
  document, avec leurs CMaps) et ses pages. Opérations : `doc_metadata`,
  `doc_pages` (un handle par page de `pdf.pages`, avec sa géométrie :
  `page_number`, `initial_doctop`, `rotation`, les boîtes, `bbox`,
  `width`, `height`), `doc_objects`, `doc_attr` (`getattr(pdf, nom)` :
  listes par sorte, arêtes, `pages` ; les noms que construisent `to_dict`
  et `to_csv`, `t + "s"` : un nom qui ne finit pas par `s` lève
  `NotPorted`), `doc_annots`, `doc_hyperlinks`,
  `doc_structure_tree`, `doc_to_dict`/`to_json`/`to_csv`, `doc_close`
  (`PDF.close()`, sans libérer le handle : `free` le fait).
  `doc_open_file` (`[path, options]`, vague 5) exécute
  `pdfplumber.open(path, **options)` sur les octets que JS a lus
  (`Source::Path` : le `PDF` possède le fichier, `close()` le ferme) :
  l'`open(path)` de Node.
- **File** (vague 5) : les requêtes d'un cœur s'exécutent une à une dans
  l'ordre où elles sont faites, chacune jusqu'au bout (ses relances après
  un appel nul comprises) ; un *job* (`Core.exclusive`) tient le cœur pour
  plusieurs requêtes et le JavaScript entre elles (`search`, `filter` de
  l'API). Dans la page (`WasmCore`) comme en worker (`WorkerCore` met en
  file côté appelant, le worker exécute ses messages un par un). Sans
  elle, un appel fait pendant qu'un autre attendait ses CMaps passait
  avant sa relance.
- **Pages** : `page_info` (la géométrie, puis `width` et `height` lus
  chacun dans une instruction : sa valeur, ou son exception sous
  `width_error`/`height_error`, le dict d'`errors.rs`), `page_objects`, `page_attr` (`getattr(page,
  nom)` : `chars`, `rects`..., les arêtes, `annots`, `hyperlinks`,
  `objects`, les mêmes noms que `doc_attr`), `page_structure_tree`, `page_extract_text`,
  `page_extract_text_simple`, `page_extract_words`,
  `page_extract_text_lines`, la recherche en deux appels
  (`page_search_text` : le texte que parcourt le motif, celui de la carte
  de `search`, calculée là où `search` la calcule ; JS y exécute sa
  `RegExp` ; `page_search` prend les spans, comme `textmap_search`, en
  unités UTF-16, ou en points de code de Python avec `code_points`, son
  septième argument : les spans de `re` que donne le harnais des traces),
  `page_crop`/`page_within_bbox`/`page_outside_bbox`,
  `page_filter` (le masque que JS a calculé sur les objets du parent, par
  sorte : la fonction de test le lit par identité ; des objets recalculés
  après un `flush_cache` du parent ne sont pas dans le masque, `NotPorted`),
  `page_dedupe_chars` (ces cinq donnent un handle de page dérivée),
  `page_find_tables`/`page_find_table` (un handle par table),
`page_debug_tablefinder` (`obj.debug_tablefinder(*args)` : `edges`,
`intersections` comme les éléments du dict de Python, `[[(x, top), {"v",
"h"}], ...]`, `cells`, `tables` avec leurs handles, `settings`, les champs
du `TableSettings` résolu),
  `page_extract_tables`/`page_extract_table`, `page_to_dict`/`to_json`/
  `to_csv`, `page_close`, et `table_extract` sur une table.
- **Arbres de structure** (vague 5, `crates/wasm/src/structs.rs`) :
  `struct_tree [page ou None]` sur un document (`PDFStructTree(obj,
  page)` : le handle de l'arbre et ses éléments, chacun les champs du
  dataclass, `children` compris, écrits sans récursion ; la page d'un
  autre document est `NotPorted`), puis sur l'arbre, un élément nommé par
  son chemin (les indices depuis `tree.children`) : `struct_find_all`
  (`list(x.find_all(m))`), `struct_find` (`x.find(m)`) — `x` l'arbre ou un
  élément, `m` un type (`match_tag`) ou une autre valeur, appelée comme
  Python l'appelle (`TypeError` au premier élément, un `dict` compris) ;
  avec un troisième argument, le masque `[...]` (la vérité de la fonction
  ou de la `RegExp` que JS a évaluée sur chaque élément, dans l'ordre de
  `_find_all`, lue par identité dans une fonction d'un cadre), `m` n'est
  pas lu : le masque a son propre argument pour qu'aucune valeur d'un
  appelant ne soit prise pour lui (jonction de la vague 5 : un
  `{mask: [...]}` d'un utilisateur était lu comme masque, là où Python
  appelle le `dict`) ; les chemins des éléments trouvés —,
  `struct_element_bbox` (`obj.element_bbox(el)`), `struct_all_mcids`
  (`list(el.all_mcids())`), `struct_element_repr` (`repr(el)`).
- **Flux** (vague 5, `crates/wasm/src/streams.rs`) : avec
  `set_stream_refs`, chaque `PDFStream` d'un résultat reçoit un handle
  (le premier handle vivant de ce flux, sinon un nouveau que l'opération
  crée, dans l'ordre d'écriture, comme les pages : `Core::label_values`),
  qui appartient au document et vit autant que lui ; `stream_rawdata`,
  `stream_data` (`obj.rawdata`, `obj.data`), `stream_get_data`,
  `stream_get_rawdata` (`obj.get_data()`, qui décode le flux, et
  `obj.get_rawdata()`) atteignent son document (un rejeu refait le
  décodage). Après `PDF.close()`, un flux garde ses octets, comme en
  Python. En JS (`pdfobjects.ts`, `runtime.ts` `StreamRefs`), un
  `PDFStream` par handle tant qu'il vit (`WeakRef`), lu dans les jobs de
  son document (`CallOptions.owner`, le `PDFPlumberDocument`) : après
  `release()`, `DocumentReleased`.
- **Profondeur** : chaque requête est une instruction au niveau du module
  d'un script Python (`ENTRY_DEPTH` : un cadre, deux unités C, mesurés par
  le script) ; une propriété y est lue à un site qui lui est propre
  (`pycompat::specialize::AttrSite` : le getter appelé depuis le C la
  première fois, deux unités), une méthode appelée avec `*args, **kwargs`
  (`CALL_FUNCTION_EX` : la méthode liée entrée depuis le C, deux unités),
  un attribut par son nom avec `getattr`. Ce script est
  `oracle/unit/js_pages_run.py` : une `RecursionError` arrive par la
  frontière là où elle arrive dans ce script. Les arguments sont liés
  comme CPython lie ceux de l'appel (`pycompat::call`) : un mot-clé qui
  nomme un paramètre déjà lié (`self`, `bbox` de `crop`, `strip` et
  `return_chars` d'`extract_text_lines`, `pattern` de `search`) lève
  « got multiple values for argument », un inconnu « got an unexpected
  keyword argument », un `**` qui n'est pas un dict « argument after **
  must be a mapping », avant l'appel ; `table_settings` est résolu comme
  `TableSettings.resolve` le fait de n'importe quelle valeur, et les
  options de `to_dict`/`to_json`/`to_csv` passent telles quelles au code
  porté (`stream=None` rend le texte, un autre flux est `NotPorted`).
  `page_filter` calcule les objets du parent comme `mask_of` du script
  (un cadre, un site de chargement à lui) : JavaScript les a déjà calculés
  par `page_objects` pour y évaluer sa fonction. Une instruction de Python
  écrite autrement (des mots-clés explicites plutôt que `**options`, une
  propriété lue à un site neuf) prend jusqu'à deux unités C de moins ou de
  plus : visible seulement à la limite des unités C.
- **Appels nuls** : un document que l'appel nul a atteint jette son état
  (son `PDF`, les pages de ses handles de pages et de tables,
  `Object::reset`), puis le cœur **rejoue son historique** : chaque appel
  qui l'a atteint (de `doc_open` à l'appel précédent, ceux d'une
  `sequence` un par un ; `Entry`), résultats et diagnostics jetés, depuis
  `ENTRY_DEPTH`, chacun remplissant les handles qu'il avait créés. Chaque
  entrée garde l'état du processus d'où partait sa première exécution
  (`ProcessState` : les symboles internés en chaîne persistante, partagée
  entre les états ; le reste, borné par les sites statiques du port,
  partagé avec l'état précédent quand rien n'a changé) et la façon dont
  elle a fini (l'exception ou rien) : son rejeu repart de cet état, sans
  quoi un nom interné, un site spécialisé ou une CMap mise en cache par
  un appel postérieur le laisserait descendre plus loin que la première
  fois, réussir là où il avait levé `RecursionError`, et mettre en cache
  des objets ou des pages que Python n'a jamais. Un rejeu qui finit
  autrement que la première exécution (autre exception, autres objets
  créés, une CMap manquée) laisse ses documents inutilisables : toute
  opération qui les atteint lève `RuntimeError` (jamais un document
  silencieusement différent). L'état du processus de l'appel nul est
  remis après le rejeu. Le document est alors l'objet qu'a l'exécution
  unique de Python à ce point (caches des pages, flux décodés, décodages
  en place des métadonnées et de `Page.annots`), pas un document rouvert.
  Une opération n'atteint qu'un document ; l'appel nul retire de
  l'historique les entrées de cet appel. Le rejeu refait le travail des
  appels précédents, une fois par CMap manquante (le préchargement des
  CMaps d'un document est une optimisation possible de la vague 5).
- **Handles libérés** : libérer une page ou une table libère son handle
  (son emplacement resservira) ; l'historique nomme les objets par leur
  identité (`Target::Object`), pas par leur handle : un rejeu refait un
  objet libéré hors de la table, pour les appels qui l'ont utilisé, puis
  le jette. Libérer le document libère ses pages, ses tables et son
  historique ; `free` ne peut pas entrer dans une `sequence` (un appel nul
  ne le déferait pas). L'historique d'un document ne raccourcit qu'à sa
  libération : un document gardé longtemps garde chaque appel qui l'a
  atteint (ses arguments et un état du processus partagé avec les appels
  voisins), et un appel nul les rejoue tous.
- **Historique borné** (vague 5) : une fois chaque CMap connue donnée au
  cœur (`cmaps::complete` ; une installation sans CMaps l'est dès le
  départ), aucun appel ne peut plus être nul : l'historique est vidé au
  début de l'appel suivant et plus rien n'y est noté (les documents sont
  marqués : si des noms de CMaps devenaient connus ensuite, un appel nul
  sur eux les laisserait inutilisables, jamais différents). Au-delà de
  `HISTORY_LIMIT` entrées (4 096), chaque réponse le signale (drapeau 1) et
  le runtime donne toutes les CMaps (`WasmCore.loadAll`, ~3 Mo) : la
  mémoire de l'historique est bornée, sans rien changer à Python. Testé
  par `the_history_goes_once_every_cmap_is_handed_over` et dans
  `tests.mjs` (`set_history_limit`).
- **Tests** : `oracle/unit/js_runtime.py` exécute des sessions de cas
  dans un processus neuf chacune, au niveau du module de
  `js_pages_run.py` (`pages.jsonl`) : `documents` (toutes les opérations,
  fichiers `micro` et upstream, options d'ouverture, erreurs), `cjk`
  (CMaps paresseuses, appels nuls et rejeux), et des fenêtres autour de la
  première profondeur qui lève `RecursionError` par la frontière (Form
  XObjects imbriqués sous `objects`, `chars` et `extract_text`, tirets
  imbriqués sous `to_json`, chaîne d'éléments de structure sous
  `pdf.structure_tree` ; opérandes imbriqués à la limite des unités C sous
  chaque chemin qui interprète la page : `objects`, chaque liste par sorte
  et chaque liste d'arêtes par `getattr`, `extract_text`,
  `extract_text_simple`, `extract_words`, `extract_text_lines`, `search`,
  `dedupe_chars`, `find_tables`, `find_table`, `extract_tables`,
  `extract_table`, `Table.extract` d'une table trouvée par lignes
  explicites, `to_dict`, `to_json`, `to_csv` (avec et sans `object_types`), et ceux du document
  (`objects`, `edges`, `to_dict`, `to_json`, `to_csv`) ; `repr` d'un `/K`
  imbriqué et `str(s)` de `decode_text` sous `pdf.structure_tree` ; un
  chargement de propriété spécialisé pour une classe puis exécuté pour une
  autre : `extract_text` d'une `Page` puis d'une `CroppedPage`,
  `Table.extract` d'une table de chacune, `chars` d'une `Page` puis d'un
  `PDF` ; `filter` ; un appel nul après une `RecursionError` : `objects`
  d'une page à la limite des unités C puis le texte d'une page CJK, la
  même chose avec une police CJK sous la page profonde, `PDF.pages` qui
  lève à la limite des cadres puis un appel nul sur sa première page),
  cherchée par dichotomie sur le script : 46 fenêtres de quatre
  profondeurs et 3 de huit. Les fenêtres de classe dichotomisent leur dernière
  opération, sur un second document : sur le premier, la seconde
  exécution (spécialisée, deux unités de moins) réussit à la profondeur où
  la dernière échoue, met les objets de la page en cache, et la dernière
  ne descend plus. Rejouées par `js_runtime_unit.rs`
  (`page_operations_match_pdfplumber_through_the_boundary`, deux formats,
  l'un sans collecte de diagnostics, l'autre au niveau `debug`, un fil
  neuf par session) et par JS (Node, navigateurs, cœur direct et
  worker) avec la `RegExp` de JS pour `search`, dont les spans doivent être
  ceux de `re` ; `a_void_call_rebuilds_its_document_by_replaying_its_history`
  compare une session aux appels nuls (pages dérivées, page libérée,
  caches) à la même session dans un cœur qui a toutes les CMaps dès le
  départ.

## Paquet JS

```
js/src/index.ts, browser.ts   points d'entrée Node et navigateur (plateforme + exports.ts)
js/src/api.ts                 open(), configure(), PDFPlumberDocument, PDFPlumberPage
js/src/core/runtime.ts        WasmCore : ABI, requêtes, relance après CMaps
js/src/core/wire.ts           décodeurs PJB1/JSON, encodeur des arguments
js/src/platform/*.ts          Node (fs, fetch) et navigateur (fetch, compileStreaming, Worker)
js/src/worker/*.ts            mode worker : requêtes encodées ici, exécutées (CMaps comprises)
                              dans le worker, réponse décodée ici (les classes survivent)
js/src/search.ts              motifs de search() (RegExp JS → spans, voir public-api.md)
js/src/internal.ts            API interne des tests et du benchmark (pdfplumber-js/internal)
js/src/hooks.ts               crochets internes du harnais des traces (ORACLE, TYPED ; jamais publics)
js/src/core/typed.ts          valeurs typées (PyDict, project) du format typé
js/trace/                     traces des étapes plumber_* par l'API publique (oracle.verify --engine js-api)
```

Un nouveau cœur (après un plantage, `configure()`, ou `dropCore()` des
tests, qui rejouent chaque session de `js_pages_run.py` dans un processus
neuf) instancie le module déjà compilé de la même source (URL par défaut,
chaîne ou `URL` ; des octets ou un `WebAssembly.Module` donnés passent
tels quels) au lieu de le télécharger et de le compiler de nouveau, et
`dropCore()` arrête le worker du cœur abandonné. Sans cela, les ~400
cœurs du test des sessions gardaient chacun leur code compilé (Firefox)
ou leur worker (Chromium, WebKit) jusqu'au ramasse-miettes : le test
dépassait les 5 Go de `heavy.sh` (`check-browsers.sh` de l'intégration de
la vague 4).

Build : `node scripts/build.mjs` = `build-wasm.mjs` (cargo `--profile dist`,
puis `wasm-opt -O3` du paquet npm `binaryen`, `--no-opt` pour l'éviter),
`build-cmaps.mjs`, `build-js.mjs` (`tsc` → `dist/src` avec les `.d.ts` ;
esbuild → `dist/browser/{index,worker,internal}.js` avec morceaux partagés).
Aucune dépendance d'exécution. `node scripts/build-wasm.mjs --oracle`
construit `dist/pdfplumber-oracle.wasm` : le même module avec les crochets
d'oracle du harnais (feature `oracle-hooks`, `crates/wasm/src/oracle.rs`,
opérations `oracle_*` : rang des `id()` de Python, ligne `ids`, indices par
identité), dans son propre dossier `target/oracle`, jamais livré
(`package.json` ne liste que `dist/pdfplumber.wasm`).

## Taille

`node scripts/size.mjs` (gzip -9, brotli 11) ; `--budget` dans `check.sh`.

| Fichier | brut | gzip | brotli |
|---|---|---|---|
| `dist/pdfplumber.wasm`, cargo `dist` sans `wasm-opt` | 987 949 | 322 247 | 238 048 |
| `dist/pdfplumber.wasm`, `wasm-opt -O3` | 760 450 | 299 366 | 226 421 |
| `dist/browser/index.js` et ses morceaux | 26 371 | 9 745 | 8 837 |
| `dist/browser/worker.js` | 587 | 376 | 331 |
| `dist/src` (Node, `.js`) | 70 940 | 22 037 | 19 314 |
| `dist/cmaps` (148 fichiers zlib) | 2 976 819 | — | — |

Le Wasm contient déjà les tables générées de pdfminer (glyphlist,
fontmetrics), les tables Unicode de `pycompat`, `layout`, `text`, `table` ;
la couche page ajoutera l'interpréteur et les polices. Budget (brotli) :
1 Mo pour le Wasm, 40 Ko pour le JS navigateur.

## Tests

- **Rust** (`cargo test -p pdfplumber-wasm`, natif et `wasm32-wasip1` dans
  `check.sh`) : `tests/boundary.rs` (4 000 valeurs aléatoires encodées en
  PJB1 et en JSON et décodées par des décodeurs Rust qui suivent `wire.ts`,
  toutes les sortes de colonnes, formes multiples, surrogates, entiers aux
  bornes, cycles PDF → `ValueError("Circular reference detected")`,
  200 000 niveaux d'imbrication écrits, lus et libérés, arguments
  aléatoires, entrées tronquées ou altérées sans panique, spans de
  recherche invalides, ABI, erreurs des requêtes, diagnostics) et
  `tests/js_runtime_unit.rs` (rejeu des cas Python de
  `oracle/unit/js_runtime.py` à travers `Core::call`, dans les deux formats :
  MRO, CMaps paresseuses, appels nuls, `main_group` de `search`, texte et
  tables).
- **JS** : `js/test/shared/tests.mjs`, les mêmes tests pour Node (vitest,
  `js/test/node/`, avec et sans génération de code) et les navigateurs
  (Playwright, `js/test/browser/`, cœur dans la page et en mode worker) :
  chargement, valeurs adverses (NaN, -0, grands entiers, surrogates isolés
  depuis des tampons faits à la main, `__proto__`, 10^5 niveaux dans les
  deux sens, cycles), formes de table dont les clés ne diffèrent que par la
  place d'un U+0000, bloc PJB1 fait à la main, erreurs et MRO contre
  CPython, `CoreCrash` sur un dépassement de pile (module fait à la main),
  diagnostics, CMaps paresseuses et appels nuls contre pdfminer,
  texte/mots/recherche (dont `main_group` contre `m.group()`)/tables contre
  pdfplumber, entrées et options de `open()` (`open.jsonl`), un PDF une
  seule fois en mémoire, absence de fuite de tampons.
- **Sessions de la frontière, vague 5** (`pages.jsonl`) : `structure`
  (arbres des fichiers étiquetés d'upstream et d'un cas sur deux
  d'`adv-structure` : document, page, page rognée ; chaque matcher, les
  chemins absents, `element_bbox`, `all_mcids`, `repr`, après
  `doc_close`), `streams` (résultats qui tiennent des flux, lectures
  avant et après décodage et après `doc_close`, `page_close` puis les
  flux refaits, objets d'un résultat rendus en argument), et dans
  `documents` `page_info` aux exceptions séparées, `debug_tablefinder`,
  arguments `PSLiteral`/`PSKeyword`/`PDFObjRef`/page (jamais un littéral
  mêlé à des nombres dans ce que pdfplumber met dans un `set` :
  `cluster_list` trie `set(xs)`, dont l'ordre suit alors l'adresse du
  littéral, `id()`, et le `TypeError` de Python change d'un processus à
  l'autre ; non portable, comme l'ordre des NaN) ; fenêtres de
  profondeur de `debug_tablefinder` (unités C), de `PDFStructTree`
  (récursion de `prune`, `str(s)` de `decode_text`) et de `repr(el)`
  (cadres), et `flate-warning-*` : une chaîne de formes dont la dernière
  a des données Flate corrompues, si bien que le point le plus profond
  est le `log.warning` de `decompress_corrupted` (les cadres de l'appel,
  logging coupé : `Logger::log`). La session `streams` ouvre aussi un
  document par chemin (`doc_open_file`, rejoué avec le nom et les octets
  du fichier) dont les flux d'image ont un `/Filter` indirect, lus après
  `PDF.close()`, et une annotation qui tient des flux dans un dict, une
  liste et un dict dans une liste.
- **API publique** (vague 5) : les sessions de `oracle/unit/js_api.py`
  (`sessions.jsonl`), rejouées par `js/test/shared/api-sessions.mjs` par
  l'API publique seulement, dans Node sur les sources (`api.test.ts`) et
  sur le build `dist` (`api-dist.test.ts`), et dans les navigateurs, dans
  la page et en worker (`api.spec.ts` ; `js/test/serve.mjs` sert aussi
  `/upstream/` et `/oracle-api/`) ; voir `architecture.md`, « Vague 5 »,
  « Harnais ».
- `scripts/check.sh` : noms de CMaps, build (sans `wasm-opt`), types, vitest,
  budget. `scripts/check-browsers.sh` : build complet puis Playwright.
- **Mutations** (`docs/verification.md`), sur tout `crates/wasm/src` et sur
  le code de la frontière dans `crates/pdfplumber` (libération de
  `PyValue`, `TextMap::search_with`, `resolved`), tests du paquet
  `pdfplumber-wasm` :

  ```sh
  scripts/heavy.sh cargo mutants -f crates/wasm/src/{abi,cmaps,errors,handles,ops,strings}.rs \
    -f crates/wasm/src/wire/{binary,json,mod}.rs -f crates/pdfplumber/src/{obj,table,utils/text}.rs \
    -F '^crates/wasm/src/|take_items|Drop for PyValue|search_with|replace resolved|in resolved' \
    --test-package pdfplumber-wasm --profile mutants -j 1 --timeout 20
  scripts/heavy.sh scripts/mutants-ok.sh -f <les fichiers de crates/wasm/src> \
    --profile mutants -j 1 --timeout 20 -- --package=pdfplumber-wasm
  ```

  896 mutants : 538 détectés, 311 non compilables (les `PyResult::new(..)`,
  voir la passe `Ok(..)`), 1 délai dépassé (`mro` dont la linéarisation ne
  termine plus : une boucle, détectée), 46 survivants. Tués depuis par un
  test : les valeurs de `test_values` (épinglées en Rust comme
  `expectedTestValues` les attend en JS), le repli `CMap()` de
  `doc_cmap_decode` sur `CMapNotFound` seulement, U+10FFFF et les positions
  d'erreur de l'UTF-8 généralisé (vérifiés en reposant à la main onze de
  ces mutants dans une copie : tous détectés ; la relance `cargo mutants`
  ciblée n'a pas obtenu le créneau lourd, tenu des heures par d'autres
  chantiers) ; la
  borne de `group_index` (redondante avec `search_with`) est retirée.
  Équivalents, notés dans le code : les `|`/`^` sur des bits disjoints
  (`handles.rs`, `varint`, `column_kind`, `strings.rs`), la capacité
  réservée d'une table (une indication), `>`/`>=` sur un infini
  (`json.rs`), les raccourcis `Utf8`/`Utf16` de `PyStrRef` (même résultat
  par le chemin général), la garde `KeyError` de `cmap_unichr` (seule
  erreur possible des tables chargées). Passe `Ok(..)` : 93 mutants, 2
  survivants (`ObjectsPage::bbox`, `BinWriter::table` → `Ok(false)`) tués
  par des lignes explicites numériques (`js_runtime.py`) et un test du
  format table ; relancée sur `ops.rs` et `wire/binary.rs` : 70 mutants,
  63 détectés, 7 non compilables, aucun survivant ; `enter` → `Ok(())`
  boucle sur une valeur cyclique (délai, détecté). Les corps `Ok(vec![])`
  et `Ok(vec![Obj::new()])` de `TextMap::search_with` ont été posés à la
  main : les rejeux échouent.
- **Navigateurs sur cette machine** : Chromium 148, Firefox 150 (Nightly de
  Playwright) et WebKit 26.4 de Playwright 1.60 ; Firefox 158 du système pour
  les mesures. WebKit manque de bibliothèques système (GStreamer, libwoff2,
  libavif, EGL de Mesa…) et la machine n'a pas `sudo` :
  `scripts/webkit-local-libs.sh` les télécharge (`apt-get download`, sans
  installation) sous `.cache/webkit-libs` et écrit un lanceur que
  `check-browsers.sh` passe à Playwright (`PDFPLUMBER_WEBKIT`,
  `PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS=1`).
