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
- **Vague 3** (en cours, octobre 2026) : base `wave3/base` partagée par
  deux chantiers parallèles (INTERP, CONVERTER ; plan et contrat dans
  `architecture.md`, « Vague 3 ») : côté Python, les étapes `fonts`,
  `interp`, `layout` (la spécification), mesurées sur `quick` ; côté Rust,
  les interfaces de `pdfdevice`, `pdfinterp`, `converter` et du
  périphérique de pdfplumber, en squelettes qui compilent, et
  `pdftrace --oracle-dir` (les `id()` de Python pour l'étape `layout`).
  Chantier INTERP (branche `wave3/interp`) : `pdfinterp` et `pdfdevice`
  complets, cadres comptés ; étapes Rust `fonts`, `interp` (en attente du
  convertisseur) et `fonts_text`, `interp_text` (sur `PDFTextDevice`,
  Python et Rust) ; corpus adverse `adv-interp` et rejeu `interp_unit`
  (résultats et mesures : `architecture.md`, « Vague 3 », « Réalisation »).
- **Vagues suivantes**, dans l'ordre des dépendances :
  1. `pdfinterp`, `pdfdevice`, `converter` (`PDFPageAggregator` vers
     `layout`) : étapes `fonts`, `interp`, `layout` (comptage des cadres de
     l'interpréteur, ordre des `id()` du layout) ;
  2. pdfplumber `page`, `pdf`, `container`, `structure`, `convert` :
     étapes `plumber_*`, en réutilisant `text`/`table` déjà portés ;
  3. l'API JS publique sur la couche page (`pdf.pages`, objets, texte,
     tables), sur l'infrastructure Wasm/JS déjà en place.
  Les questions ouvertes sont notées dans les en-têtes des modules, dans
  `architecture.md` et dans `js-runtime.md` (`search` suit la sémantique
  `RegExp` de JS, choix documenté ; `atan2` et le fournisseur de CMaps sont
  réglés).
