# Vérification : traces canoniques, corpus, mutations

Comment on prouve qu'un module Rust se comporte comme pdfminer/pdfplumber.
Aucun module n'est « fini » sans les quatre preuves décrites en bas de page.

## Principe

Pour chaque étape du pipeline, Python (l'oracle) et Rust écrivent la **même
trace texte canonique** à partir de la même entrée. On compare les traces
octet pour octet. En cas d'écart, la première ligne différente localise le bug.
On ne compare jamais avec une tolérance numérique.

## Format canonique

Une trace est une suite de lignes terminées par `\n`, en ASCII pur (tout ce
qui n'est pas ASCII passe par les échappements JSON). Chaque étape définit ses
lignes (préfixe d'étiquette, puis valeurs). Les valeurs sont écrites par
`canon(v)` :

| Valeur Python | Écriture | Exemples |
|---|---|---|
| `None`, `True`, `False` | tel quel | `None` |
| `int` (taille quelconque) | décimal | `-12`, `0` |
| `float` | `repr()` Python | `1.0`, `0.1`, `1e-05`, `1e+16`, `-0.0`, `inf`, `nan` |
| `str` | `json.dumps(s, ensure_ascii=True)` | `"caf\u00e9"` |
| `bytes` (≤ 256 octets) | `x"` hexa minuscule `"` | `x"0d0a"` |
| `bytes` (> 256 octets) | `x#` longueur `:` 16 premiers hexa du SHA-256 | `x#5120:9f86d081884c7d65` |
| `tuple` | `(a, b)` ; un élément `(a)` | `(1.0, 2)` ; vide `()` |
| `list` | `[a, b]` | `[]` |
| `dict` | `{k: v, …}` dans l'ordre d'insertion | `{"Type": /"Page"}` |
| `PSLiteral` | `/` + canon(nom) (nom `str` ou `bytes`) | `/"F1"`, `/x"ff"` |
| `PSKeyword` | `K` + canon(nom bytes) | `Kx"4254"` |
| `PDFObjRef` | `R` + canon(objid) | `R12` |
| `PDFStream` | `Stream(objid, genno, attrs, raw)`, `raw` = canon(`rawdata`) | `Stream(5, 0, {"Length": 300}, x#300:…)` |
| exception | `!` + nom de classe + `(` + json(str(e)) + `)` | `!PSEOF("Unexpected EOF")` |
| objet d'étape | nom + canon(dict des champs) | `LTChar{"x0": 1.0}` |

- Le séparateur est toujours `", "` ; les clés de `dict` passent aussi par
  `canon`. Les objets propres à une étape (`LTChar`, cellule de table…) sont
  écrits comme un `dict` préfixé par le nom de la classe : `LTChar{…}`
  (`oracle.canon.Named`).
- `bool` est testé avant `int`. Tous les NaN s'écrivent `nan` (signe et charge
  utile ignorés, comme `repr`). Un `int` dépassant la limite de 4 300 chiffres
  de `str()` est écrit en décimal quand même (la limite globale n'est jamais
  modifiée : pdfminer la subit comme chez un utilisateur).
- `str` : un caractère hors BMP devient une paire d'échappements
  (`"\ud83d\ude00"`), un surrogate isolé un seul échappement (`"\ud800"`) ;
  une chaîne Python faite des deux surrogates d'une paire s'écrit donc comme
  le caractère correspondant.
- `PDFStream` : `rawdata` vaut `None` une fois le flux décodé (`decode()` le
  vide), et `raw` s'écrit alors `None`. C'est voulu : la trace montre l'état
  du cache de décodage, que le port reproduit (`get_rawdata()` est public).
  Chaque étape fixe donc l'ordre de ses accès. Pour un document chiffré,
  `raw` est le contenu stocké, encore chiffré.
- Exception : le nom est `type(e).__name__` pour les classes de `builtins`,
  pdfminer, pdfplumber et `oracle` ; sinon `module.nom` (`zlib.error`,
  `struct.error`, `binascii.Error`). Dans le message, toute adresse de repr
  par défaut (` at 0x7f3a…>`) devient ` at 0x?>`.
- `canon` existe en Python (`oracle/canon.py`) et en Rust
  (`crates/pdftrace/src/canon.rs`). Les deux sont testés l'un contre l'autre
  sur des valeurs générées : `oracle/unit/canon.py` écrit
  `.cache/unit/canon/cases.jsonl` (≈ 6 300 cas, graine fixe), une ligne
  `{"value": <JSON typé>, "canon": "<texte attendu>"}` par cas.
- `canon` est itératif (profondeur quelconque). Une structure cyclique ou un
  type inconnu lève `CanonError`, qui dérive de `BaseException` : une erreur du
  harnais n'est jamais rattrapée par le `except Exception` d'une étape ni
  écrite dans une trace comme si pdfminer l'avait levée.
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
| `objects` | xref (type, trailer, `get_pos` de chaque entrée), catalogue, info, chiffrement, puis chaque objet indirect résolu (`getobj`), erreurs comprises |
| `streams` | chaque flux : filtres, paramètres, données décodées (`x#…`) ou exception |
| `pages` | arbre des pages : attributs hérités, boîtes, rotation, ressources, contenus, libellés ; boîtes de la `Page` pdfplumber (TrimBox/BleedBox/ArtBox comprises), `doctop` |
| `fonts` | chaque police utilisée : type, métriques, et pour chaque code rencontré `decode`, `to_unichr`, `char_width`, `char_disp` |
| `interp` | appels au périphérique (`begin_figure`, `paint_path`, `render_char`, `render_image`, balises) avec l'état graphique |
| `layout` | arbre `LTPage` complet, avec et sans `laparams` |
| `plumber_objects` | `page.objects` de pdfplumber, tous attributs, plus boîtes et `doctop` |
| `plumber_text` | `extract_text` (simple, `layout=True`, options), `extract_words`, `extract_text_lines`, `search`, `dedupe_chars` |
| `plumber_tables` | `find_tables`/`extract_tables` pour les stratégies `lines`, `lines_strict`, `text`, `explicit` |
| `plumber_doc` | métadonnées, annotations, liens, arbre de structure, `crop`/`within_bbox`/`outside_bbox` |

En tête de chaque `oracle/stages/<nom>.py` : les modules upstream exercés, le
parcours exact (ordre des appels à `getobj`/`get_data`, qui a des effets de
bord par le cache) et le format de chaque ligne. Règles communes :

- Le document s'ouvre comme dans pdfplumber : `PDFDocument(PDFParser(open(f,
  "rb")), password=password or "")` (`caching=True`, `fallback=True`,
  `STRICT=False`). Pas de rustine ni de crochet sur pdfminer/pdfplumber.
- Une étape tourne dans un fil d'exécution neuf : son générateur commence
  toujours à la même profondeur de pile Python (`STAGE_ENTRY_DEPTH = 5` cadres
  en dessous), donc une `RecursionError` (arbre de pages ou tableaux très
  profonds) arrive au même endroit quel que soit l'appelant. Limite de
  récursion par défaut (1000) ; la pile C du fil fait 256 Mio pour qu'une
  récursion C profonde finisse en `RecursionError` et non en erreur de
  segmentation (la limite compte des cadres, pas des octets : sans effet sur
  les traces).
- La journalisation (`logging`) est coupée pendant une étape : pdfminer ne
  lit jamais son niveau de log, cela ne change aucun résultat.
- Une étape par page traite les `--max-pages` premières pages (20 par défaut,
  0 = toutes), créées paresseusement : le générateur `create_pages` n'est plus
  relancé après la dernière page demandée.

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
| `str` contenant un surrogate (U+D800–U+DFFF) | `{"s": [<point de code>, …]}` (UTF-8 ne peut pas le porter) |
| `int` | `{"i": "<décimal>"}` |
| `float` | `{"f": "<16 hexa des bits IEEE-754, gros-boutiste>"}` |
| `bytes` | `{"b": "<hexa>"}` |
| `list` | tableau JSON |
| `tuple` | `{"t": [ … ]}` |
| `dict` | `{"d": [[clé, valeur], …]}` (ordre d'insertion) |
| `PSLiteral`, `PSKeyword` | `{"lit": <str ou {"b": …}>}`, `{"kw": "<hexa>"}` |
| `PDFObjRef` | `{"ref": objid}` (entier JSON si l'objid est un `int` tenant dans un i64, sinon valeur typée) |
| `PDFStream` | `{"stream": {"objid": v, "genno": v, "attrs": {"d": …}, "rawdata": v, "data": v}}` (valeurs typées, `rawdata`/`data` éventuellement `null`) |
| exception | `{"exc": "<classe>", "msg": "<str(e)>"}` (classe nommée comme dans `canon`) |
| objet d'étape | `{"named": "<nom>", "d": [[clé, valeur], …]}` |

Chaque objet JSON a une seule clé, sauf `exc` (+ `msg`) et `named` (+ `d`).
`dumps` écrit un JSON compact, ASCII, clés dans l'ordre ci-dessus : on peut
comparer un ré-encodage au texte d'origine. Une exception décodée devient
`oracle.canon.ExcValue`, que `canon` écrit comme l'exception d'origine.

`oracle/typed_json.py` et `crates/pdftrace/src/typed_json.rs` implémentent ce
format des deux côtés.

## Outils

Mode d'emploi détaillé : [`oracle/README.md`](../oracle/README.md).

- `python -m oracle.trace --stage S [--password P] [--max-pages N] FICHIER.pdf` :
  trace Python sur la sortie standard (`--max-pages` vaut 20 par défaut, 0 =
  toutes les pages).
- `pdftrace --stage S [--password P] [--max-pages N] FICHIER.pdf` (binaire
  Rust) : même chose, mêmes valeurs par défaut.
  `pdftrace --stage S [--password P] [--max-pages N] --out DIR F1.pdf F2.pdf …`
  écrit `DIR/<sha256 du PDF>.txt` pour une liste de fichiers. Code de sortie 2 :
  étape non implémentée.
- `python -m oracle.verify --stage S --set NOM [-j N] [--max-pages N]
  [--pdftrace CHEMIN] [--limit K] [--files …] [--json]` : construit au besoin
  le cache de l'oracle (`-j` processus, 3 par défaut), lance `pdftrace` par lots
  de fichiers de même mot de passe, compare octet pour octet, puis affiche le
  décompte (identiques / différents / plantages Rust / délais oracle / erreurs
  oracle / total) et, pour chaque fichier en écart, son chemin et la première
  ligne différente avec 3 lignes de contexte de chaque côté. Un lot qui échoue
  est rejoué fichier par fichier pour isoler le coupable. `--json` donne un
  résumé lisible par machine ; `--oracle-only` ne fait que remplir le cache.
  Code de sortie : 0 si tout est identique (hors délais), 1 sinon, 2 si
  l'étape n'existe pas côté Rust ou si le binaire manque.
- Cache : `.cache/oracle/<étape>/<empreinte>/<sha256 du PDF>.txt`
  (`<sha256>.pw-<12 hexa du SHA-256 du mot de passe>.txt` avec un mot de
  passe). L'empreinte couvre les sources de l'étape (et des étapes de son `DEPENDS`), de `stages/__init__.py`,
  `stages/_common.py`, `canon.py` et `typed_json.py`, les versions de Python,
  pdfminer.six et pdfplumber, et `--max-pages` pour une étape par page : tout
  changement invalide le cache. Écriture atomique (fichier temporaire puis
  renommage). Chaque trace est calculée dans son propre processus ; au-delà de
  120 s (`--timeout`), le fichier reçoit la trace `!Timeout`, exclue de la
  comparaison et comptée à part (`--retry-timeouts` la recalcule).
- Racine du corpus : la variable `PDFPLUMBER_ROOT` (par défaut la racine du
  dépôt) donne l'emplacement de `corpus/`, `upstream/` et `.cache`
  (`PDFPLUMBER_CACHE` peut désigner un autre cache). Dans un worktree Git, elle
  pointe vers le dépôt principal, qui contient les PDF téléchargés et le venv.

## Ensembles de PDF (`oracle/corpus.py`)

| Nom | Contenu |
|---|---|
| `micro` | `corpus/micro` (générés, petits, chacun vise une fonctionnalité) |
| `upstream` | `upstream/pdfplumber-python/tests/pdfs`, `upstream/pdfminer-six/samples`, `corpus/upstream-contract` |
| `real` | `corpus/external`, `corpus/external-holdout`, `corpus/parity-cycles` |
| `pdfjs` | `upstream/pdfjs/test/pdfs` (937 PDF, beaucoup de cas tordus) |
| `quick` | tout `micro`, plus les fichiers de `all` dont le SHA-256 commence par un octet < 18 (~7 %, 149 PDF aujourd'hui) : l'appartenance ne dépend que du contenu du fichier, en ajouter n'en retire aucun |
| `all` | tout ce qui précède (1 852 PDF distincts aujourd'hui) |
| `adv-<étape>` | corpus adverse généré pour une étape (voir plus bas) |

Un fichier est tout `*.pdf` (casse ignorée) des dossiers, récursivement, en
ordre de chemin relatif. Les fichiers identiques sont dédoublonnés par SHA-256
(la première occurrence l'emporte, dans l'ordre du tableau).

Mots de passe, par SHA-256 du fichier : `corpus/passwords.json` (fichiers de
test upstream, chaque entrée cite la ligne du test upstream qui l'ouvre),
puis les champs `password`/`openOptions.password` des manifestes
(`corpus/**/manifest.json`, `*-manifest.json`, dont `corpus/micro/manifest.json`
écrit par `oracle/generate_micro_pdfs.py`), puis
`upstream/pdfjs/test/test_manifest.json`. `python -m oracle.corpus --check`
signale les conflits.

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
