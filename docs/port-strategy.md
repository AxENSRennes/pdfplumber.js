# Stratégie du port : diagnostic, décision, méthode

Document de passation (octobre 2026). À lire avant de toucher au port. Il
résume le diagnostic du code actuel, la décision d'architecture prise après deux
expériences mesurées, et la méthode de vérification à suivre. Les mesures et
les leçons des deux expériences sont dans
[`decision-experiments.md`](decision-experiments.md).

## Objectif

Un pdfplumber qui tourne dans le navigateur avec un runtime aussi efficace que
possible, et le même comportement que pdfplumber/pdfminer en Python. La valeur
de pdfplumber tient surtout à son extraction de tables, qui marche là où les
solutions artisanales échouent. Pyodide est écarté : trop lourd au démarrage et
à l'exécution.

## Diagnostic du port actuel (`src/`)

**Deux moteurs qui se contredisent.** pdf.js fait l'interprétation (liste
d'opérateurs, polices, texte), alors que l'oracle est pdfminer. Un parseur
natif partiel (`src/pdf/`) relit les flux de contenu pour produire des
« indices » (couleurs, transformations, épaisseurs, chemins). Ces indices ne
sont réinjectés que si les nombres d'opérations coïncident des deux côtés, et
les caractères sont parfois remplacés en bloc sur une heuristique
(`src/open.ts`, autour des lignes 328 à 420 :
`native > 2 × pdfjs && native >= pdfjs + 100`). Chaque divergence entre pdf.js
et pdfminer devient un patch qui défait le travail de pdf.js.

**Des cas spéciaux calés sur les données observées.** Exemples :

- `src/pdfminer-compat.ts:58` : reconnaissance d'**un fichier précis** à son
  empreinte (Producer PyPDF2, Title IntMetadata, etc.).
- `src/font-decoding.ts:315` : police TimesNewRoman sans ToUnicode et glyphe `&`
  donne `"C"`.
- `src/text.ts:257` (chiffres en Times Bold avec `x0 > 500`) et `src/text.ts:276`
  (`splitChartSizeGroup` : Times-Roman entre 6 et 9 pt, « paysage » si hauteur
  < 700).
- `src/pdf.ts:309` : correction de bbox réservée à TimesNewRoman.
- `src/open.ts:203` : regex sur le texte brut pour **simuler** l'erreur Python
  `maximum recursion depth exceeded`.

**Généralisation mesurée.** Sur 60 PDF de la suite de tests pdf.js jamais vus
par le port : 52/60 identiques à pdfplumber sur des métriques grossières, et
**34/42** pages avec texte identiques au caractère près (position, police,
couleur). La doc du projet annonçait 119/120 sur ses holdouts. Sur deux
fichiers, le JS sort un *meilleur* texte que pdfminer (`à` au lieu de
`(cid:224)`) parce que pdf.js décode mieux les polices, et c'est compté comme un
échec : c'est le symptôme du mauvais choix de moteur.

**Ce qui est réutilisable.** Le harnais de tests, les manifestes et le corpus,
l'API publique et ses types, et les briques bas niveau de `src/pdf/` (à
revalider par traces). Les couches mots et tables sont à refaire par port
fidèle, comme dans l'expérience sur les tables
(`git show legacy-pdfjs:experiments/tables-bench/rust/lib.rs`).

## Décision d'architecture

**Un cœur en Rust compilé en Wasm, porté module par module depuis pdfminer et
pdfplumber, avec une API à gros grain** : le JS ne reçoit que des résultats par
page (caractères en tableaux par colonne, tables), jamais chaque objet
intermédiaire. pdf.js sort du chemin d'extraction. Le même cœur donne aussi un
module natif (PyO3), soit un pdfplumber environ 60× plus rapide que CPython.

Cette décision vient de deux expériences, chacune avec deux ports
(TS et Rust/Wasm) identiques au bit près à Python et optimisés avec le même
effort (mesures sur Mac M-series, médianes en ms) :

| Étage | Python | TS optimisé | Wasm, objets renvoyés au JS | Wasm, rien ne revient au JS | Rust natif |
|---|---|---|---|---|---|
| Parsing des flux de contenu (63,8 Mo) | 17 225 | 575 (Chromium) / 487 (WebKit) / 1 273 (Firefox) | 586 / 520 / 745 | 375 / 373 / 495 | 289 |
| Tables, pages réelles (1 083 pages) | 2 214 | 105 / 71 / 89 | 51 / 40 / 53 | 36 / 35 / 45 | 29 |

Lecture :

- Au grain du token, le Wasm ne gagne rien sur V8 et JSC : le coût de la
  frontière avec le JS annule le gain. Avec une frontière grossière, il gagne
  1,3 à 1,5× sur le parsing et environ 2× sur les tables.
- Un premier jet TS peut être 3,6× plus lent que sa version optimisée, à cause
  de pièges propres à V8. Ne jamais conclure sans profiler.
- Le Firefox fourni par Playwright (build Nightly) exécute le Wasm environ 7×
  plus lentement. Mesurer avec un vrai Firefox.

## Méthode (obligatoire)

1. **Port fidèle, module par module** : mêmes noms, même structure, une
   référence `fichier:ligne` upstream par fonction. Les optimisations ne
   changent pas la sortie et sont vérifiées par les traces.
2. **Oracle à chaque étape, pas seulement à la fin.** Python écrit une trace
   canonique de l'étape (tokens et objets, bords fusionnés, intersections,
   cellules, etc.) ; le port écrit la même ; on compare des SHA-256. La
   première divergence localise le bug.
3. **Corpus adverse généré, avec Python comme oracle**, en plus du corpus réel.
   Le corpus réel seul ne détecte pas les erreurs aux frontières (blocs de 4 096
   octets, tolérances exactes, échappements).
4. **Tests de mutation** : casser volontairement des détails et vérifier que le
   test échoue. Si une mutation passe, le corpus est trop gentil.
5. **Aucun cas spécial.** Pas de nom de police, d'empreinte de fichier ou de
   seuil numérique dans le code sans contrepartie upstream citée. Un écart avec
   Python se corrige à sa cause, en remontant la trace.
6. **Profiler avant de conclure** sur la performance, et donner le même effort
   d'optimisation aux implémentations comparées.

## Sémantique Python à émuler (pièges rencontrés)

À centraliser dans un module `pycompat` plutôt qu'à éparpiller :

- **`sum()` en Python 3.12** : les ints sont additionnés exactement ; le premier
  float est ajouté sans compensation, puis les floats suivants avec la
  compensation de Neumaier, et les ints rencontrés ensuite sans compensation ;
  enfin `+ c` si c est fini et non nul. Avec une somme naïve, 219 des 418 pages
  réelles avec tables ont des coordonnées différentes (une a une structure
  différente). **La sortie de pdfplumber dépend donc de la version de Python** :
  l'oracle est figé sur 3.12 (`.python-version`).
- **int contre float** : JS ne les distingue pas. Sans effet sur les données
  réelles (pdfplumber ne produit que des floats), mais visible sur des entrées
  construites à la main.
- `min()` et `max()` gardent le premier élément à égalité (signe de zéro) ;
  `float.__repr__` (exposant hors de [1e-4, 1e16)) ; `int()` refuse plus de
  4 300 chiffres ; `\s` des regex bytes vaut `[\t\n\v\f\r ]` ; ordre d'insertion
  des dicts ; tris stables.
- **pdfminer lit par blocs de 4 096 octets**, et certains comportements en
  dépendent (échappement `\` suivi de `\r\n` coupé entre deux blocs). Le port
  garde ce découpage.
- Les exceptions Python qui s'échappent (AssertionError, PSSyntaxError,
  IndexError, TypeError, KeyError) font partie du comportement à reproduire.

## Environnement

- Python **3.12** (oracle), uv, Node 26, Rust 1.99 avec la cible
  `wasm32-unknown-unknown` (`rust-toolchain.toml`).
- Installation : `scripts/setup-dev.sh`. Les PDF externes (`corpus/external*`), ignorés par git, se
  téléchargent depuis leurs manifestes (avec vérification des SHA-256) ou se
  copient depuis une machine qui les a.
- Les mesures de performance ne se comparent qu'entre runs d'une même machine.

## État

- Octobre 2026 : l'ancien port (pdf.js + rustines) est retiré du dépôt et
  étiqueté `legacy-pdfjs`. Le corpus est dans `corpus/`, les sources Python
  portées dans `upstream/` (pdfminer.six épinglé sur le tag `20260107`,
  exactement le code exécuté par l'oracle).
- Version de Python : vérifiée sur 316 pages réelles (134 PDF), pdfplumber
  sort des résultats identiques sous 3.12, 3.13 et 3.14 ; sous 3.11, 54 pages
  ont des coordonnées de tables différentes (contenu extrait identique). Le
  contrat est donc « Python ≥ 3.12 », l'outillage reste figé sur 3.12.
- Le dossier `experiments/` (prototypes TS et Rust des deux expériences) a été
  retiré du dépôt au commit « Remove experiments/ … » qui suit l'intégration de
  la fondation Rust et du harnais Python ; on le lit avec
  `git show legacy-pdfjs:experiments/<chemin>` (par exemple
  `git ls-tree -r --name-only legacy-pdfjs experiments`). Le prototype de
  parsing vit désormais dans `crates/pdfminer` ; celui des tables sera repris
  lors du port de `pdfplumber/table.py`. Mesures et leçons :
  [`decision-experiments.md`](decision-experiments.md).
- Le port suit l'ordre des dépendances : fondations (`pycompat`, `psparser`,
  modèle d'objets, outillage de traces), puis filtres, chiffrement, CMaps et
  polices, puis document et pages, interpréteur et mise en page, puis
  pdfplumber, puis le Wasm et l'API JS. L'organisation est décrite dans
  [`architecture.md`](architecture.md), la méthode de preuve dans
  [`verification.md`](verification.md).
- **Vague 0** (fondations) : `pycompat` (nombres, `sum`, `pow` de glibc,
  `repr`, `round`, récursion, Unicode), `psparser`, `pdftypes`,
  `PDFContentParser`, `canon`/JSON typé, `pdftrace`, harnais Python (étapes,
  cache, `verify`, générateurs unitaires à empreinte), `scripts/check.sh`
  (natif et `wasm32-wasip1`), `scripts/heavy.sh`.
- **Vague 1** (octobre 2026, fusionnée dans `rewrite/rust-core`) : les
  modules sans document, chacun avec son générateur unitaire, son rejeu
  Rust (natif et WASI) et sa passe de mutations (liste dans
  `verification.md`) :
  - `utils`, `casting`, `data_structures`, `pdfcolor`, `layout` (objets
    `LT*` et analyse complète, ordre des `id()` injectable) et `ctm` de
    pdfplumber ;
  - `pycompat::zlib` (inflate de zlib 1.3 avec la sémantique de CPython),
    filtres LZW, ASCII85/ASCIIHex, RunLength, prédicteurs,
    `PDFStream.decode` ; `ccitt` (décodeur G4), branché dans `decode` : les
    38 305 flux de l'ensemble `all` sont identiques, CCITT compris ;
  - chiffrement : `arcfour`, `_saslprep` (NFKC d'Unicode 3.2.0 générée),
    gestionnaires de sécurité standard (RC4, AES-128/256, révisions 2 à 6),
    `unpad_aes` ;
  - `glyphlist`, `latin_enc`, `fontmetrics` (générés), `encodingdb`,
    `cmapdb` (CMaps prédéfinies hors du binaire, servies par un
    `CMapProvider`) ;
  - pdfplumber : modèle d'objets (`T_obj`, valeurs Python), `clustering`,
    `geometry`, `utils/text.py` (mots, `TextMap`, `extract_text`, layout)
    et `table.py` complets, testés sur des pages synthétiques et réelles ;
    `pycompat::listsort` (timsort de CPython, utilisé aussi par le layout).
- **Vague 2** (octobre 2026, fusionnée dans `rewrite/rust-core` après
  revue et correction de chaque branche) :
  - document : `pdfparser`, `pdfdocument` (xrefs table/flux/repli,
    `getobj`, flux d'objets, mot de passe, plans, libellés, noms,
    destinations), `pdfpage` (`create_pages` paresseux), `pdftypes::ObjId`,
    `pycompat::bytes_format`, boîtes de `Page.__init__` ; comptage exact des
    cadres dans `psparser`, `pdftypes`, les filtres et le document
    (`pycompat::logging`, noms internés par processus) ;
  - polices : `pdffont` complet et la création de police de
    `PDFResourceManager.get_font` ; `cmapdb` compte les cadres de `CMapDB`
    et `CMapParser`. À l'intégration, les compensations que `pdffont` et
    `cmapdb` prenaient pour `psparser`/`pdftypes` ont été retirées (ils
    comptent désormais les leurs) : les seuils de profondeur restent
    identiques ;
  - `str` : `pycompat::PyStr` (surrogates isolés compris) dans le modèle
    d'objets, `LTChar`/`LTAnno`, `TextMap`, les tables et la frontière JS
    (arguments et messages d'erreur compris) ; `unicodedata.normalize`,
    `atan2` de glibc, arguments utilisateur de tout type, identité des NaN ;
    le texte de `to_unichr` entre tel quel dans `LTChar` ;
  - Wasm/JS ([`js-runtime.md`](js-runtime.md)) : ABI sans `wasm-bindgen`,
    format `PJB1`, erreurs avec MRO, diagnostics, CMaps paresseuses, paquet
    `js/` (Node, navigateur, worker) ; fournisseur de CMaps installé par
    chaque point d'entrée (Wasm, tests natifs et WASI, `pdftrace
    --cmap-dir`).
  Vérification à l'intégration : `scripts/check.sh` vert (natif,
  `wasm32-wasip1` sous Node, build `dist`, tests JS) ; `oracle.verify`
  identique sur `psparser`, `objects`, `streams`, `pages` pour `all` (1 853
  fichiers), `adv-objects` (333) et `adv-pages` (285) ;
  `scripts/check-browsers.sh` : 102 tests verts (Chromium, Firefox, WebKit).
  Mutations sur le code modifié à l'intégration (`--in-diff`, `-j 1`) :
  `crates/wasm` 87 + 19 mutants, `pdffont`/`cmapdb` 100, `pdftrace` 3, puis
  passes `Ok(..)` (12 et 22) et 9 mutants à la main pour les types sans
  `Default` : trois survivants, deux tués (borne du nombre de chaînes à
  surrogate de `read_table`, `binary.rs:508`) et un supprimé
  (`From<&Unichr>` sans appelant). Code de `crates/pdfplumber` réécrit en
  résolvant les fusions (`TextMap::search_with` sur `code_points()`,
  `WordExtractor::merge_chars`, `stream_missing_attribute` délégué à
  `PDFStream::missing_attribute`) : 16 mutants (4 tués, 12 non
  compilables), passe `Ok(..)` de `utils/text.rs` (67 tués, 4 non
  compilables) et 2 mutants à la main de `stream_missing_attribute`
  (tués par `ops_cases`), avec `text_tables_unit`. `fonts_unit` construit
  un `LTChar` pour chaque `to_unichr` et compte ce qu'il vérifie : 40 910
  (dont 113 surrogates isolés) sur `fonts` et 106 769 (dont 28) sur
  `corpus_fonts`, avec un plancher par test.
- **Vague 3** (octobre 2026, fusionnée dans `rewrite/rust-core` : base
  `wave3/base`, chantiers `wave3/interp` et `wave3/converter`, jonction
  `wave3/join`, revue `wave3/fix` ; détails dans `architecture.md`,
  « Vague 3 ») :
  - `pdfinterp` complet, `pdfdevice` (`PDFDevice`, `PDFTextDevice`),
    `converter` (`PDFLayoutAnalyzer`, `PDFPageAggregator`) et le
    périphérique de pdfplumber (`PDFPageAggregatorWithMarkedContent`),
    menés comme pdfplumber (un `PDFResourceManager` par document, un
    périphérique et un interpréteur par page) ; hors périmètre, notés
    dans les en-têtes : `TextConverter`, `HTMLConverter`, `XMLConverter`,
    `HOCRConverter`, `PDFConverter`, `TagExtractor` ;
  - étapes `fonts`, `interp`, `layout` (et `fonts_text`, `interp_text`
    sur le `PDFTextDevice` de pdfminer) ; corpus adverse `adv-interp`
    (353 PDF, dont les chaînes de Forms autour de la limite des cadres,
    `depth-*`, et les opérandes autour de la limite des unités C,
    `climit-*`) ; cadres et unités C comptés jusqu'au premier `str(x)`
    d'un site d'appel (`pycompat::specialize`) ; état du processus remis
    après un appel nul des CMaps paresseuses (`pdfminer::process_state`) ;
    pile Wasm de 8 Mio, vérifiée au profil `dist` par
    `scripts/wasm-depth-gate.sh`.
  - **`id()` du layout** : l'étape Python écrit le rang des `id()` de son
    analyse (lignes `ids`), que `pdftrace --oracle-dir` relit : parité
    exacte. La bibliothèque et l'API JS gardent `CreationOrder`, qui donne
    le résultat de Python sur 98,6 % des pages de `all` où Python est
    stable ; Python lui-même change d'analyse d'une exécution à l'autre
    sur 233 pages (10,6 % des pages à deux boîtes ou plus : les `id()`
    sont des adresses), et `CreationOrder` y retrouve l'un de ses tirages
    sur 124. Une statistique, pas un critère de parité.
  - **Noms des images en ligne** : `str(id(obj))` (une adresse, dans
    pdfminer/pdfinterp.py:1309 `PDFPageInterpreter.do_EI`) devient
    `inline{"n": k}` dans les traces et `"inline-<k>"` dans le port
    (`k` : rang dans la page) ; écart documenté, visible dans le `name`
    des images de pdfplumber.
  - **Journalisation** : la référence est celle de pdfminer coupée ; avec
    la configuration par défaut de Python, un `log.warning` formate son
    `%r` et peut lever `RecursionError` plus tôt (`verification.md`,
    « Étapes ») : écart documenté, non modélisé.
  - Vérification à l'intégration (`oracle.verify --timeout 600`, un job
    lourd à la fois) : identique à Python pour `psparser`, `objects`,
    `streams`, `pages`, `fonts`, `interp`, `layout` sur `all` (1 853
    fichiers, 20 premières pages), `adv-objects` (333), `adv-pages` (285)
    et `adv-interp` (353), plus `interp_text` et `fonts_text` sur
    `adv-interp` ; aucun délai, aucune erreur de l'oracle.
    `scripts/check.sh` vert (33 harnais `cargo test` par cible, dont 5 de
    doc-tests, certains vides : 206 tests réussis et 1 ignoré en natif,
    200 et 5 ignorés en `wasm32-wasip1`, dont les deux tests d'appel nul
    de `js_runtime_unit` relancés seuls, un processus WASI chacun ; porte
    de profondeur `dist` : 188 fenêtres d'`adv-interp` × 3 étapes, 79
    d'`adv-pages`, 23 d'`adv-objects`), `scripts/check-browsers.sh` (102
    tests : Chromium, Firefox, WebKit). Mutations : aucun survivant non
    justifié (`architecture.md`, « Jonction », « Corrections de la
    revue ») ; la fusion est sans conflit et n'a modifié aucun code.
  - Finalisation (revue de l'intégration) : `interp_text` et `fonts_text`
    repassés sur `all` après les changements de `wave3/fix` (premier
    `str(x)` de `literal_name`, état du processus remis) : 1 853
    identiques chacune ; `pdftrace` compilé en Wasm (`dist`) donne les
    traces natives sur `quick` pour `interp`, `fonts` et `layout` (150
    fichiers chacune), par `python -m oracle.wasm_parity`, qui passe le
    fournisseur de CMaps explicitement (3 fichiers de `quick` en
    dépendent) et le rang des `id()` de Python à `layout` ; les tests
    d'appel nul tournent désormais en Wasm ; l'en-tête de la porte de
    profondeur dit pourquoi ses traces `layout` (`CreationOrder`) sont
    celles de `verify` (188 fichiers identiques avec et sans
    `--oracle-dir`).
  - Temps (étape `layout`, 20 premières pages de `all`) : `pdftrace` 54 ms
    par page contre 288 ms pour Python hors démarrage (5,3 fois), dont
    l'essentiel à écrire la trace.
- **Vague 4** (octobre 2026, fusionnée dans `rewrite/rust-core` : base
  `wave4/base`, chantiers `wave4/page` et `wave4/doc`, jonction
  `wave4/join`, revue `wave4/fix` ; détails dans `architecture.md`,
  « Vague 4 ») : la couche page de pdfplumber (`pdf.py`, `page.py`,
  `container.py`, `convert.py`, `structure.py`, `utils/exceptions.py`,
  `utils/pdfinternals.py`) ; hors périmètre, notés dans les en-têtes et
  `public-api.md` (« Not Ported ») : `display.py`, `repair.py`, `cli.py`.
  Base (`architecture.md`, « Vague 4 ») : spécification Python des étapes `plumber_objects`,
  `plumber_text`, `plumber_tables`, `plumber_doc` (mesurée sur `quick`),
  interfaces Rust de `pdf`, `page`, `container`, `convert`, `structure`,
  `utils/exceptions` (squelettes qui compilent), contrat des chantiers
  PAGE et DOC, `oracle.verify` qui passe le fournisseur de CMaps.
  Chantier DOC (`wave4/doc`, `architecture.md`, « Chantier DOC ») :
  `structure.py` porté, métadonnées décodées en place
  (`PDFObject::Str`), `Page.annots`/`hyperlinks`/`structure_tree`,
  fichier fermé par `PDF.close`, étape `plumber_doc` identique sur `all`
  et sur `adv-structure` (441 PDF, avec `plumber_objects`), rejouée par
  `structure_unit`. Chantier PAGE (`wave4/page`, `architecture.md`,
  « Réalisation (branche `wave4/page`) ») : `pdf.py`, `page.py`,
  `container.py`, `convert.py`, le reste de `utils/pdfinternals.py` ;
  étapes `plumber_objects` (1 852/1 853 sur `all` : un fichier aux boîtes
  de texte en double, limite documentée du rang des `id()`),
  `plumber_text` et `plumber_tables` (1 853/1 853), les quatre étapes
  identiques sur `adv-plumber` (131), rejouées par `plumber_unit` ;
  balayages de profondeur de la couche page (`plumber_sweep`), qui ont
  fait compter les annotations des fonctions imbriquées
  (`pycompat::typing_cache`), `pathlib.Path` et les cadres de `table.py`
  au-dessus des propriétés de la page. Jonction (`wave4/join`,
  `architecture.md`, « Réalisation de la jonction ») : la fusion est vide
  (`wave4/page` contenait `wave4/doc`) ; les chargements de propriétés
  tiennent les unités C de l'appel du getter depuis le C jusqu'à leur
  spécialisation (`pycompat::specialize::AttrSite`, trouvé par les
  fenêtres `climit-*` d'`adv-interp` passées aux étapes `plumber_*`) ; la
  frontière Wasm garde le `PDF` de pdfplumber et ses pages
  (`crates/wasm/src/pages.rs`), chaque opération étant l'instruction d'un
  script Python (`oracle/unit/js_pages_run.py`), un appel nul rejouant
  l'historique des documents qu'il a atteints ; le test JS CJK passe par
  `pdf.pages`. Revue (`wave4/fix`, `architecture.md`, « Correctifs de la
  revue ») : rejeu d'un appel nul depuis l'état du processus de chaque
  entrée (et document inutilisable s'il diverge), liaison des arguments de
  CPython (`pycompat::call`), options de sérialisation en valeurs Python,
  `pycompat::warnings`, handles libérés réutilisés, `Drop` des chaînes de
  pages dérivées en boucle, valeurs `Page` de la frontière (`PageRef`),
  `pdf.pages`/`pdf.metadata` en promesses.
  - Vérification à l'intégration (la fusion de `wave4/fix` est sans
    conflit et ne change aucun code ; `python -m oracle.verify --timeout
    600 -j 3`, un job lourd à la fois, fournisseur de CMaps, caches de
    l'oracle reconstruits pour les étapes `psparser` à `fonts_text`, dont
    l'empreinte suit `oracle/depth.py`) : les 13 étapes (`psparser`,
    `objects`, `streams`, `pages`, `interp`, `fonts`, `layout`,
    `interp_text`, `fonts_text`, `plumber_objects`, `plumber_text`,
    `plumber_tables`, `plumber_doc`) sur `all` (1 853 fichiers, 20
    premières pages) et les cinq corpus adverses (`adv-objects` 333,
    `adv-pages` 285, `adv-interp` 353, `adv-plumber` 199,
    `adv-structure` 441) : 75 passes sur 78 identiques à Python, aucun
    délai, aucun plantage. Les trois autres sont les écarts connus :
    `plumber_objects` sur `all` (1 852/1 853 : les lignes `ids` de
    `pdfjs-issue19624-text.pdf`, deux boîtes de texte de même contenu que
    la clé ne distingue pas), `plumber_text` (336/353) et
    `plumber_tables` (341/353) sur `adv-interp` (fichiers `op-*`/`text-*`
    aux coordonnées NaN : l'ordre d'un `set` de NaN dans
    `cluster_objects` suit leur adresse, Python lui-même change d'un
    processus à l'autre ; l'oracle de `096-text-TL.pdf` pour
    `plumber_text` dépasse les 5 Go de `heavy.sh`).
    `scripts/check.sh` vert (39 min : 246 tests réussis et 1 ignoré en
    natif sur 36 harnais, 242 et 7 ignorés en `wasm32-wasip1` sur 40, les
    tests d'appel nul relancés chacun dans son processus WASI ; porte de
    profondeur `dist` = natif : 188 fenêtres d'`adv-interp` × 3 étapes, 79
    d'`adv-pages`, 23 d'`adv-objects`, `adv-plumber` 94 (`plumber_objects`,
    `plumber_text`), 72 (`plumber_doc`), 68 (`plumber_tables`),
    `adv-structure` 64 × 2 ; build `dist`, `tsc`, 42 tests Node, budget de
    taille ; 47 auto-tests de l'oracle) ; `pdftrace` compilé en Wasm
    (`dist`) donne les traces natives sur `quick` pour les 13 étapes
    (`oracle.wasm_parity`, 150 fichiers chacune) ;
    `scripts/check-browsers.sh` : 111 tests verts (Chromium, Firefox,
    WebKit, dans la page et en worker) après une correction du paquet JS :
    le test des sessions de la frontière (~400 cœurs neufs) dépassait les
    5 Go de `heavy.sh`, chaque nouveau cœur recompilant le module Wasm et
    `dropCore()` laissant vivre le worker de l'ancien ; le module compilé
    est désormais réutilisé et le worker arrêté (`js-runtime.md`,
    « Paquet JS »).
- **Vague 5** (en cours ; base `wave5/base`, `architecture.md`,
  « Vague 5 ») : contrat de l'API (`public-api.md`, table « Operations »),
  épine de l'API sur les opérations de la frontière (file FIFO des jobs,
  causes des erreurs, `open(path)` comme `pdfplumber.open(path)`),
  harnais (sessions `oracle/unit/js_api.py` rejouées par l'API publique
  dans Node, le build `dist` et les navigateurs ; traces des étapes
  `plumber_*` par l'API, `oracle.verify --engine js-api`), propriété des
  fichiers des chantiers API, BOUNDARY et PERF, référence PERF sur
  `quick`. Jonction (`wave5/join`, `architecture.md`, « Jonction ») : les
  trois chantiers fusionnés (un conflit, `path_segment`, les deux
  intentions gardées) ; le masque des matchers de structure dans son
  propre argument (un objet d'un appelant n'est plus pris pour lui) ;
  l'écart d'unités C de la couche page que PERF a trouvé, corrigé à ses
  causes (`isinstance` avec un tuple de classes, `"".join` et `ord` de
  `decode_text`, escaliers des abonnements `typing`, `re.sub` de
  `process_object`) et vérifié par `plumber_frames` ; chaque membre de
  `public-api.md` vérifié par l'API publique (sessions dans Node, `dist`,
  Chromium, Firefox, WebKit, page et worker ; traces `plumber_*` par
  l'API sur `all` et les ensembles adverses, différences toutes
  classées) ; API publique ×6,3 sur Python dans Node, ×6,0 dans
  Chromium, natif ×8,4 (`quick`). Corrections de la revue (`wave5/fix`,
  `architecture.md`, « Corrections de la revue ») : `pdf.page()` limité
  aux références du document, `null` pris pour `None`, objets sans valeur
  Python refusés, `pdf.pages` en FIFO, erreurs de `re` et de `filter`
  pour les arguments qui ne sont ni motifs ni fonctions, `toDict` et
  itération des éléments de structure ; fuites (flux, pages dérivées dans
  une boucle sans tour d'événements, historique d'une CMap absente),
  adresses au-delà de 2 Gio, worker planté arrêté ; une unité C de
  `re.sub` qui n'existait pas retirée ; fenêtres de profondeur propres à
  l'API ; licence, avis tiers, déclarations et bundlers. Ce qui suit est
  la liste d'origine de la vague.
- **Vague 5 (liste d'origine)** : l'API JS publique
  (`PDFPlumberDocument`, `PDFPlumberPage`), construite sur les opérations
  de la frontière que la vague 4 a vérifiées (`js-runtime.md`,
  « Documents et pages » : `pdf.pages` par `doc_pages`, la recherche en
  deux appels, `filter` par un masque). Ce qu'il lui faut :
  - le contrat déjà fixé par les types (`public-api.md`, « Asynchrony ») :
    `pdf.pages` et `pdf.metadata` sont des promesses (pages paresseuses,
    comme pdfplumber), les pages dérivées (`crop`, `withinBbox`,
    `outsideBbox`, `dedupeChars`) aussi ; une `Page` dans un résultat est
    un `PageRef` (son handle) ; `CreationOrder` par défaut pour les
    `id()` du layout ;
  - n'exposer que des noms d'attributs que la frontière sait lire :
    `doc_attr`/`page_attr` lèvent `NotPorted` hors des noms que
    `to_dict`/`to_csv` construisent (`width`, `bbox`, `layout`... passent
    par leurs propres opérations) ;
  - `element_bbox` de l'arbre de structure dans l'API (la fenêtre
    `climit-tf-list-tagged-nopg` le couvre déjà par l'étape) ;
  - le préchargement des CMaps d'un document : un appel nul rejoue tout
    l'historique du document, une fois par CMap manquante, et cet
    historique ne raccourcit qu'à la libération du document ;
  - les flux d'images par référence (aujourd'hui `rawdata`/`data` sont
    copiés entiers dans chaque résultat : un nouveau format de fil, une
    classe JS et une durée de vie de handle) ;
  - les mesures JS (`js/bench/`) sur l'API finale ; le point chaud de la bibliothèque est `page.objects`
    (`process_object`, la moitié du temps sans trace).
  Restent ouverts, documentés (`public-api.md`, `architecture.md`) : la
  copie profonde d'un document lu depuis des octets par `structure_tree`
  (`copy.deepcopy` de tout le `PDFDocument`, non modélisée : la
  `RecursionError` peut différer vers 320 niveaux d'éléments à attribut
  référence), le texte des `log.*` de pdfminer (seuls leurs cadres sont
  comptés ; les avertissements de pdfplumber atteignent `onWarning`), les
  noms `inline-<k>` répétés d'une page à l'autre dans `pdf.images` et
  `pdf.objects`, la clé de contenu des `id()` de `plumber_objects` (deux
  boîtes identiques), l'ordre des NaN dans `cluster_objects`, les
  constructeurs des vagues 0 à 2 qui comptent deux unités C par appel de
  classe contre trois pour `pdfinterp` et le layout (sans effet observable
  aujourd'hui : à unifier si une étape approche la limite C).
  Les questions ouvertes sont notées dans les en-têtes des modules, dans
  `architecture.md` et dans `js-runtime.md` (`search` suit la sémantique
  `RegExp` de JS, choix documenté ; `atan2` et le fournisseur de CMaps sont
  réglés).
