# Détection de tables : TS vs Rust/Wasm

Seconde expérience de décision, après `../psparser-bench` (parsing). On porte
`TableFinder` de pdfplumber (stratégies `lines` / `lines_strict`, avec
`utils/clustering.py` et `utils/geometry.py`) dans les deux langages, avec le
même algorithme, puis on mesure. Cet étage est de la géométrie en flottants :
regroupement, tris, intersections et cellules. Les résultats qui reviennent au
JS sont peu nombreux.

## Parité (oracle : pdfplumber réel, Python 3.12.13)

On compare la **trace complète** de `TableFinder` : bords fusionnés avec largeur
et hauteur, points d'intersection avec le nombre de bords, cellules, tables
avec bbox, cellules et `rows`. Chaque nombre est comparé au bit près.

| Corpus | Jobs | TS | Rust/Wasm |
|---|---|---|---|
| Réel : `page.edges` des 20 premières pages de 1 072 PDF (1 083 pages avec des bords, 1,15 M bords, 718 tables) | 1 083 | 1 083/1 083 identiques | 1 083/1 083 identiques |
| Adverse généré : grilles aux limites des tolérances, segments à joindre, doublons, bords diagonaux, mélanges int/float, réglages variés (495 000 bords, 4 824 tables) | 3 000 | 3 000/3 000 identiques | 3 000/3 000 identiques |

Toutes les mutations testées sont détectées (`<` à la place de `<=` dans les
jointures et intersections, ordre de tri des tables, calcul de la hauteur,
regroupement, etc.), sauf `Math.min` à la place du `min` Python. Ce dernier ne
diffère que sur le signe de zéro, que la sérialisation ne distingue pas.

### Sémantique numérique de Python : ce qui compte vraiment

`snap_objects` moyenne les coordonnées avec `sum()`. Depuis **Python 3.12**,
`sum()` utilise une sommation compensée (Neumaier) pour les floats ; les ints
sont additionnés exactement et sans compensation.

- **Somme naïve en JS** (`a + b + c`) : 380 pages réelles sur 1 083 ont des bords
  fusionnés différents, **219 des 418 pages avec tables** ont des coordonnées
  de tables différentes au dernier bit, et **1 page** a une structure de table
  différente.
- Conséquence : la sortie de pdfplumber dépend de la version de Python, puisque
  la 3.11 faisait une somme naïve. L'oracle doit fixer sa version.
- **Distinction int/float** : elle change le résultat sur le corpus adverse
  (1 608/3 000 jobs identiques sans elle), mais **aucune page réelle** n'est
  concernée, car pdfplumber ne produit que des floats.

## Performance (médiane de 5 passes, ms)

Chaque mode renvoie les mêmes tables sous forme d'objets JS (vérifié avant la
mesure).

- **TS** : objets bords → objets tables.
- **Wasm hybride** : objets bords JS emballés dans la mémoire Wasm, puis tables
  décodées en objets JS.
- **Wasm cœur** : les bords sont déjà dans la mémoire Wasm, comme si le parseur
  Rust les avait produits ; les tables sont décodées en objets JS.

**Pages réelles** (1 083 pages ; Python 2 214 ms ; Rust natif 29 ms) :

| Mode | Node 26 | Chromium 148 | WebKit 26.4 | Firefox 151 (installé) |
|---|---|---|---|---|
| TS, premier jet | 118 | 100 | 80 | 114 |
| TS, optimisé | 97 | 105 | 71 | 89 |
| Wasm hybride | 52 | 51 | 40 | 53 |
| Wasm cœur | 36 | 36 | 35 | 45 |

**Pages denses en tables** (corpus adverse, 3 000 pages ; Python 3 667 ms ; Rust natif 113 ms) :

| Mode | Node 26 | Chromium 148 | WebKit 26.4 | Firefox 151 (installé) |
|---|---|---|---|---|
| TS, premier jet | 403 | 347 | 267 | 390 |
| TS, optimisé | 273 | 284 | 198 | 240 |
| Wasm hybride | 146 | 141 | 127 | 151 |
| Wasm cœur | 133 | 130 | 121 | 143 |

- Instanciation du Wasm : 2 à 3 ms.
- Taille : TS 2,5 Ko gzip ; Wasm 180 Ko (47 Ko gzip), sérialiseur de trace
  inclus et sans `wasm-opt`.
- Les deux ports ont reçu les mêmes optimisations d'algorithme : identifiants
  de bbox par bord, regroupement par tri d'indices, fenêtre de recherche pour
  les intersections avec le prédicat exact de Python conservé. Après cette
  passe, le profil TS est plat : aucune fonction ne dépasse 10 %.

## Lecture

1. Sur cet étage, le Wasm gagne nettement : **1,6 à 2,1× en hybride**, et
   jusqu'à 2,9× quand les bords sont déjà en mémoire Wasm. La frontière y est
   naturellement grossière : beaucoup de bords entrent, peu de tables sortent.
2. L'écart restant vient du modèle de données (objets JS, tris par comparateur)
   plus que d'un défaut d'implémentation : le profil TS est plat.
3. En Python, cet étage pèse peu face au parsing (2,2 s ici contre environ 17 s
   pour parser les flux de contenu). Le gain global d'un cœur Rust dépendra
   donc surtout de l'étage de parsing.

## Reproduire

```sh
.venv/bin/python experiments/tables-bench/dump_tables.py               # corpus réel + oracle (~85 s)
.venv/bin/python experiments/tables-bench/dump_tables.py --fuzz 3000     # corpus adverse + oracle
(cd experiments/tables-bench/rust && ~/.cargo/bin/cargo build --release --target wasm32-unknown-unknown \
  && cp target/wasm32-unknown-unknown/release/tables.wasm tables.wasm)
npx tsx experiments/tables-bench/ts/verify.ts ts --corpus fuzz      # ou: wasm, --corpus corpus, --no-ints
npx tsx experiments/tables-bench/ts/bench-node.ts
BROWSERS=chromium,webkit,firefox-plain FIREFOX_BIN=/Applications/Firefox.app/Contents/MacOS/firefox \
  node experiments/tables-bench/run-browsers.mjs
(cd experiments/tables-bench/rust && ~/.cargo/bin/cargo run --release --example bench)
```

Fichiers : `ts/table.ts` (TS optimisé), `ts/table-v1.ts` (premier jet),
`rust/lib.rs` (Rust optimisé), `rust/lib-v1.rs` (premier jet Rust, non compilé),
`ts/wasm.ts` (chargeur, emballage des bords, décodage des tables), `ts/canon.ts`
(sérialisation de trace partagée avec Python).
