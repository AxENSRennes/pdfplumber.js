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
- **Texte** : `str` Python ↔ `pycompat::PyStr` (les octets de
  `s.encode("utf-8", "surrogatepass")` : un surrogate isolé, que pdfminer
  produit par `chr(cid)`, y tient comme en Python ; ordre, égalité,
  concaténation et `in` sont ceux des octets, `as_str()` donne le `&str` d'un
  texte sans surrogate). Le texte des objets (`LTChar`, valeurs et clés des
  objets pdfplumber, `TextMap`, cellules de tables) et les messages
  d'exception qui reprennent une `str` de l'utilisateur
  (`PyErr::with_message`) l'utilisent ; une chaîne qui ne peut pas contenir
  de surrogate (nom de classe, mot-clé de pdfminer) reste un `String`. Vers
  JavaScript, `to_utf16`/`from_utf16` (un surrogate isolé est une unité
  UTF-16 ; une paire valide se relit comme un seul caractère).
- **Identité des NaN** : Python compare les éléments des conteneurs et les
  clés de `set`/`dict` avec un raccourci d'identité (`x is y or x == y`) ;
  une valeur NaN du modèle d'objets porte l'identité de son objet dans sa
  charge utile (`PyValue::float` : un nouvel objet ; un clone : le même),
  voir `obj.rs`, « NaN identity ». `PyValue::Float` tient un `PyFloat` au
  champ privé : hors de `obj.rs`, un `float` ne se fabrique que par
  `PyValue::float` (nouvel objet ; c'est ce qu'emploie un décodeur de
  valeurs venues de JavaScript) ou `PyValue::float_with_identity` (cas
  de test), jamais comme un NaN nu que toutes les valeurs partageraient.
- **Taille des séquences** : `seq * n` (`str`, `bytes`, `tuple`, `list`)
  lève les erreurs de CPython (`OverflowError`, `MemoryError` au-delà de
  `PY_SSIZE_T_MAX`) et construit tout résultat de 4 Gio au plus (dans la
  représentation du port, `obj::MAX_REPEAT_BYTES`). Au-delà, le port lève
  `MemoryError` là où CPython tenterait l'allocation (refusée sur la
  machine de l'oracle à partir d'environ 26 Gio) : écart accepté, puisque
  `wasm32` ne peut adresser plus ; c'est ce que ferait CPython sur une
  machine qui n'accorde pas l'allocation. Aucun cas généré ne tombe entre
  les deux. Les boucles du texte en mise en page (`[x] * n`, `range(n)`
  qui ajoute une ligne par tour) gardent leur garde de 10^7 tours
  (`utils/text.rs`, `MAX_REPEAT`) : Python y bouclerait jusqu'à épuiser la
  mémoire.
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
  `str(e)` ; quand il contient un surrogate isolé (une `str` de
  l'utilisateur), `message` en est une copie avec U+FFFD, et seul
  `PyErr::py_message()` est exact : tout ce qui montre `str(e)` (traces,
  frontière JavaScript, en UTF-16 par `PyStr::to_utf16`) lit
  `py_message()`. Les `try/except` upstream deviennent des `match` sur ces
  types.
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
  cadre (`py_frame()`) pour sa durée ; une fonction qui revient aussitôt
  est seulement vérifiée (`leaf(n)`) ; un appel depuis du C (le `__init__`
  d'une classe appelée, un générateur repris par `list()`, `join()`,
  `extend()`, une lambda de `re.sub`) prend aussi ses unités
  (`eval_entry()`, `c_called_leaf()`) ; un générateur prend son cadre à sa
  création (il démarre et se suspend) puis à chaque reprise ; un appel de
  log prend ses cadres par `pycompat::logging` (deux, trois au premier
  appel d'un niveau sur un logger depuis `logging.disable()`, qui vide les
  caches, voir `docs/verification.md`). Une compréhension ne prend pas de
  cadre (PEP 709) ; `collections.abc.Iterator[...]` non plus (fait en C).
- `LIT()`/`KWD()` appelés à l'exécution (un nom lu par le parseur,
  `KWD(b"obj")` de `_getobj_parse`, `LIT("D")` des libellés) prennent le
  cadre de `PSSymbolTable.intern`, plus celui de `PSLiteral.__init__` (appelé
  depuis le C) la première fois que le processus interne ce nom
  (`psparser::py_intern`). L'état « noms internés » est celui d'un processus
  Python : au départ ceux qu'internent les imports (`import pdfplumber`,
  `psparser_symbols.rs` ; ceux de `python -m oracle.trace --stage S` pour
  le harnais, `pdftrace::stages::py_symbols`, générés par
  `oracle/codegen/py_symbols.py`), puis tous ceux internés depuis ; le
  harnais repart de l'état d'un nouveau processus à chaque trace, comme
  l'oracle (`pdftrace::stages::py_process`).
- Le comptage est en place partout où le document est lu : `psparser`
  (chaque méthode des parseurs, `_add_token`, `contextlib.suppress`,
  `literal_name`), les accesseurs de `pdftypes` (`resolve1`, `dict_value`…,
  qui comptent les leurs), `PDFStream` (`__getitem__`, `get`,
  `get_filters`, `decode`, `get_data`), les filtres (`lzwdecode`,
  `ascii85decode`, `asciihexdecode`, `rldecode`, prédicteurs,
  `ccittfaxdecode`, `decompress_corrupted`), `pdfparser`, `pdfdocument`
  (`getobj`, `read_xref_from`, les chargements de xref, `lookup`, les
  plans, les libellés), `pdfpage` (`create_pages`, chaque générateur
  `depth_first_search` de la chaîne `yield from`, repris à chaque page,
  `get_pages`) et `pdfinternals.resolve_all`, ainsi que
  `PDFContentParser`. Une boucle (`/Prev`, flux d'objets, `/Length` égal au
  flux, arbre de pages ou de plans, cycle de références) finit en
  `RecursionError` comme en Python, et une chaîne finie échoue au même
  niveau : les corpus adverses `adv-pages` et `adv-objects` ont, pour
  chaque forme de récursion, des fenêtres de longueurs autour du premier
  niveau qui échoue en Python (`cases_depth`), dont le point le plus
  profond est chaque fois un autre chemin (journalisation, flux, nom
  interné pour la première fois, flux d'objets et chaque filtre,
  déchiffrement, xref de repli, arbres de nombres et de noms, plans,
  `resolve_all`). Restent à compter les modules portés plus tard
  (`pdfinterp` hors `PDFContentParser`, `pdffont`, `layout`) et les appels
  d'un code porté avant ce comptage (`cmapdb` : ses fonctions ne prennent
  pas encore leur cadre).
- Le harnais fixe la profondeur de départ (`with_depth`) à celle que
  l'oracle a mesurée au même point d'appel ; le code porté ne la fixe jamais.

## Résolution des références et ré-entrance du parseur (pour `pdfparser`/`pdfdocument`)

Décisions prises avant le port de `pdfparser.py` et `pdfdocument.py` :

- `PDFParser.do_keyword(KEYWORD_STREAM)` lit `int_value(dic["Length"])`
  quand `fallback` est faux (tout PDF dont la table xref est valide, et tout
  PDF chiffré), et `dict_value(dic)` dans tous les cas ; une référence y
  passe par `PDFDocument.getobj` → `_getobj_parse`, qui réutilise **le
  même** parseur (`self._parser.seek(pos)`, dont le `reset()` vide le
  contexte et la pile du `nextobject` extérieur) pendant que celui-ci est
  dans `do_keyword`. Implémentation (vague 2) : le document garde son
  parseur dans un emplacement (`RefCell<Option<Box<PDFParser>>>`) ; toute
  opération qui lit le fichier l'en sort (`PDFDocument::with_parser`), et
  toute résolution lancée pendant que le parseur tourne (`do_keyword`, le
  `dict_value` d'un trailer, le `get_data` d'un flux d'xref ou d'un flux
  d'objets lu pendant `__init__`, `int_value` de `/Prev`) le **prête** au
  document (`pdfparser::lend`) : l'état du parseur est échangé avec un
  substitut et remis dans l'emplacement le temps de la résolution, où un
  `getobj` imbriqué le trouve, puis repris, avec ce que l'imbrication y a
  laissé, comme en Python. Les résolutions passent ainsi par l'interface
  ordinaire (`PDFObjRef.resolve`, `PDFResolver`), sans contexte explicite
  à propager dans `pdftypes`. Un cas adverse de l'étape `objects`
  (`length-indirect-in-array`) le vérifie : un `/Length` indirect dont le
  dictionnaire du flux est dans un tableau non fermé.
- Le fichier : `PDFParser` fait `self.fp.seek(pos)` et `self.fp.read(n)`
  sur des valeurs du fichier (`/Length`, `/Prev`, offsets) ; les erreurs
  d'un `io.BufferedReader` (`open(path, "rb")`, ce que lit pdfplumber et
  l'oracle) et d'un `io.BytesIO` diffèrent pour une position ou une
  longueur négative ou énorme : `pdfparser::FileKind` choisit lesquelles
  (l'API JS lit des octets ; l'oracle des étapes, un fichier). Un
  `BufferedReader.read(n)` au-delà de son tampon alloue `n` octets d'abord :
  le port fait la même allocation (mémoire virtuelle jamais touchée) et lève
  `MemoryError` quand elle échoue ; en wasm32, où la mémoire ne rétrécit
  pas, seule une taille hors de l'espace d'adressage échoue. Non modélisé :
  l'`OSError` d'un `seek` au-delà de la taille maximale de fichier du
  système de fichiers.
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
  (`pdfparser.py`), toujours un `int`. Implémenté par
  `pdftypes::ObjId` (l'objet d'origine et l'égalité/le hachage de Python) ;
  `PDFStream::objid()` rend un `ObjId`. La fonction de déchiffrement reçoit
  cet `ObjId` comme Python : `struct.pack("<L", objid)` (RC4, AES-128)
  lève `struct.error` pour un id qui n'est pas un entier (un document
  chiffré réparé par `PDFXRefFallback`), AES-256 et Identity l'ignorent,
  et rien n'est appelé pour un objet sans chaîne non vide.
- `CMapParser.do_keyword` appelle `CMapDB.get_cmap` (`usecmap`) de façon
  synchrone : le fournisseur de CMaps prédéfinies est synchrone pour le
  cœur ; un chargement paresseux côté navigateur se fait avant l'analyse
  (le cœur signale le nom manquant et l'analyse est relancée).
- `Type1FontHeaderParser` met dans `results` des paires `(cid, nom)` dont
  `cid` est un `int` Python quelconque : le port garde ces paires dans l'état
  du parseur (`Type1Header::pairs`, dans le même ordre) et pose une entrée
  témoin dans `results` (seul `do_keyword` en ajoute, `flush` ne fait rien,
  le parseur ne repositionne qu'à sa création), sans changer `psparser`.
- `pdffont::get_font` est la partie de `PDFResourceManager.get_font` qui
  crée la police (aiguillage sur `/Subtype`, polices `Type0`) et prend le
  cadre de `get_font` ; le cache par `objid` reste à `PDFResourceManager`
  (`pdfinterp`), qui l'appelle.
- Profondeur dans `pdffont` : chaque fonction portée prend son cadre ;
  `pdftypes` (accesseurs, `resolve1`, `PDFStream.get_data` jusqu'aux
  filtres) et `psparser` (parseurs, `literal_name`, internement) comptent
  les leurs, et `pdffont::frames` prend ceux des appels vers du code qui ne
  les compte pas encore (`casting`, `get_unichr`/`decode` de `cmapdb`,
  `EncodingDB.get_encoding`/`name2unicode`). Quand un de ces modules
  comptera ses cadres, l'aide correspondante disparaît. Les appels de
  journalisation prennent les leurs par `pycompat::logging`. `cmapdb`
  compte les siens pour `CMapDB` (`get_cmap`, `get_unicode_map`,
  `_load_data` avec les cadres de la bibliothèque standard qu'il exécute :
  4 sous `_load_data` pour un nom absent, `os.path.realpath`, et 8 pour le
  premier chargement d'un fichier, `json.load` lisant l'en-tête gzip,
  mesurés pour chaque fichier de pdfminer) et pour `CMapParser`
  (construction, `run`, `do_keyword`, `_warn_once`). Non modélisés
  (documentés dans `cmapdb.rs` et `pdffont/mod.rs`) : les cadres de
  `add_cid2unichr` et du générateur `choplist` sous `do_keyword`, jamais
  plus profonds que la construction du parseur, qui vient avant ; l'import
  du codec UTF-16 au premier `bytes.decode("UTF-16BE")` d'un processus
  Python.

## Sémantique Python centralisée (`crates/pycompat`)

Tout ce qui imite Python vit ici, une seule fois, testé contre CPython :
`sum()` 3.12 (Neumaier, ints exacts), `min`/`max` (premier à égalité),
`float ** float` avec le `pow` de la bibliothèque C de l'oracle (glibc 2.39,
variante FMA choisie sur un processeur x86-64 qui a FMA et AVX2, portée dans
`pycompat::libm` : `f64::powf` n'est pas cette fonction en Wasm), `math.atan2`
(`m_atan2` de CPython puis l'`atan2` de la même glibc, variante FMA, porté
dans `pycompat::libm` : `f64::atan2` est celui de musl en Wasm), le `hash()`
numérique et les nombres comme clés de dict (`NumKey` : `3` trouve `3.0`),
`round()` (arrondi bancaire décimal exact), `float.__repr__`, `int()` (limite de
4 300 chiffres), conversions `int`→`float` (OverflowError), division réelle et
entière, `%` Python, tris stables, `str.split()`/`strip()` et classes de
caractères (`\s` bytes = `[\t\n\v\f\r ]`, `str.isspace()` Unicode),
normalisation Unicode (NFC/NFKC/NFD/NFKD, tables de la version Unicode de
CPython 3.12), échappement JSON `ensure_ascii`, `bytes % int` (avec le
`MemoryError` d'une largeur ou d'une précision que l'allocation refuse),
les deux compteurs de récursion (`recursion`) et les cadres d'un appel de
`logging` (`logging`, le cache par logger et niveau).

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
- Pas de `wasm-bindgen` : ABI minimale (`pp_alloc`, `pp_free`, `pp_call` qui
  écrit un tampon de résultat). Le format de résultat, choisi par mesure, est
  le binaire `PJB1` (valeurs étiquetées, table de chaînes, objets en colonnes) ;
  les nombres traversent sans perte. Détails, mesures, CMaps paresseuses,
  erreurs et diagnostics : [`js-runtime.md`](js-runtime.md).
- Les pages dérivées (`crop`, `within_bbox`, `filter`, `dedupe_chars`) vivent
  côté Rust. `filter(fn)` évalue `fn` en JS sur les objets déjà matérialisés
  et renvoie un masque au Rust. Une page rognée garde la `bbox` telle que
  l'appelant l'a donnée (`list` ou `tuple` : les messages d'erreur
  l'impriment, `calculate_area` reçoit cette valeur).
- **`search` (décidé)** : `TextMap::search` reçoit les correspondances
  (positions en points de code). L'API JS prend une `RegExp` ou une chaîne
  avec la sémantique des `RegExp` de JavaScript (`re` n'est pas porté) : JS
  exécute le motif sur `as_string`, le cœur convertit les positions UTF-16 en
  points de code et construit les résultats de pdfplumber. Les écarts avec
  `re` (correspondances vides de `finditer`, classes Unicode, syntaxe, `$`)
  sont documentés dans `public-api.md` (« Search »). Un port de `sre` reste
  possible plus tard (même interface : les spans).
- Le même cœur doit pouvoir servir un module natif (PyO3) plus tard : pas de
  dépendance du cœur à `wasm32`, au JS ou à un runtime asynchrone.
- Dépendances Rust : le moins possible. Une crate éprouvée est acceptable pour
  un algorithme standard au résultat unique (MD5, SHA-2, AES), vérifiée contre
  CPython. Elle ne l'est pas quand il faut reproduire un comportement propre à
  CPython ou à zlib : erreurs et sortie partielle de `zlib` (porter l'inflate
  de zlib), tables Unicode de la version de CPython 3.12 (générer depuis
  `unicodedata`).
