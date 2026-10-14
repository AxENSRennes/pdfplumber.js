# Expériences de décision : TypeScript ou Rust/Wasm

Deux expériences ont fondé le choix d'un cœur Rust compilé en Wasm
([`port-strategy.md`](port-strategy.md)). Pour chacune, un étage de
pdfminer/pdfplumber a été porté deux fois, en TypeScript et en Rust, avec le
même algorithme, vérifié au bit près contre Python, puis optimisé avec le même
effort. Mesures sur Mac M-series, médianes de 5 passes, en ms.

Le code (`experiments/psparser-bench`, `experiments/tables-bench`) a été retiré
du dépôt une fois la fondation Rust intégrée ; il reste lisible dans l'étiquette
`legacy-pdfjs`, par exemple
`git show legacy-pdfjs:experiments/tables-bench/rust/lib.rs`. Le prototype de
parsing est devenu `crates/pdfminer` ; celui des tables servira de point de
départ au port de `pdfplumber/table.py`.

## 1. Parsing des flux de contenu (`psparser.py` + `PDFContentParser`)

Le point chaud de pdfminer : 25 à 48 % du temps de pdfplumber selon le profil.
Un job est un ensemble de flux de contenu décodés ; Python écrit le SHA-256 de
la sérialisation canonique de tous les `(pos, obj)` de `nextobject()`,
exception finale comprise.

### Parité

| Corpus | Jobs | TS | Rust/Wasm |
|---|---|---|---|
| Réel : 1 072 PDF, 20 premières pages, pages + Form XObjects, 63,8 Mo | 3 459 | 3 459/3 459 | 3 459/3 459 |
| Adverse : tokens à cheval sur les blocs de 4 096 octets et les frontières de flux, échappements, images inline, 5 types d'exception | 4 000 | 4 000/4 000 | 4 000/4 000 |

Le corpus réel seul laissait passer des mutations (`BUFSIZ` à 4095,
échappement `\` + `\r\n`, `\n` inséré au changement de flux, limite de 4 300
chiffres) ; le corpus adverse les détecte toutes.

### Performance (63,8 Mo, 10,75 M objets)

Chaque mode passe chaque objet au même consommateur JS (comme la pile
d'opérandes de l'interpréteur).

| Mode | Node 26 | Chromium 148 | WebKit 26.4 | Firefox 151 |
|---|---|---|---|---|
| TS, premier jet | 1 992 | 1 765 | 701 | 1 903 |
| TS, optimisé | 555 | 575 | 487 | 1 273 |
| Wasm, objets décodés en JS | 568 | 586 | 520 | 745 |
| Wasm seul, rien ne revient au JS | 375 | 375 | 373 | 495 |

- Références : pdfminer sous CPython 17 225 ms (3,7 Mo/s) ; le même Rust en
  natif 289 ms (221 Mo/s), premier jet Rust 330 ms.
- Instanciation du Wasm : 2 à 3 ms. Taille : TS 12 Ko minifié (4,6 Ko gzip) ;
  Wasm 129 Ko (55 Ko gzip, sans `wasm-opt`).
- Le Firefox de Playwright (build Nightly) exécute ce Wasm environ 7× plus
  lentement (2 643 ms) : mesurer avec un vrai Firefox.

## 2. Détection de tables (`TableFinder`, stratégies `lines`/`lines_strict`)

Géométrie en flottants (`utils/clustering.py`, `utils/geometry.py`) :
beaucoup de bords entrent, peu de tables sortent. Trace comparée au bit près :
bords fusionnés, intersections, cellules, tables (bbox, cellules, `rows`).

### Parité (pdfplumber réel, Python 3.12)

| Corpus | Jobs | TS | Rust/Wasm |
|---|---|---|---|
| Réel : `page.edges` des 20 premières pages de 1 072 PDF (1 083 pages, 1,15 M bords, 718 tables) | 1 083 | 1 083/1 083 | 1 083/1 083 |
| Adverse : grilles aux limites des tolérances, segments à joindre, doublons, diagonales, int/float mêlés (495 000 bords, 4 824 tables) | 3 000 | 3 000/3 000 | 3 000/3 000 |

Toutes les mutations testées sont détectées, sauf `Math.min` à la place du
`min` Python, qui ne diffère que sur le signe de zéro, invisible dans la trace.

### Performance

- TS : objets bords → objets tables. Wasm hybride : bords JS copiés en mémoire
  Wasm, tables décodées en JS. Wasm cœur : bords déjà en mémoire Wasm (produits
  par le parseur Rust), tables décodées en JS.

Pages réelles (1 083 pages ; Python 2 214 ms ; Rust natif 29 ms) :

| Mode | Node 26 | Chromium 148 | WebKit 26.4 | Firefox 151 |
|---|---|---|---|---|
| TS, premier jet | 118 | 100 | 80 | 114 |
| TS, optimisé | 97 | 105 | 71 | 89 |
| Wasm hybride | 52 | 51 | 40 | 53 |
| Wasm cœur | 36 | 36 | 35 | 45 |

Pages denses en tables (corpus adverse, 3 000 pages ; Python 3 667 ms ; Rust
natif 113 ms) :

| Mode | Node 26 | Chromium 148 | WebKit 26.4 | Firefox 151 |
|---|---|---|---|---|
| TS, premier jet | 403 | 347 | 267 | 390 |
| TS, optimisé | 273 | 284 | 198 | 240 |
| Wasm hybride | 146 | 141 | 127 | 151 |
| Wasm cœur | 133 | 130 | 121 | 143 |

- Instanciation : 2 à 3 ms. Taille : TS 2,5 Ko gzip ; Wasm 180 Ko (47 Ko gzip,
  sérialiseur de trace inclus, sans `wasm-opt`).
- Optimisations communes aux deux ports : identifiants de bbox par bord,
  regroupement par tri d'indices, fenêtre de recherche des intersections avec le
  prédicat exact de Python. Après cette passe, le profil TS est plat (aucune
  fonction au-dessus de 10 %).

## Leçons

1. **La qualité d'implémentation pèse autant que le langage.** Le premier jet
   TS du parseur était 3,6× plus lent que sa version optimisée (tableaux
   recréés par objet, `length = 0`, chaîne construite par mot-clé). Ne jamais
   conclure sans profiler.
2. **La frontière JS/Wasm décide.** Au grain du token, le Wasm ne gagne rien
   sur V8 et JSC (égalité avec TS optimisé) et 1,7× sur Firefox. Quand seuls
   les résultats finaux traversent, il gagne 1,3 à 1,5× sur le parsing (2,6× sur
   Firefox) et 1,6 à 2,9× sur les tables. D'où l'API à gros grain.
3. **Le natif ouvre une seconde cible** : le même Rust est environ 60× plus
   rapide que CPython sur le parsing (module PyO3 possible).
4. **`sum()` de Python 3.12 est compensé (Neumaier).** Avec une somme naïve,
   380 pages réelles sur 1 083 ont des bords fusionnés différents, 219 des
   418 pages avec tables des coordonnées différentes au dernier bit, et une page
   une structure de table différente. La sortie de pdfplumber dépend donc de la
   version de Python : l'oracle est figé (3.12, contrat « Python ≥ 3.12 »).
5. **int contre float** : sans cette distinction, seuls 1 608 jobs adverses
   sur 3 000 restent identiques, mais aucune page réelle ne change (pdfplumber
   ne produit que des floats). Le port la garde (`pycompat::Num`).
6. **Le corpus adverse est indispensable** : c'est lui, pas le corpus réel, qui
   tue les mutations aux frontières (blocs de 4 096 octets, tolérances exactes,
   échappements).
7. En Python, les tables pèsent peu face au parsing (2,2 s contre environ 17 s) :
   le gain global d'un cœur Rust vient surtout du parsing et de l'interpréteur.
