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

- Les cas sont écrits dans `$PDFPLUMBER_ROOT/.cache/unit/<module>/`
  (`<nom>.jsonl`, une ligne `{"in": [...], "out": ...}` au format typé
  ci-dessous), avec `meta.json` qui contient le SHA-256 du générateur. Les
  tests Rust (`crates/pdftrace/tests/<module>_unit.rs`, via
  `pdftrace::unit::UnitData`) échouent si les données manquent ou viennent
  d'une autre version du générateur, en affichant la commande qui les
  régénère.
- Générateurs actuels : `pycompat.py` (sémantique CPython, bibliothèque
  standard seule), `exceptions.py` (classes d'exception de pdfminer et
  pdfplumber), `pdftypes.py` (modèle d'objets, références, flux,
  déchiffrement), `psparser_base.py` (`PSStackParser` sur un fichier :
  `nexttoken`, `nextline`, `revreadlines`, `seek`, `pop`), `psparser.py`
  (corpus réel de flux de contenu et corpus adverse `--fuzz 4000` pour
  `PDFContentParser`, comparés job par job par empreinte de la
  sérialisation canonique).

```sh
export PDFPLUMBER_ROOT=/chemin/du/dépôt/principal   # .venv, corpus/, .cache/
for m in pycompat exceptions pdftypes psparser_base; do $PDFPLUMBER_ROOT/.venv/bin/python oracle/unit/$m.py; done
$PDFPLUMBER_ROOT/.venv/bin/python oracle/unit/psparser.py --fuzz 4000
$PDFPLUMBER_ROOT/.venv/bin/python oracle/unit/psparser.py          # corpus réel, ~1 min
cargo test --release
```

### Format d'échange typé (entrées et sorties des tests unitaires)

Les cas unitaires sont des fichiers JSON Lines. Les valeurs Python y sont
encodées sans perte, en gardant la distinction int/float et tuple/list :

| Valeur Python | JSON |
|---|---|
| `None`, `True`, `False`, `str` | `null`, `true`, `false`, chaîne JSON |
| `int` | `{"i": "<décimal>"}` |
| `float` | `{"f": "<16 hexa des bits IEEE-754, gros-boutiste>"}` |
| `bytes` | `{"b": "<hexa>"}` |
| `list` | tableau JSON |
| `tuple` | `{"t": [ … ]}` |
| `dict` | `{"d": [[clé, valeur], …]}` (ordre d'insertion) |
| `PSLiteral`, `PSKeyword` | `{"lit": <str ou {"b": …}>}`, `{"kw": "<hexa>"}` |
| `PDFObjRef` | `{"ref": objid}` |
| exception | `{"exc": "<classe>", "msg": "<str(e)>"}` |

`oracle/typed_json.py` et `crates/pdftrace/src/typed_json.rs` implémentent ce
format des deux côtés.

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

Une mutation peut faire boucler le code sans consommer son entrée, souvent en
allouant : sur la machine partagée, un mutant a atteint 16,5 Go. D'où trois
règles :

- **Plafond mémoire** : chaque `cargo mutants`, génération de corpus adverse
  ou passe sur le corpus complet tourne dans
  `systemd-run --user --scope --quiet -p MemoryMax=5G -p MemorySwapMax=0 …`,
  avec `-j 1 --timeout 20` pour `cargo mutants`, et jamais deux travaux lourds
  à la fois. Les tests corrects restent sous 250 Mo.
- **Profil `mutants`** (`--profile mutants`) : `release` avec les
  `debug_assertions`, qui activent `psparser::ProgressGuard`. Entre deux
  `seek`, un parseur ne lit qu'en avant (position
  `PSStackParserExt::read_position`) ; une boucle qui ne progresse plus
  panique en quelques pas au lieu d'épuiser la mémoire. Le code du garde est
  exclu des mutations (`-E "ProgressGuard|check_progress|read_position"`),
  ses mutants étant équivalents par
  construction.
- **Bornes dans les tests de rejeu** : un job ne peut pas produire plus
  d'objets que pdfminer (`psparser_unit`), ni plus de tokens ou de lignes que
  d'octets en entrée (`psparser_base_unit`).

```sh
systemd-run --user --scope --quiet -p MemoryMax=5G -p MemorySwapMax=0 \
  cargo mutants -f crates/pdfminer/src/psparser.rs -E "ProgressGuard|check_progress|read_position" \
  --test-package pdftrace --cargo-test-arg=--test=psparser_unit \
  --profile mutants -j 1 --timeout 20
```

## Définition de « fini » pour un module

1. Port fidèle : un module Rust par module Python, références upstream sur
   chaque fonction, aucun cas spécial.
2. Parité sur `all` (20 premières pages) pour ses étapes : 100 % identique, hors
   `!Timeout` de l'oracle (listés).
3. Corpus adverse de l'étape : 100 % identique.
4. Mutations : aucune survivante non justifiée.

La version Wasm est vérifiée par les mêmes traces (le binaire de trace tourne
aussi sous Node via le Wasm) sur l'ensemble `quick`.
