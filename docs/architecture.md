# Architecture du port Rust/Wasm

Contrat commun à tout le code du dépôt. À lire avec
[`port-strategy.md`](port-strategy.md) (le pourquoi) et
[`verification.md`](verification.md) (comment on prouve la parité).

## Oracle et sources portées

- L'oracle est **pdfplumber 0.11.9** (`upstream/pdfplumber-python`, commit
  `9804153`) sur **pdfminer.six 20260107** (`upstream/pdfminer-six`, tag
  `20260107`), exécutés par **CPython 3.12** (`.venv`, voir `.python-version`).
  Le code porté est exactement le code exécuté par l'oracle : ne jamais porter
  depuis une autre version.
- La sortie de pdfplumber est identique sous 3.12, 3.13 et 3.14, mais diffère
  sous 3.11 (`sum()` compensé, voir `port-strategy.md`). Le contrat public est
  « identique à pdfplumber sous Python ≥ 3.12 ». Les derniers bits de
  `float ** float` viennent de la bibliothèque C : la référence est glibc sur
  un processeur avec FMA (variante `__pow_fma`, celle de l'oracle) ; la
  variante sans FMA de la même glibc diffère sur environ 0,05 % des entrées,
  une autre libm (macOS, `libm` de FreeBSD) bien plus souvent.

## Organisation

```
Cargo.toml                 workspace
crates/
  pycompat/                sémantique Python partagée (nombres, sum, repr, round, tri, exceptions)
  pdfminer/                port de pdfminer.six, un fichier Rust par module Python
  pdfplumber/              port de pdfplumber, même structure (utils/ compris)
  pdftrace/                sérialisation canonique + binaire `pdftrace` (traces par étape)
  wasm/                    API à gros grain pour JavaScript (cdylib wasm32)
js/                        paquet npm : chargeur Wasm, API publique TS, types, tests Node et navigateur
oracle/                    côté Python : traces canoniques, cache, générateurs de corpus adverses, vérificateur
corpus/                    PDF réels et manifestes de provenance (les gros PDF sont téléchargés, pas versionnés)
upstream/                  sous-modules : pdfminer-six, pdfplumber-python, pdfjs (corpus de tests seulement)
docs/                      stratégie, architecture, vérification, API publique
```

## Règles de port

1. **Un module Python → un module Rust du même nom** (`pdfminer/psparser.py` →
   `crates/pdfminer/src/psparser.rs`, `pdfplumber/utils/text.py` →
   `crates/pdfplumber/src/utils/text.rs`). Un module trop gros peut devenir un
   dossier (`pdfdocument/mod.rs` + `pdfdocument/security.rs`), en le disant dans
   l'en-tête.
2. **Mêmes noms** de types, fonctions et méthodes (en `snake_case` / `CamelCase`
   Rust). Chaque fonction ou méthode porte en commentaire sa référence upstream,
   la ligne de son `def` suivie de son nom qualifié :
   `// pdfminer/pdfinterp.py:1282 PDFPageInterpreter.do_Tj` (une plage cite
   tous les noms qu'elle couvre : `:327-329 C.A, C.B, C.C`).
   `python -m oracle.upstream_refs` (lancé par `scripts/check.sh`) vérifie
   chacune de ces références, dans `crates/` et `oracle/`.
3. **Même algorithme, même ordre d'évaluation**, mêmes cas d'erreur. Les
   optimisations sont permises si la trace reste identique ; elles sont
   commentées et testées par mutation.
4. **Aucun cas spécial** : pas de nom de police, d'empreinte de fichier, de
   seuil ou de constante sans contrepartie upstream citée. Un écart se corrige à
   sa cause.
5. Les parties de pdfminer/pdfplumber **hors périmètre** (sorties HTML/XML/hOCR
   de `converter.py`, `high_level.py`, `image.py`/ImageWriter, `display.py`
   et le rendu, `repair.py` qui appelle Ghostscript, `cli.py`) ne sont pas
   portées. Tout le reste de ce qu'utilise `pdfplumber.open(...)` l'est,
   y compris `laparams`, la structure balisée, les annotations, le chiffrement.

## Modèle de valeurs

Python est dynamique ; le port reproduit cette dynamique là où elle est
observable, et seulement là.

- **Objets PDF** (`pdfminer.pdftypes`/`psparser`) : un `enum PDFObject`
  (`Null`, `Bool`, `Int(PyInt)`, `Float(f64)`, `Bytes`, `Literal` (PSLiteral,
  nom `str` ou `bytes` comme en Python), `Keyword`, `List`, `Dict` (ordonné,
  clés `str`), `Stream` (`Rc<PDFStream>`), `Ref` (`PDFObjRef`)). Les
  `PSLiteral`/`PSKeyword` sont internés comme en Python (identité).
- **Nombres** : Python distingue `int` et `float`, et cette distinction change
  des résultats (`sum()`, `int/int`, `repr`). `pycompat::PyInt` (i64, avec
  repli entier arbitraire) et `pycompat::Num` (`Int`/`Float`) suivent les règles
  d'arithmétique Python. Un code qui, upstream, ne voit que des floats peut
  utiliser `f64`, à condition de le justifier en commentaire.
- **Texte** : `str` Python ↔ `String` Rust. Si un chemin upstream peut produire
  un surrogate isolé (`chr()` sur une valeur arbitraire), le signaler et le
  traiter explicitement (la trace le détecte).
- **Objets pdfplumber** (`T_obj`) : dictionnaires ordonnés à clés `str`, comme
  en Python. L'ordre des clés est celui de Python (ordre d'affectation de
  `__dict__` des objets `LT*`, puis ajouts de `process_object`). Les attributs
  fréquents peuvent avoir un accès rapide, mais la vue dictionnaire reste exacte
  (`extra_attrs`, `to_dict`, l'API JS en dépendent).
- **Exceptions** : `pycompat::PyErr { kind, message }`, où `kind` reprend la
  classe Python (`PSEOF`, `PSSyntaxError`, `PDFSyntaxError`, `KeyError`,
  `TypeError`, `AssertionError`, `ValueError`, `IndexError`, `zlib.error`,
  `struct.error`, `UnicodeDecodeError`, `RecursionError`, …) avec sa
  hiérarchie (`except PSException:` attrape `PSEOF`). Le message reproduit
  `str(e)`. Les `try/except` upstream deviennent des `match` sur ces types.
- **Avertissements** (`logging`, `warnings.warn`) : leur texte est sans effet
  sur la sortie et remonte par un canal de diagnostics (option `onWarning`
  côté JS) ; leurs cadres Python comptent pour la profondeur de récursion
  (ci-dessous).
- **Réglages** : `pdfminer.settings.STRICT = False`, comme l'oracle.

## Profondeur de récursion

Python lève `RecursionError` à partir de deux compteurs, que
`pycompat::recursion` reproduit (`Depth`, par fil d'exécution) : les cadres
Python (au plus `sys.getrecursionlimit()` = 1000) et les unités de récursion
C (10 000 : `repr()`, `str()`, les comparaisons, l'appel d'une fonction
native en prennent une, une entrée dans la boucle d'évaluation depuis du C
deux). Règles :

- Les structures arbitrairement profondes que Python construit sans
  récursion (un flux de contenu de 10^6 `[`) se libèrent, se comparent,
  s'écrivent et se convertissent sans récursion en Rust non plus
  (`PDFObject`, `Value`, JSON typé, `canon`) : la pile Rust (1 Mio dans un
  navigateur) ne limite jamais rien.
- Là où Python a une limite, le port la reproduit : `repr()`/`str()` d'un
  objet PDF prennent les unités C de CPython à chaque niveau (cas
  unitaires à la frontière pour chaque sorte d'objet), `resolve_all` et
  `decipher_all` un cadre par niveau. Une fonction Python portée prend son
  cadre (`py_frame()`) pour sa durée, un appel depuis du C ses unités
  (`eval_entry()`), un appel de log ses cadres (`py_frames(2)`, 3 au premier
  appel d'un niveau sur un logger, voir `docs/verification.md`).
- Le comptage est en place dans `pdftypes` (`resolve_all`, `decipher_all`,
  `PDFObjRef.resolve`, `repr`) et pour `literal_name`. Les fonctions de
  `psparser` et de `pdfinterp` ne prennent pas encore de cadre : aucun
  appelant porté ne peut les exécuter près de la limite des cadres (une
  étape les appelle à une profondeur d'environ 10). Elles en prendront avec
  le port de l'interpréteur, dont la récursion (`do_Do` → `render_contents`)
  les y amène, chaque module étant vérifié par des cas unitaires à la
  frontière, comme `pdftypes`.
- Le harnais fixe la profondeur de départ (`with_depth`) à celle que
  l'oracle a mesurée au même point d'appel ; le code porté ne la fixe jamais.

## Résolution des références et ré-entrance du parseur (pour `pdfparser`/`pdfdocument`)

Décisions prises avant le port de `pdfparser.py` et `pdfdocument.py` :

- `PDFParser.do_keyword(KEYWORD_STREAM)` lit `int_value(dic["Length"])`
  quand `fallback` est faux (tout PDF dont la table xref est valide, et tout
  PDF chiffré) ; avec un `/Length` indirect, la résolution passe par
  `PDFDocument.getobj` → `_getobj_parse`, qui réutilise **le même**
  parseur (`self._parser.seek(pos)`, dont le `reset()` vide le contexte et la
  pile du `nextobject` extérieur) pendant que celui-ci est dans
  `do_keyword`. Le document garde donc son parseur dans un
  `RefCell<Option<PDFParser>>` : un `getobj` venu de l'extérieur le prend,
  et la résolution faite depuis `do_keyword` reçoit le parseur en cours
  (`&mut PDFParser`, contexte explicite : `getobj_in(objid, Some(parser))`,
  `int_value_in`), sans emprunt du `RefCell`, de sorte que le `seek` et le
  `reset` imbriqués agissent sur le même état, comme en Python. Un cas
  adverse de l'étape `objects` le vérifie : un `/Length` indirect dont le
  dictionnaire du flux est dans un tableau non fermé.
- `PDFDocument.__init__` résout déjà des références (`Encrypt`, `ID`,
  `Info`, `Root` du trailer) : le document est créé dans un `Rc` dont l'état
  est intérieur (`RefCell`/`Cell`) avant que sa logique d'`__init__` ne
  s'exécute, pour que `PDFObjRef.resolve` (`Weak<dyn PDFResolver>`) puisse le
  joindre pendant l'initialisation (`Rc::new_cyclic` ne suffit pas : son
  `Weak` ne se résout qu'une fois la construction finie).
- Les clés de `PDFXRef.offsets`, de `_cached_objs`, l'argument de
  `getobj` et `PDFStream.objid` sont des valeurs Python hachables quelconques
  (`PDFXRefFallback` stocke tel quel l'en-tête d'un flux d'objets : `5.5`,
  `True`, un littéral, un mot-clé…) : un type de clé `ObjId` suit `hash()` et
  `==` de Python (nombres via `pycompat::NumKey` : `5 == 5.0 == True` ;
  littéraux et mots-clés par identité ; `bytes` par valeur ; une liste ou un
  dict lève `TypeError("unhashable type: 'list'")`). `PDFObjRef.objid` reste un
  `PyInt` : pdfminer ne crée de référence qu'à travers `safe_int`
  (`pdfparser.py`), toujours un `int`.
- `CMapParser.do_keyword` appelle `CMapDB.get_cmap` (`usecmap`) de façon
  synchrone : le fournisseur de CMaps prédéfinies est synchrone pour le
  cœur ; un chargement paresseux côté navigateur se fait avant l'analyse
  (le cœur signale le nom manquant et l'analyse est relancée).
- `Type1FontHeaderParser` met dans `results` des paires `(cid, nom)` dont
  `cid` est un `int` Python quelconque : le parseur générique reçoit un type
  de résultat associé (`PSStackParserExt::Result`) plutôt qu'un encodage dans
  `PDFObject`.

## Sémantique Python centralisée (`crates/pycompat`)

Tout ce qui imite Python vit ici, une seule fois, testé contre CPython :
`sum()` 3.12 (Neumaier, ints exacts), `min`/`max` (premier à égalité),
`float ** float` avec le `pow` de la bibliothèque C de l'oracle (glibc 2.39,
variante FMA choisie sur un processeur x86-64 qui a FMA et AVX2, portée dans
`pycompat::libm` : `f64::powf` n'est pas cette fonction en Wasm), le `hash()`
numérique et les nombres comme clés de dict (`NumKey` : `3` trouve `3.0`),
`round()` (arrondi bancaire décimal exact), `float.__repr__`, `int()` (limite de
4 300 chiffres), conversions `int`→`float` (OverflowError), division réelle et
entière, `%` Python, tris stables, `str.split()`/`strip()` et classes de
caractères (`\s` bytes = `[\t\n\v\f\r ]`, `str.isspace()` Unicode),
normalisation Unicode (NFC/NFKC/NFD/NFKD, tables de la version Unicode de
CPython 3.12), échappement JSON `ensure_ascii`.

## Données upstream

Les tables de pdfminer (`glyphlist.py`, `latin_enc.py`, `fontmetrics.py`,
`encodingdb.py`) sont **générées** en Rust par un script de `oracle/codegen/`
qui lit les modules Python : jamais recopiées à la main. Les CMaps prédéfinies
(`pdfminer/cmap/*.json.gz`, ~8 Mo) ne sont pas embarquées : le cœur les demande
par nom à un fournisseur (fichiers en natif et sous Node, `fetch` paresseux dans
le navigateur), et seuls les documents qui en ont besoin les chargent. Tout
point d'entrée qui charge des polices (points d'entrée natif et Wasm, tests
d'étape pdffont/pdfinterp, exécuteurs de corpus multi-threads de `pdftrace`)
installe ce fournisseur : `CMapDB::set_default_provider` une fois pour le
processus, ou `CMapDB::set_provider` pour un thread (prioritaire ; `None` =
installation sans les données, `CMapNotFound`). Sans aucun fournisseur, le
chargement d'une CMap prédéfinie lève `RuntimeError` plutôt que
`CMapNotFound`, que pdffont transformerait en silence en CMap vide.

## Frontière JavaScript (`crates/wasm`, `js/`)

- **Gros grain** : le JS envoie les octets du PDF et des requêtes (page,
  options) ; le Rust renvoie des résultats complets (objets d'une page, texte,
  mots, tables). Jamais un aller-retour par objet intermédiaire.
- Pas de `wasm-bindgen` : ABI minimale (`alloc`, `free`, appels qui écrivent
  un tampon de résultat). Le format de résultat est choisi par mesure
  (JSON compact ou bande binaire en colonnes) ; les nombres traversent sans perte.
- Les pages dérivées (`crop`, `within_bbox`, `filter`, `dedupe_chars`) vivent
  côté Rust. `filter(fn)` évalue `fn` en JS sur les objets déjà matérialisés
  et renvoie un masque au Rust.
- Le même cœur doit pouvoir servir un module natif (PyO3) plus tard : pas de
  dépendance du cœur à `wasm32`, au JS ou à un runtime asynchrone.
- Dépendances Rust : le moins possible. Une crate éprouvée est acceptable pour
  un algorithme standard au résultat unique (MD5, SHA-2, AES), vérifiée contre
  CPython. Elle ne l'est pas quand il faut reproduire un comportement propre à
  CPython ou à zlib : erreurs et sortie partielle de `zlib` (porter l'inflate
  de zlib), tables Unicode de la version de CPython 3.12 (générer depuis
  `unicodedata`).
