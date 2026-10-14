# Vérification : traces canoniques, corpus, mutations

Comment on prouve qu'un module Rust se comporte comme pdfminer/pdfplumber.
Aucun module n'est « fini » sans les quatre preuves décrites en bas de page.

## Principe

Pour chaque étape du pipeline, Python (l'oracle) et Rust écrivent la **même
trace texte canonique** à partir de la même entrée. On compare les traces
octet pour octet. En cas d'écart, la première ligne différente localise le bug.
On ne compare jamais avec une tolérance numérique.

## Format canonique

Une trace est une suite de lignes UTF-8 terminées par `\n`. Chaque étape
définit ses lignes (préfixe d'étiquette, puis valeurs). Les valeurs sont
écrites par `canon(v)` :

| Valeur Python | Écriture | Exemples |
|---|---|---|
| `None`, `True`, `False` | tel quel | `None` |
| `int` (taille quelconque) | décimal | `-12`, `0` |
| `float` | `repr()` Python | `1.0`, `0.1`, `1e-05`, `1e+16`, `-0.0`, `inf`, `nan` |
| `str` | `json.dumps(s, ensure_ascii=True)` | `"café"` |
| `bytes` (≤ 256 octets) | `x"` hexa minuscule `"` | `x"0d0a"` |
| `bytes` (> 256 octets) | `x#` longueur `:` 16 premiers hexa du SHA-256 | `x#5120:9f86d081884c7d65` |
| `tuple` | `(a, b)` | `(1.0, 2)` ; vide `()` |
| `list` | `[a, b]` | `[]` |
| `dict` | `{k: v, …}` dans l'ordre d'insertion | `{"Type": /"Page"}` |
| `PSLiteral` | `/` + canon(nom) (nom `str` ou `bytes`) | `/"F1"`, `/x"ff"` |
| `PSKeyword` | `K` + canon(nom bytes) | `Kx"4254"` |
| `PDFObjRef` | `R` + objid | `R12` |
| `PDFStream` | `Stream(objid, genno, attrs, raw)` | `Stream(5, 0, {"Length": 42}, x#42:…)` |
| exception | `!` + nom de classe + `(` + json(str(e)) + `)` | `!PSEOF("Unexpected EOF")` |

- Le séparateur est toujours `", "` ; les clés de `dict` passent aussi par
  `canon`. Les objets propres à une étape (`LTChar`, cellule de table…) sont
  écrits comme un `dict` préfixé par le nom de la classe : `LTChar{…}`.
- `canon` existe en Python (`oracle/canon.py`) et en Rust
  (`crates/pdftrace/src/canon.rs`). Les deux sont testés l'un contre l'autre
  sur des valeurs générées (flottants extrêmes, sous-normaux, Unicode, octets).
- Une trace finit par une ligne `!end` si l'étape s'est terminée normalement,
  ou par la ligne d'exception qui l'a interrompue. Une exception rattrapée dans
  une boucle (un objet illisible parmi d'autres) est écrite à sa place.

## Étapes

Chaque étape a un nom stable, une implémentation Python
(`oracle/stages/<nom>.py`) et une implémentation Rust
(`crates/pdftrace/src/stages/<nom>.rs`). La liste vivante est
`oracle/stages/` ; chaque fichier documente ses lignes en tête.

| Étape | Ce qui est tracé (par PDF) |
|---|---|
| `psparser` | objets `(pos, obj)` produits par `PDFContentParser` sur les flux de chaque page et Form XObject |
| `objects` | xref, trailer, chaque objet indirect résolu (`getobj`), erreurs comprises |
| `streams` | chaque flux : filtres, paramètres, données décodées (`x#…`) ou exception |
| `pages` | arbre des pages : attributs hérités, boîtes, rotation, ressources, contenus |
| `fonts` | chaque police utilisée : type, métriques, et pour chaque code rencontré `decode`, `to_unichr`, `char_width`, `char_disp` |
| `interp` | appels au périphérique (`begin_figure`, `paint_path`, `render_char`, `render_image`, balises) avec l'état graphique |
| `layout` | arbre `LTPage` complet, avec et sans `laparams` |
| `plumber_objects` | `page.objects` de pdfplumber, tous attributs, plus boîtes et `doctop` |
| `plumber_text` | `extract_text` (simple, `layout=True`, options), `extract_words`, `extract_text_lines`, `search`, `dedupe_chars` |
| `plumber_tables` | `find_tables`/`extract_tables` pour les stratégies `lines`, `lines_strict`, `text`, `explicit` |
| `plumber_doc` | métadonnées, annotations, liens, arbre de structure, `crop`/`within_bbox`/`outside_bbox` |

Les modules sans PDF en entrée (filtres, inflate, chiffrement, parseur de
CMap, analyse de mise en page, tables à partir de bords) ont aussi des **tests
différentiels unitaires** : `oracle/unit/<module>.py` génère des cas (graine
fixe) et les sorties Python, Rust les rejoue (`cargo test`).

## Outils

- `oracle/trace.py --stage S [--password P] FICHIER.pdf` : trace Python sur
  la sortie standard.
- `pdftrace --stage S [--password P] FICHIER.pdf` (binaire Rust) : même chose.
  `pdftrace --stage S --out DIR F1.pdf F2.pdf …` écrit `DIR/<sha256 du PDF>.txt`
  pour une liste de fichiers.
- `oracle/verify.py --stage S --set NOM [-j N]` : construit au besoin le cache
  de l'oracle, lance `pdftrace`, compare, puis affiche le décompte et, pour
  chaque fichier en écart, la première ligne différente avec du contexte.
- Cache : `.cache/oracle/<étape>/<empreinte de l'étape>/<sha256 du PDF>.txt`.
  L'empreinte de l'étape couvre le code Python de l'étape et `canon.py`, donc un
  changement de l'étape invalide le cache. Écriture atomique (fichier
  temporaire puis renommage), délai maximal par fichier (le dépassement est
  écrit `!Timeout` et exclu de la comparaison, et compté à part).
- Racine du corpus : la variable `PDFPLUMBER_ROOT` (par défaut la racine du
  dépôt) donne l'emplacement de `corpus/`, `upstream/`, `.venv` et `.cache`.
  Dans un worktree Git, elle pointe vers le dépôt principal, qui contient les
  PDF téléchargés et le venv.

## Ensembles de PDF (`oracle/corpus.py`)

| Nom | Contenu |
|---|---|
| `micro` | `corpus/micro` (générés, petits, chacun vise une fonctionnalité) |
| `upstream` | `upstream/pdfplumber-python/tests/pdfs`, `upstream/pdfminer-six/samples`, `corpus/upstream-contract` |
| `real` | `corpus/external`, `corpus/external-holdout`, `corpus/parity-cycles` |
| `pdfjs` | `upstream/pdfjs/test/pdfs` (937 PDF, beaucoup de cas tordus) |
| `quick` | échantillon déterministe d'environ 150 PDF tirés de tous les ensembles |
| `all` | tout ce qui précède |
| `adv-<étape>` | corpus adverse généré pour une étape (voir plus bas) |

Par défaut, une étape page par page s'arrête aux 20 premières pages d'un
fichier (`--max-pages`). La passe finale couvre toutes les pages.

## Corpus adverses

Le corpus réel ne voit pas les frontières : blocs de 4 096 octets, tolérances
exactement atteintes, échappements, ints contre floats, objets manquants ou
mal typés. Pour chaque étape, `oracle/adversarial/<étape>.py` génère des PDF
(ou des entrées unitaires) ciblant ces frontières, avec une graine fixe, dans
`.cache/corpus/adv-<étape>/`. Les anciens tests « pièges » de l'ancien port,
qui interrogeaient Python, sont une bonne source d'idées
(`git show legacy-pdfjs:test/lowlevel/`).

## Mutations

`cargo mutants` (crate `cargo-mutants`) mute le code Rust d'un module
(`<` en `<=`, constante changée, branche supprimée…) et relance les tests de
parité. **Toute mutation survivante** est soit tuée par un nouveau cas
adverse, soit justifiée par écrit (mutation équivalente, avec preuve). La
commande et le résultat sont notés dans le rapport du module.

## Définition de « fini » pour un module

1. Port fidèle : un module Rust par module Python, références upstream sur
   chaque fonction, aucun cas spécial.
2. Parité sur `all` (20 premières pages) pour ses étapes : 100 % identique, hors
   `!Timeout` de l'oracle (listés).
3. Corpus adverse de l'étape : 100 % identique.
4. Mutations : aucune survivante non justifiée.

La version Wasm est vérifiée par les mêmes traces (le binaire de trace tourne
aussi sous Node via le Wasm) sur l'ensemble `quick`.
