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
5. Les parties de pdfminer/pdfplumber **hors périmètre** (les convertisseurs
   texte/HTML/XML/hOCR de `converter.py` et `PDFConverter`, `TagExtractor` de
   `pdfdevice.py`, `high_level.py`, `image.py`/ImageWriter, `display.py`
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
  (`extra_attrs`, `to_dict`, l'API JS en dépendent). Le `/P` des
  annotations est la page elle-même (`PyValue::Page`, `obj::PageRef` :
  `<Page:n>`, égale à elle seule).
- **Exceptions** : `pycompat::PyErr { kind, message }`, où `kind` reprend la
  classe Python (`PSEOF`, `PSSyntaxError`, `PDFSyntaxError`, `KeyError`,
  `TypeError`, `AssertionError`, `ValueError`, `IndexError`, `zlib.error`,
  `struct.error`, `UnicodeDecodeError`, `RecursionError`, …) avec sa
  hiérarchie (`except PSException:` attrape `PSEOF`). Le message reproduit
  `str(e)` ; quand il contient un surrogate isolé (une `str` de
  l'utilisateur), `message` en est une copie avec U+FFFD, et seul
  `PyErr::py_message()` est exact : tout ce qui montre `str(e)` (traces,
  frontière JavaScript, en UTF-16 par `PyStr::to_utf16`) lit
  `py_message()`. Une exception levée avec une autre pour seul argument
  (`PdfminerException(e)`, `MalformedPDFException(e)`) la garde
  (`PyErr::wrapping`, `PyErr::cause`). Les `try/except` upstream deviennent
  des `match` sur ces types.
- **Avertissements** (`logging`, `warnings.warn`) : leur texte est sans effet
  sur la sortie et remonte par un canal de diagnostics (option `onWarning`
  côté JS) ; leurs cadres Python comptent pour la profondeur de récursion
  (ci-dessous). La référence est la journalisation de pdfminer coupée
  (`logging.disable`, ou `setLevel(ERROR)` sur le logger `pdfminer`, mêmes
  cadres) : dans la configuration par défaut de Python, un `log.warning`
  est émis par `logging.lastResort`, plus profond, et formate ses `%r`
  (écart non modélisé, `verification.md`, « Étapes »). Texte transmis
  aujourd'hui : le `logger.warning` de `PDF.__init__` (métadonnées), celui
  de `PDFStructTree` (« Unknown attribute class »), le `warnings.warn` de
  `Page.annots`, ce dernier une fois par texte (`pycompat::warnings` : les
  filtres par défaut de Python et leur registre, état du processus remis
  par un appel nul). Les appels `log.*` de pdfminer ne comptent que leurs
  cadres : leur texte n'est pas transmis (`public-api.md`).
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
  (`PDFObject`, `Value`, JSON typé, `canon`). Là où Python récurse (les
  Form XObjects : `execute` → `do_Do` → `render_contents` → `execute`,
  trois cadres par niveau), le Rust récurse aussi, jusqu'à la limite de
  Python : la plus longue chaîne que Python accepte (environ 330 niveaux)
  prend environ 1,2 Mio de pile au profil `dist` en Wasm (mesuré : 3,7 Kio
  par niveau ; avec 512 Kio de pile, la chaîne la plus profonde qui passe
  a 140 niveaux), plus que le 1 Mio que `wasm-ld` réserve par défaut (le
  profil `dist` piège sur chaque fenêtre `depth-*` d'`adv-interp`). Le
  module Wasm et les binaires `wasm32` de `pdftrace` ont donc une pile de
  8 Mio (`crates/wasm/build.rs`, `crates/pdftrace/build.rs`, plus de six
  fois la mesure ; avec 2 Mio tout passe déjà), et
  `scripts/wasm-depth-gate.sh` (dans `scripts/check.sh`) fait tourner le
  profil `dist` en `wasm32-wasip1` sur les fenêtres de profondeur
  d'`adv-interp`, `adv-pages` et `adv-objects` et compare ses traces à
  celles du natif. En natif, un fil de 2 Mio (celui des tests) suffit au
  profil `release` (1,5 Mio passe, 1 Mio non) ; un hôte qui appellerait le
  cœur depuis un fil plus petit devrait l'agrandir.
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
- L'interpréteur adaptatif de CPython 3.12 exécute la première fois
  l'instruction `CALL` d'un site sous sa forme générique, puis la
  spécialise. Pour `str(x)` (le type `str`, sans `tp_vectorcall` en 3.12),
  la forme générique passe par `_PyObject_MakeTpCall`, qui tient une unité
  C de plus pendant tout l'appel : le premier `str(x)` d'un processus à un
  site lève `RecursionError` pour un objet imbriqué un niveau de moins que
  les suivants (`CALL_NO_KW_STR_1`). Seuls les sites dont l'appel peut
  approcher la limite C sont modélisés (`pycompat::specialize`, état de
  processus) : `return str(x)` de `literal_name` (psparser.py:121,
  `psparser::LITERAL_NAME_STR`), qu'atteignent un opérande de `Tf`, `Do`,
  `CS`/`cs`, une ressource `/ColorSpace`, une clé de dictionnaire des
  parseurs ou de `CMapParser`. Ailleurs, l'écart est d'une unité tenue le
  temps d'un appel qui ne récurse pas (un `isinstance` générique), loin de
  la limite ; les appels de classes et de fonctions Python ne changent pas
  de coût en 3.12. `oracle/depth.py` (`executed_call_sites`) lit l'état
  d'un site dans le compteur adaptatif de son instruction.
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
  l'oracle (`pdftrace::stages::py_process`). L'état modélisé d'un
  processus (noms internés, motifs `re` compilés, caches des loggers,
  sites d'appel spécialisés, caches de `CMapDB`) se prend et se remet d'un
  bloc (`pdfminer::process_state`) : la frontière JavaScript le remet tel
  qu'il était avant un appel nul du protocole des CMaps paresseuses, pour
  que la relance parte de l'état dont l'exécution unique de Python serait
  partie.
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
  `resolve_all`). `pdffont` compte les siens (vague 2), `cmapdb` ceux de
  `CMapDB` et de `CMapParser`, et `pdffont::frames` ceux du code qu'il
  appelle sans comptage (`casting`, requêtes de `cmapdb`, `encodingdb`) ;
  ces cas sont vérifiés aux seuils par `fonts_unit` (`depth`,
  `corpus_depth`). La vague 3 compte `pdfinterp` et `pdfdevice` (chantier
  INTERP, ci-dessous : les fonctions de `casting` et de `utils` qu'ils
  appellent, les méthodes de `PDFStream` appelées depuis le C, la
  compilation du motif de `re.sub` de `get_inline_data`, vérifiés aux
  seuils par `adv-interp`), `converter`, le périphérique de pdfplumber
  (`page.rs`), les constructeurs des objets bruts de `layout` et
  `decode_text` (chantier CONVERTER, vérifiés par le jeu `depth` de
  `oracle/unit/converter.py`) ; restent l'analyse du layout hors de ses
  récursions et le reste de la couche pdfplumber.
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

## Vague 3 : interpréteur, périphériques, convertisseur

Base commune : branche `wave3/base` (étapes Python `fonts`, `interp`,
`layout` ; squelettes Rust qui compilent, corps `todo!()`). Deux chantiers
parallèles, INTERP et CONVERTER, partent de cette base et ne modifient pas
le contrat ci-dessous sans accord (un changement d'une étape Python
invalide son cache et celui des étapes qui en dépendent).

### Décisions

- **Pilotage** : celui de pdfplumber. Un `PDFResourceManager` par document
  (pdfplumber/pdf.py:28 `PDF.__init__`, à sa ligne 53, `caching=True`) ;
  pour chaque page un `PDFPageAggregatorWithMarkedContent(rsrcmgr,
  pageno=..., laparams=...)` et un `PDFPageInterpreter(rsrcmgr, device)`,
  puis `process_page` (pdfplumber/page.py:256 `Page.layout`). Le
  gestionnaire de ressources fait partie de l'état du document : un appel
  nul du protocole des CMaps paresseuses (`crates/wasm/src/cmaps.rs`) le
  jette avec les polices qu'il a mises en cache (une police construite sans
  sa CMap garde un `CMap()` vide). L'interpréteur ne rattrape rien qui
  masquerait ce signal (ce n'est pas une exception : le fournisseur note le
  nom).
- **`id()` du layout** : l'étape `layout` écrit, pour l'analyse qu'elle
  trace, le rang des `id()` des boîtes et groupes du résultat (lignes
  `ids`, clés `[min_char, n_boxes, rang]` de `oracle/unit/layout.py`).
  Le module déclare `ORACLE_INPUT` ; `oracle.verify` passe alors à
  `pdftrace` `--oracle-dir <cache de l'étape>`, et le Rust lit la trace de
  l'oracle (`StageInput::oracle`, `stages::layout::oracle_ids`) pour
  analyser avec un `IdSource` injecté (le `RecordedIds` de
  `layout_unit.rs`, à déplacer dans la bibliothèque) : la parité est
  exacte. La bibliothèque et l'API JS gardent `CreationOrder`. À mesurer
  et noter ici une fois l'étape identique : la proportion de pages de
  `all` où `CreationOrder` seul donne le résultat de Python (`pdftrace
  --stage layout` sans `--oracle-dir`, comparé page à page) ; c'est une
  statistique, pas un critère de parité. Première mesure, sur le rejeu du
  corpus de `oracle/unit/converter.py` : 93,3 % (« Chantier CONVERTER »).
- **Images en ligne** : pdfminer/pdfinterp.py:1309 `PDFPageInterpreter.do_EI`
  les nomme `str(id(obj))`, une adresse mémoire. Les traces écrivent
  `inline{"n": k}` (`k` : rang d'apparition dans la page). Le port leur
  donne un nom déterministe, `pdfinterp::inline_image_name(k)` =
  `"inline-<k>"`, `k` comptant les images en ligne de l'interprétation
  d'une page (compteur partagé par les sous-interpréteurs,
  `PDFPageInterpreter::inline_images`) : écart documenté, qui atteint
  `LTFigure.name`, `LTImage.name` et le `name` des images de pdfplumber.
  Python repère ces noms par le site d'appel (`do_EI`), Rust par les noms
  qu'il a donnés ; les deux ne divergent que si un PDF nomme une XObject
  `inline-<k>`. Les noms ne sont uniques que dans l'interprétation d'une
  page : deux pages (ou une page interprétée de nouveau après
  `flush_cache`) redonnent `inline-0`, `inline-1`… et `pdf.images`,
  `pdf.objects["image"]` ont des noms répétés, là où les adresses de
  Python diffèrent pour des images vivantes en même temps.
- **Hors périmètre** : `PDFConverter`, `TextConverter`, `HTMLConverter`,
  `XMLConverter`, `HOCRConverter` et `pdfdevice.TagExtractor`
  (en-têtes de `converter.rs` et `pdfdevice.rs`).
- **Classes Python en Rust** (`pdfdevice.rs`, « Classes, overrides and
  `super()` ») : une classe = une structure d'attributs + des traits
  d'accès (`PDFDevice::attrs`, `LayoutAnalyzer::analyzer`,
  `PageAggregator::aggregator`, `MarkedContent::marked`) ; une méthode =
  une méthode de trait dont le corps est une fonction libre générique du
  module de sa classe (`pdfdevice::device`, `pdfdevice::text_device`,
  `converter::layout_analyzer`, `converter::page_aggregator`,
  `pdfplumber::page::marked_content`). Une sous-classe appelle la fonction
  de sa base comme Python appelle `super()`, et les appels sur `self` de ces
  fonctions (`render_char` depuis `render_string_horizontal`,
  `paint_path` par sous-chemin, `receive_layout`) atteignent la classe la
  plus dérivée, y compris les périphériques de trace de `pdftrace`.
- **Profondeur** : chaque méthode prend son cadre ; la récursion des Form
  XObjects (`execute` → `func(*args)` = `do_Do` → `subinterp` →
  `render_contents` → `execute`) compte, à chaque niveau, l'entrée de la
  boucle d'évaluation de `func(*args)` (`CALL_FUNCTION_EX`, à mesurer avec
  `oracle/depth_probe.py`), les appels de classe (`PDFTextState()`,
  `dup`, les objets `LT*` du convertisseur : à compter dans `layout.rs`,
  qui ne compte aujourd'hui que son analyse), le générateur de
  `CMap.decode` repris par la boucle de `render_string_*`, et les appels de
  journalisation (`pycompat::logging`, `pdfminer.pdfinterp`,
  `pdfminer.pdfdevice`, `pdfminer.converter`). Les étapes `interp` et
  `fonts` ajoutent les cadres de leurs surcharges Python (un par appel
  surchargé), que leurs périphériques Rust prennent aussi ; l'étape
  `layout` n'en ajoute aucun (analyse à la profondeur de `end_page`,
  vérifié par une sonde bornée).

### Contrat entre les chantiers

Fixé dans `wave3/base` (signatures, champs, sémantique documentés dans le
code) :

- `pdfminer::pdfdevice` : `PDFDevice` (`attrs`, `close`, `set_ctm`,
  `begin_tag`/`end_tag`/`do_tag` avec `props: Option<&PDFStackT>`,
  `begin_page`/`end_page(&PDFPage)`, `begin_figure(name: &str, bbox:
  &ObjSeq, matrix: &ObjSeq)`, `end_figure`, `paint_path(&PDFGraphicState,
  stroke, fill, evenodd, &[PathSegment])` sur l'état vivant,
  `render_image(name, &Rc<PDFStream>)`, `render_string(&mut PDFTextState,
  &PDFTextSeq, &Rc<PDFColorSpace>, &Rc<PDFGraphicState>)`, la copie de
  `do_TJ` partagée par les caractères), `PDFTextDevice`
  (`render_string_horizontal`/`_vertical` → `Point`, `render_char(Matrix,
  &Rc<PDFFont>, fontsize, scaling, rise: &Num, cid: &PyInt, ncs,
  graphicstate) -> PyResult<Num>`), `ObjSeq` (liste du PDF ou tuple de
  l'interpréteur), `PDFDeviceAttrs`, `PDFTextSeq = PDFObject`.
- `pdfminer::pdfinterp` : `PDFTextState`, `PDFGraphicState` (champs dans
  l'ordre de `__dict__`, `DICT_KEYS`), `StandardColor`, `Color`,
  `PDFStackT`, `PDFResourceManager` (`new`, `get_font(Option<&PyInt>,
  &PDFDict) -> Rc<PDFFont>`, `cached_fonts()`), `PDFPageInterpreter`
  (`new(Rc<PDFResourceManager>, Rc<RefCell<dyn PDFDevice>>)`,
  `process_page`, `render_contents`, `execute`, `fontmap`,
  `inline_images`), `inline_image_name`.
- `pdfminer::converter` : `InterpPayload` (`FontName` de `pdffont`,
  `GraphicState = Rc<PDFGraphicState>`, `Color`), `MinerPayload`,
  `PDFLayoutAnalyzer` (dont `ids: Box<dyn IdSource>`), `CurItem`,
  `LayoutAnalyzer`, `PDFPageAggregator`, `PageAggregator`, les fonctions de
  `layout_analyzer` et `page_aggregator` ;
  `pdfplumber::page` : `PlumberPayload`, `MarkedContentAttr`,
  `MarkedContentState`, `MarkedContent`, `PDFPageAggregatorWithMarkedContent`,
  `marked_content`.
- `pdftrace` : `StageInput::oracle`, `Stage::oracle_input`,
  `--oracle-dir`, `stages::common::page_layout` (le corps de
  `Page.layout`), `stages::interp::{Call, TracingDevice}`,
  `stages::layout::oracle_ids`.
- Les étapes Python `oracle/stages/{interp,fonts,layout}.py` sont la
  spécification (formats en tête de chaque fichier) ; mesures sur `quick`
  (150 fichiers, `-j 3`) : `interp` 31 s (89 s cumulées, 54 Mo de traces),
  `fonts` 20 s (59 s, 9 Mo), `layout` 39 s (107 s, 141 Mo) ; sur `all`
  (×12) : environ 18, 12 et 22 min cumulées et 0,7, 0,1 et 1,7 Go de
  cache. Les fichiers les plus lents de `quick` (`pdfjs-issue5509.pdf`
  ≈ 20 s, `highlights.pdf` ≈ 10 s) restent sous le délai de 120 s ; sur
  `all`, construire le cache avec `--oracle-only --timeout 600` et lister
  les délais.

### Chantier INTERP

`pdfinterp.rs` (tout sauf `PDFContentParser`), `pdfdevice.rs`, la
dispatch des opérateurs (`execute` : mêmes noms, mêmes arités, mots-clés
`T_a`, `_q`… compris), le comptage des cadres de l'interpréteur ; les
étapes Rust `fonts` et `interp` (`FontsDevice`, `TracingDevice`, le
rapport `font_attrs` déplacé de `fonts_unit.rs` dans la bibliothèque) ;
le corpus adverse `oracle/adversarial/interp.py` → `adv-interp`
(`oracle/corpus.py` nomme un corpus adverse d'après une étape ; celui-ci
est vérifié par `fonts`, `interp` et `layout`, `--set adv-interp`) : chaque opérateur avec des
opérandes manquants, en trop, de tout type (`safe_*` qui échouent,
`TJ` sur un nombre, des `bytes`, un dict, un flux), `q`/`Q` déséquilibrés,
`Tf` inconnu, couleurs et motifs (`SCN`/`scn` avec 0 à 5 composantes,
`Pattern`), `BI`/`ID`/`EI` (filtres, `~>`), `Do` absent, image ou Form
sans `/BBox`, `/Matrix`/`/BBox` atypiques, Form sans `/Resources`,
références circulaires de flux, `BMC`/`BDC`/`EMC` déséquilibrés,
polices `Type0` verticales et Type 3 ; `cases_depth` : imbrications de
Form XObjects autour du premier niveau qui échoue en Python, le point le
plus profond étant chaque fois un autre chemin (avertissement d'un
opérateur, police créée au niveau le plus profond, caractère, image en
ligne, chemin à plusieurs sous-chemins, référence circulaire refusée).
Tant que CONVERTER n'est pas fusionné, INTERP peut vérifier
l'interpréteur avec un générateur unitaire qui le mène sur un
`PDFTextDevice` enregistreur (sans convertisseur : `render_char` rend 0).

#### Réalisation (branche `wave3/interp`)

- `pdfinterp.rs` : tout `pdfinterp.py` (`PDFTextState`, `PDFGraphicState`,
  couleurs, `PDFResourceManager` avec son cache et `get_cmap`,
  `PDFPageInterpreter` : chaque `do_*` avec son nom et son arité, table
  `Op` de `execute` construite comme `"do_" + name.replace(...)`, donc
  `T_a`, `_q`, `q\x80` compris). `pop(n)` du `SCN`/`scn` suit Python sur
  un `ncomponents` quelconque (`/N` brut d'un `ICCBased` : `0.0`, `True`,
  entier négatif ou énorme, référence, nom : `TypeError` de `-n` ou de la
  tranche). `self.resources.copy()` d'une Form sans `/Resources` copie un
  dict ou une liste, lève `AttributeError` sinon (inatteignable : une page
  dont les ressources ne sont pas un dict n'a pas de Form à dessiner).
- `pdfdevice.rs` : `PDFDevice`, `PDFTextDevice` (`render_string`,
  `render_string_horizontal`/`_vertical` partagent une boucle,
  `text_seq_items` itère `seq` comme Python : liste, `bytes` en entiers,
  clés d'un dict, `PDFKeyError` du `__getitem__(0)` d'un flux, `TypeError`
  sinon).
- **CTM** : le contrat garde `Matrix` (des nombres). `do_Do` multiplie la
  `/Matrix` de la Form par la CTM sans contrôle ; un élément `bytes` ou
  `list` donnerait une CTM de séquences, mais le `begin_figure` de
  pdfplumber (`LTFigure`) lève `TypeError` avant pour toute telle matrice
  (`utils::ObjMatrix`). Pour un autre périphérique, le port lève
  `TypeError` dans `do_Do` (limite documentée dans `PDFPageInterpreter`).
- **Cadres** : chaque méthode prend le sien ; `func(*args)` prend une
  entrée de boucle d'évaluation (mesuré : 2 unités C de plus pour un
  opérateur à arguments, aucune pour `func()`) ; les fonctions de
  `casting` (`safe_float` 1, `safe_int` 1, `safe_matrix` 2, `safe_rgb` et
  `safe_cmyk` 3), `utils.mult_matrix`/`translate_matrix`, `keyword_name`,
  `font.is_multibyte()`/`is_vertical()` sont vérifiées par `leaf` ;
  `PDFStream.__contains__` (depuis le C) par `c_called_leaf`. Le
  `re.sub` de `PDFContentParser.get_inline_data` prend 2 cadres, et 11 à
  la première compilation de son motif dans un processus (mesuré avec
  `oracle/depth_probe.py`) : état de processus, depuis la jonction celui
  de `pycompat::re_cache` (un seul cache de motifs compilés par processus,
  comme celui de `re`, remis à zéro par
  `pdftrace::stages::py_process::start`).
- **Étapes sans convertisseur** : `interp` et `fonts` (Rust, écrites contre
  le contrat) ont besoin de `PDFPageAggregatorWithMarkedContent`. Pour
  vérifier l'interpréteur seul, deux étapes de plus,
  `oracle/stages/interp_text.py` et `fonts_text.py` : les mêmes lignes, sur
  `PDFTextDevice` (`render_char` rend `0`) ; leur `TextDevice` a le
  constructeur de la classe de pdfplumber et lève `TypeError("figure
  matrix is not numbers")` dans `begin_figure` là où `LTFigure` lèverait
  (ci-dessus). Les surcharges, enregistrements et le formateur sont
  partagés (`stages::interp::{traced, PageFormatter, run_with}`,
  `stages::fonts::run_with`, rapport de police `pdftrace::fonts_report`,
  sorti de `fonts_unit.rs`).
- **Corpus adverse** `oracle/adversarial/interp.py` → `adv-interp` (le nom
  de la base : `oracle/corpus.py` n'accepte que `adv-<étape>`) et
  générateur unitaire `oracle/unit/interp.py` → `interp_unit.rs`.
  `cases_depth` : chaînes de Form XObjects dont la page dessine d'abord
  les dernières (objets en cache, données décodées : le point le plus
  profond devient le contenu de la dernière Form plutôt que le `getobj`
  de son flux), pour 26 chemins (opérateurs avec avertissement, police
  créée — Helvetica, `/ToUnicode`, programme Type 1, CMap prédéfinie —,
  caractère, police verticale, `TJ` sur un flux, image en ligne — et la
  première du processus, qui compile le motif —, image XObject,
  sous-chemins, `q`/`cm`/`Q`, couleurs, ressource d'espace de couleur,
  balises, cycle refusé, liens dans `q`/`cm` et dans `BT`) ; le premier
  niveau qui échoue est cherché par dichotomie pour `interp_text`,
  `interp` et `fonts` (processus fils), la fenêtre couvre de deux niveaux
  avant à un après pour chacun.
- **Profondeur des chemins vers le périphérique** (`oracle/unit/interp_deep.py`,
  rejeu `deep_sweeps` de `interp_unit.rs`). Un `RecursionError` naît au
  premier point, dans le temps, qui dépasse la limite. Mesure bornée
  (crochet de profil à deux entiers) sur `adv-interp` et ses chaînes : le
  parseur de contenu descend, avant le premier opérateur de chaque
  `execute`, à 8 cadres au moins sous `execute` (39 000 `execute`) ;
  sur `PDFTextDevice`, aucun cadre d'un opérateur ne dépasse ce niveau
  (opérateur, `safe_*`, `mult_matrix`, journalisation, `push`/`pop`,
  `copy`, appels au périphérique : 7 au plus), seuls le parseur lui-même,
  la création de polices (dans `init_resources`) et `resolve1` le font. Avec
  le périphérique de pdfplumber, en revanche, les objets du convertisseur
  (`LTChar`, `set_bbox`, `apply_matrix_pt`) vont plus bas que le parseur :
  les cadres des chemins vers le périphérique (`do_Tj`, `do_TJ`,
  `render_string`, `render_string_horizontal`, `do_S`, `do_s`,
  `do_BDC`...) décident là où pdfplumber lève. `interp_text` ne peut pas les
  voir ; `interp_deep.py` les rend visibles : `DeepTracingDevice` (le
  `TracingDevice` de `interp_text`, chaque méthode descendant d'abord
  `DESCENT` = 60 cadres) rend chaque appel au périphérique plus profond que
  tout ce qui le précède sur sa page (chaque page a son périphérique et
  son interpréteur : un `RecursionError` n'arrête que sa page), et chaque
  fichier de `adv-interp` (hors `depth-*`) est tracé avec l'interprétation
  de ses pages (`page_layout`) descendue de 17 profondeurs successives
  (`down`), qui font monter la limite d'un cadre à la fois depuis
  au-dessus du point le plus profond des pages ; sans descente, de chaque
  profondeur jusqu'au cadre de `page_layout` (`begin_page`,
  `init_resources`, `init_state`). Le document est ouvert et les lignes
  écrites à la profondeur de l'étape : leur mise en forme a ses propres
  cadres, que le port ne compte pas. Les balayages ont trouvé un écart du
  banc d'essai (une CMap prédéfinie chargée par une trace précédente du
  même fil restait en cache, trois cadres moins profonde que le premier
  chargement de Python) : `stages::py_process::start` vide les caches de
  `CMapDB`, comme un nouveau processus.
- **Mutants des cadres** (171 mutants à la main, chaque cadre, entrée,
  vérification `leaf`, appel de journalisation supprimé, chaque `leaf(n)`
  diminué ; `interp_unit`) : 39 tués, dont 30 par les seuls balayages
  (`do_Tj`, `do_TJ`, `do_'`, `do_"`, `render_string`, `do_S`... `do_b*`,
  `do_MP`/`do_DP`/`do_BMC`/`do_BDC`/`do_EMC`, `do_cm`, `do_Q`,
  `set_current_state`, `init_state`, `do_EI`, `device::init`,
  `device::set_ctm`, le `log.debug` de `process_page`, la compilation du
  motif de `re.sub`), 132 équivalents. Un cadre ne décide de rien quand
  aucune entrée ne peut en faire le premier point au-delà de la limite,
  ou quand le premier point serait ailleurs sans appel au périphérique
  entre les deux (même exception, rien d'enregistré, aucun état gardé) ;
  les 132 tombent dans ces cas, que les balayages confirment (aucune
  profondeur d'aucune page de 158 fichiers ne les distingue) :
  - après le parseur, plus courts que lui (mesure bornée : le premier
    `nextobject` de chaque `execute` descend à 8 cadres au moins, sur
    39 000 `execute` de `adv-interp` et de ses chaînes ; ces chemins à 7
    au plus) : le cadre des opérateurs qui n'appellent pas le
    périphérique et leurs appels (`safe_*`, `mult_matrix` de `do_cm`,
    avertissements et `log.debug` des opérateurs, `push`/`pop`/`pop_obj`,
    `keyword_name`, le `log.debug` par opérateur d'`execute`,
    `get_current_state`/`copy`/`PDFTextState()`/`reset` de `do_q`,
    `do_BT`, `do_Tf` et la police créée pour `{}`, `subinterp`/`dup` et
    les appels de `do_Do` avant `begin_figure`, `__contains__` de
    `do_EI`), le `re.sub` d'une image en ligne une fois le motif compilé,
    et dans `PDFTextDevice` les appels sans périphérique (`mult_matrix`,
    `is_multibyte`, `is_vertical`, `translate_matrix`, l'avertissement de
    `TJ`, `__getitem__` d'un flux) ;
  - avant le parseur, aussi profonds qu'un point antérieur sans appel au
    périphérique entre les deux : le `log.debug` de `render_contents`
    (puis `dict_value` → `resolve1`), celui de `init_resources`,
    `get_procset` et la police en cache (après le `log.debug` de la même
    ressource), le `PDFColorSpace()` de `get_colorspace` (après
    `stream_value`/`list_value` → `resolve1`), `PDFTextState()`, `reset`
    et `PDFGraphicState()` d'`init_state` (après `set_ctm`, aussi
    profond), les avertissements d'`execute` (suivis de la construction
    du parseur, plus profonde) ;
  - les corps de `PDFDevice` (`pass`) et `PDFTextDevice.render_char`
    atteints par `super()` sous le cadre d'une surcharge qui descend plus
    bas avant eux (pdfplumber les remplace, ou les atteint à moins de 8
    cadres d'`execute`), `close` (jamais appelé) ;
  - le `CMap()` de `get_cmap` pour un nom absent, après une recherche de
    `CMapDB` plus profonde (8 cadres contre 3, `api.jsonl`) ;
  - les unités C des entrées de boucle d'évaluation (`eval_entry` :
    `func(*args)`, appels de classes), qui ne décident rien : chacune
    précède un cadre Python et en prend deux, si bien qu'à 1 000 cadres il
    en reste plus de 7 000 sur 10 000. **Faux pour `func(*args)`
    d'`execute`** (corrigé par la revue, « Corrections de la revue ») : ses
    deux unités restent tenues pendant toute l'exécution imbriquée d'une
    Form, et le parseur de contenu appelle bien un `repr` récursif (les
    entrées impaires d'une image en ligne ou d'un dictionnaire, dans le
    message de `PSSyntaxError`) : au bas d'une chaîne de `d` Forms, il
    échoue `2d` niveaux plus tôt (fenêtres `climit-*`, mutant tué). Les
    unités des appels de classes ne sont tenues que le temps d'un
    constructeur, sans `repr` dessous : la justification tient pour elles.
- **Autres mutations** (`--in-diff` du chantier sur `pdfinterp.rs` et
  `pdfdevice.rs`, `--profile mutants -j 1`, rejeux `interp_unit` et, pour le
  parseur, `psparser_unit`) : passe ordinaire 180 mutants (136 tués, 11 non
  compilables, 33 survivants au premier passage) et 1 du parseur (tué) ;
  les survivants ont montré un trou du corpus (`color-spaces-*` : aucun
  `scn` ne s'exécutait, l'espace `INone` faisant lever `init_resources` à
  chaque page) et des chemins sans cas (opérandes laissés sur la pile,
  `/ProcSet` par référence, `/Rotate`), ajoutés à `adv-interp` ; relancés,
  24 tués, 15 équivalents : les bras de `Op::from_method` des opérateurs
  sans opérande qui ne font rien (`F`, `W`, `W*`, `ET`, `BX`, `EX`, `BI`,
  `ID` : un mot-clé non distribué ne dépile rien, comme un appel sans
  argument ; seul le cadre de la méthode diffère, masqué) ; `nargs > 0`
  en `>=` dans `execute` (`pop(0)` rend `[]`, même appel) ; `is_truthy`
  d'un entier (`wordspace` n'est un entier que pour une police
  multi-octets, et vaut alors `0` : ajouter `0` ne change ni la valeur ni
  le type, `x` ne valant jamais `-0.0`) ; dans `pop_obj`, `v < 0` et
  `neg < 0` en `<=` (zéro est rendu avant) ; `components[:-1]` de
  `do_SCN`/`do_scn` (`+`, `/`) : la branche des motifs non colorés est
  inatteignable (un espace nommé `Pattern` est le prédéfini, à une
  composante, donc une seule composante dépilée) ; la garde `KeyError` de
  `do_Do` sur `stream_value` (aucun chemin de `resolve1`/`getobj` ne lève
  `KeyError` : `getobj` rattrape celui de `get_pos`, `_get_objects` celui
  de `/N`, `PDFParser` celui de `/Length`). Passe `Ok(..)`
  (`scripts/mutants-ok.sh`) : 151 mutants, 110 tués, 22 équivalents (corps
  qui ne font que prendre leur cadre : `pass` de `PDFDevice`,
  `get_procset`, `do_gs`, `do_sh`, `do_F`, `do_W`, `do_W*`, `do_ET`,
  `do_BX`, `do_EX`, `do_BI`, `do_ID`), 19 non compilables, remplacés à la
  main (26 mutants, `hand_ok`) : tués, sauf `PDFTextState()` et
  `PDFGraphicState()` rendus sans leurs cadres (équivalents, ci-dessus) et
  `self.resources.copy()` remplacé par le même dict (personne ne modifie
  les ressources). `device::new`, sans appelant, a été supprimé.

### Chantier CONVERTER

`converter.rs` (`PDFLayoutAnalyzer`, `PDFPageAggregator`),
`PDFPageAggregatorWithMarkedContent` (`pdfplumber/src/page.rs`), le
comptage des cadres des constructeurs `LT*` dans `layout.rs`, l'étape Rust
`layout` (vidage brut et analysé partagé avec `layout_unit.rs`,
`RecordedIds` dans la bibliothèque, repli au-delà des rangs connus) et un
générateur unitaire qui n'a pas besoin de l'interpréteur :
`oracle/unit/converter.py` → `crates/pdftrace/tests/converter_unit.rs`. Il
mène en Python les pages réelles (`quick`, `upstream`) et les PDF de
`adv-interp` avec le `TracingDevice` de l'étape `interp` et enregistre,
en JSON typé, la suite des appels que l'interpréteur fait au
périphérique (profondeur d'imbrication 0 : les appels internes de
l'analyseur sont rejoués par le Rust), avec, pour chacun, la profondeur
de récursion Python du point d'appel (`at_depth`), les états
(`Named("PDFGraphicState", …)`, couleurs, espaces de couleur), les
flux d'images (attributs et données) et les polices sous la forme des cas
de `oracle/unit/fonts.py` (`[spec, table, errors]`, reconstruites en Rust
par le chargeur de `fonts_unit.rs`, à déplacer dans la bibliothèque) ; en
sortie : le résultat de chaque `render_char`, l'exception d'un appel, et
le `LTPage` brut puis analysé (`LAParams()` et les configurations de
`layout.py`, avec les `id()` de Python). Le Rust rejoue chaque appel sur
`PDFPageAggregatorWithMarkedContent` depuis la profondeur enregistrée,
une `Rc<PDFGraphicState>` par `render_string` d'origine.

#### Réalisé (branche `wave3/converter`)

- `converter.rs`, `PDFPageAggregatorWithMarkedContent` (`page.rs`,
  `marked_content`) et les corps triviaux de `pdfdevice::device`
  (`set_ctm` et les méthodes qui ne font que `pass` : le rejeu en a
  besoin ; INTERP les reprend tels quels à la jonction) sont portés,
  fonction par fonction, avec leurs cadres. Mesures (sondes bornées, puis
  frontières vérifiées par le générateur) : un appel de classe Python
  prend **trois** unités C (celle de `_PyObject_MakeTpCall` et l'entrée de
  la boucle d'évaluation de `__init__` : `layout::class_call`), alors que
  le code des vagues précédentes n'en compte que deux
  (`PDFContentParser::new`, `PDFPage::new`, `c_called_leaf` des
  `__init__`) ; sans effet tant que la limite C n'est pas approchée (elle
  ne l'est par aucun corpus), noté pour une reprise ; `super().m(*args,
  **kwargs)` de pdfplumber est un appel depuis le C (deux unités, une
  entrée de boucle) ; le premier `re.finditer(r"m[^m]+", …)` d'un processus
  compile le motif, huit cadres sous `PDFLayoutAnalyzer.paint_path`
  (`pycompat::re_cache`, remis à zéro au début de chaque trace par
  `stages::py_process::start`) ; `decode_text` prend les cadres de ses deux
  générateurs, et le décodeur `utf-16be` (`encodings.utf_16_be.decode`,
  appelé depuis le C) pour une chaîne qui commence par `\xfe\xff`.
- Écarts d'état non observables, documentés dans `converter.rs`
  (« Errors ») : après une exception d'un périphérique, l'interpréteur ne
  rattrape rien et pdfplumber jette le périphérique, donc `begin_figure`
  empile l'élément courant une fois la figure construite et `end_page`
  passe la page à `receive_layout` (Python la garde dans `cur_item`).
- Changements du contrat : `LTChar::new` prend le résultat de
  `char_width` tel quel (`Operand` : une police Type 3 dont la
  `/FontMatrix` tient une séquence lève l'exception de Python dans
  `__init__`, après les cadres de l'appel de classe) ;
  `PDFPageAggregator::create` et `PDFPageAggregatorWithMarkedContent::create`
  sont l'appel de classe (cadres de la chaîne des `__init__`), `new` reste
  sans cadre ; `CurItem::{add, last, type_str}`.
- L'analyse d'une page (`group_objects`, `group_textlines`,
  `group_textboxes`) ne compte toujours pas les cadres de ce qu'elle
  appelle (`layout.rs`, « Recursion ») : `end_page` ne l'appelle jamais
  près de la limite depuis l'interpréteur (`process_page` est peu
  profond) ; les frontières de `end_page` sont vérifiées sur des pages
  sans caractères.
- `pdftrace::font_case` (le chargeur des polices de `fonts_unit.rs`,
  déplacé) et `pdftrace::layout_dump` (les lignes de l'étape `layout` :
  `_RawDump`, `_analyzed`, `id_ranks`, et `RecordedIds`, que
  `layout_unit.rs` utilise aussi) ; l'étape Rust `layout` est écrite
  (`stages/layout.rs`, `run: Some`) et attend l'interpréteur.
- `oracle/unit/converter.py` → `converter_unit.rs` (natif et WASI) : voir
  `verification.md`. Les états graphiques sont partagés par valeur (une
  `Rc` par valeur distincte d'un cas : l'identité n'est pas observable) et
  les flux d'images vidés de leurs données (`LTImage` ne les lit pas ; la
  ligne `raw` montre donc `x""` dans le rejeu) ; l'ensemble réel est
  `all` (vingt premières pages), pas `quick`/`upstream` ; `adv-interp`
  s'y ajoutera à la jonction.
- **Statistique `CreationOrder`** (mesurée par le rejeu du corpus,
  `CONVERTER_CREATION_ORDER=1 cargo test --release -p pdftrace --test
  converter_unit corpus`, en attendant l'étape `layout` complète) : sur les
  3 657 pages des vingt premières pages de `all`, l'analyse
  `LAParams()` avec l'ordre par défaut de la bibliothèque donne le
  résultat de Python (lignes `ids` exclues) sur 3 413 pages (93,3 % ;
  deux passes identiques sur le corpus régénéré pour les sources finales,
  3 430 sur une version antérieure du générateur) ; les 244 autres ont des paires de boîtes à égale distance que l'ordre des
  `id()` de Python départage autrement.
- **Mutations** (sur le diff final `wave3/base..wave3/converter`,
  `--in-diff`, `-j 1`, `--profile mutants`, rejeux sans `corpus`) :
  - passe ordinaire de `converter.rs`, `page.rs`, `pdfdevice.rs`,
    `re_cache.rs`, `layout_dump.rs` (`converter_unit`, `--timeout 20`) :
    338 mutants, 122 tués, 207 non compilables, 7 délais, 2 survivants ;
    `pdfinternals.rs`, `font_case.rs`, `stages/py_process.rs`
    (`converter_unit`, `document_unit`, `fonts_unit`) : 30 mutants, 10
    tués, 20 non compilables ; `re_cache.rs` repassé avec `--timeout 180`
    (ses cinq délais venaient de la recompilation des crates en aval,
    comptée dans le temps du test) : 9 mutants, 5 tués, 4 non compilables.
    Les deux délais restants (`subpaths`, `+=` en `-=`/`*=`) bouclent ;
  - passes `Ok(..)` (`scripts/mutants-ok.sh`, diff des fichiers réécrits) :
    `converter.rs`, `layout.rs`, `pdfdevice.rs`, `re_cache.rs`,
    `layout_dump.rs`, `font_case.rs` (`converter_unit`, `layout_unit`,
    `fonts_unit`) : 60 mutants, 15 tués, 34 non compilables, 11
    survivants ; `page.rs`, `pdfinternals.rs` (`converter_unit`,
    `document_unit`) : 9 mutants, 7 tués, 2 non compilables ; les corps aux types sans `Default`
    remplacés à la main par une valeur concrète (`cur_item`, `apply_pt`,
    `render_char`, `get_result`, `apply_matrix_rect_counted`,
    `LTComponent::init`, `LTCurve::init`/`new_line`/`new_rect`,
    `LTLayoutContainer::init`, `LTFigure::new_obj`, `FontDoc::getobj`,
    et dans `page.rs` `marked_content::render_char` et
    `PDFPageAggregatorWithMarkedContent::create`) : 16 mutants, tous tués
    (`class_call`, dont le résultat n'est qu'un garde, est couvert par les
    mutants des cadres) ;
  - mutants à la main des cadres (chaque `py_frame`, `leaf`,
    `c_called_leaf`, `eval_entry`, `class_call`, `init_frames` ajouté
    retiré, `leaf(n)` décalé de ±1, `LTComponent::init` et
    `LTLayoutContainer::init` remplacés par `new`, `apply_matrix_rect`
    non compté, `SUBPATH_COMPILE_DEPTH` ±1) : 94 mutants, 56 tués, 38
    survivants (dont les 4 de `LTFigure::new`, ci-dessous). Ils ont
    fait ajouter les sondes de profondeur de `LTRect` (un rectangle fermé
    par `h` et par un `l` : les coins aléatoires des autres sondes font
    des courbes) et retirer le comptage de `LTFigure::new`, que seul
    `layout_unit` appelle (le convertisseur passe par `new_obj`).

  Survivants, tous équivalents :
  - **unités C seules** (`class_call` des constructeurs `LT*` et du
    périphérique, `eval_entry` des `super().m(*args, **kwargs)` de
    pdfplumber et de `decode_text`, `c_called_leaf` des reprises du
    générateur de `"".join` dont le cadre est couvert par la création du
    générateur) : quand une méthode du périphérique s'exécute, les unités C
    en cours sont celles de la pile d'appels de l'interpréteur (au plus
    trois par cadre Python, `func(*args)` compris) et une constante ; avec
    au plus 1 000 cadres Python, elles restent loin des 10 000 de
    `Py_C_RECURSION_LIMIT` : une vérification d'unités C ne peut échouer
    la première dans aucun état que pdfplumber atteint ;
  - **cadres dominés** : une vérification à la profondeur `d` précédée, dans
    le même cadre, d'une vérification à `d` ou plus profonde, sans rien
    entre elles qui puisse lever : la création du générateur de `paint_path`
    (suivie de sa reprise depuis `str.join`), les `cast` et
    `apply_matrix_pt` des compréhensions (après le générateur),
    `tag_cur_item` (après la méthode de `super()`, au même niveau, qui a
    réussi), `get_bound` de `LTCurve.__init__` (suivi de
    `LTComponent.__init__`), `get_any` de `LTImage.__init__` (après
    `set_bbox`, plus profond), `LTComponent.__init__`/`set_bbox` et le
    second `font.is_vertical()` de `LTChar.__init__` (après
    `apply_matrix_rect`/`apply_matrix_pt` aux mêmes profondeurs), et son
    premier `font.is_vertical()` : à l'horizontale `get_descent` suit
    aussitôt, à la verticale la police est une `PDFCIDFont`, dont le
    `to_unichr` appelé plus tôt par `render_char` atteint la même
    profondeur (`get_unichr`, ou `handle_undefined_char` → `log.debug`) ;
    une police verticale dont le `/W2` donne un `vy` qu'`LTChar` rejette
    vérifie la `TypeError` qui suit, à chaque profondeur ;
  - **inatteignables par le périphérique de pdfplumber** :
    `PDFLayoutAnalyzer.receive_layout` (`pass`, surchargée par
    `PDFPageAggregator`, comme dans tous les périphériques du port) et les
    corps par défaut de `pdfdevice::device` que
    `PDFPageAggregatorWithMarkedContent` surcharge (tous sauf `set_ctm` et
    `do_tag`, tués) : à couvrir à la jonction par un périphérique qui les
    garde (celui d'INTERP) ;
  - **`RecordedIds`** (`self.next - 1` en `+ 1` ou `/ 1`) : les ids de
    repli ne sont que comparés ; toute suite croissante d'ids neufs
    au-dessus des rangs enregistrés donne le même ordre.

  `stages/layout.rs` est muté à la jonction (ci-dessous).

### Jonction (branche `wave3/join`)

`wave3/interp` puis la fusion de `wave3/converter`. Conflits résolus en
gardant les deux intentions : les corps de `pdfdevice::device` d'INTERP
(`set_ctm` vérifie son cadre par `leaf(1)` là où CONVERTER le tenait :
rien n'est appelé dessous, même sémantique) ; `py_process::start` vide
`re_cache` et les caches de `CMapDB` ; `fonts_unit.rs` garde les deux
extractions (`fonts_report`, `font_case`). Ensuite :

- **Un seul cache de motifs `re`** : le `re.sub` de `get_inline_data` passe
  par `pycompat::re_cache::compile_frames` (`EOL_COMPILE_DEPTH` = 11) au
  lieu de son propre drapeau ; `pdfinterp::reset_process_state` disparaît.
- **Un seul modèle d'appel de classe** : `pycompat::recursion::class_call`
  (l'unité C de `_PyObject_MakeTpCall`, puis l'entrée de boucle de
  `__init__` : trois unités, mesurées par CONVERTER) et `class_call_leaf` ;
  les instanciations de `pdfinterp` (`PDFContentParser`, `PDFStream`,
  `PDFTextState`, `PDFGraphicState`, `CMap`, `PDFColorSpace`,
  `PDFPageInterpreter`) en comptaient deux ; `layout::class_call` en est
  la réexportation. Seules des unités C changent (inobservable depuis
  pdfplumber, voir les survivants « unités C seules »). Les constructeurs
  des vagues précédentes (`PDFPage`, `PDFParser`, `xref`...) gardent deux
  unités : même argument, à reprendre si une étape approche un jour la
  limite C.
- **Bout à bout** : l'étape Rust `layout` mène l'interpréteur porté dans
  `PDFPageAggregatorWithMarkedContent` puis l'analyse, avec l'ordre des
  `id()` lu dans la trace Python (`--oracle-dir`, passé par
  `oracle.verify`) ; `interp` et `fonts` tournent sur le périphérique de
  pdfplumber.
- **Fenêtres de profondeur** : `adv-interp` cherche aussi le premier niveau
  qui échoue pour `layout` (`DEPTH_STAGES`), dont le périphérique n'a pas
  les surcharges de trace d'`interp` et `fonts`.
- **Rejeu** : `oracle/unit/interp.py` trace aussi `interp`, `fonts` et
  `layout` (la trace de `layout` est donnée à l'étape Rust comme entrée de
  l'oracle) ; `interp_unit` vérifie donc le périphérique de pdfplumber, le
  convertisseur et l'analyse avec le vrai interpréteur, fenêtres de
  profondeur et entrées plus profondes comprises, et sert de rejeu aux
  mutants des étapes et des cadres.
- **CMaps paresseuses** (`tests/lazy_cmap.rs`) : sur une page dont la
  police `Type0` demande `90ms-RKSJ-H` et `to-unicode-Adobe-Japan1`, un
  fournisseur qui les retient (comme celui de JavaScript avant
  téléchargement) les voit demandées par `layout`, `interp` et `fonts`
  bien que pdfminer rattrape `CMapNotFound` ; la relance après remise des
  fichiers, avec un nouveau gestionnaire de ressources, donne la trace
  d'une exécution complète (celle de Python, vérifiée par
  `oracle.trace`). Depuis la revue, l'appel nul et sa relance tournent dans
  le même processus, comme à la frontière (`py_process::continuing`), avec
  l'état du processus remis entre les deux (`pdfminer::process_state`).

#### Résultats de la jonction

- **Parité des étapes** (`oracle.verify`, `--timeout 600`, 20 premières
  pages, code final) : `interp`, `fonts` et `layout` identiques sur `all`
  (1 853/1 853 chacune), `adv-interp` (279), `adv-pages` (285) et
  `adv-objects` (333) ; `interp_text` sur `adv-interp` (279) ; régression
  `psparser`, `objects`, `streams`, `pages` sur `quick` (150 chacune).
  Aucun délai, aucune erreur d'oracle. (Le corpus que la commande de la
  vague appelait `adv-content` est `adv-interp`.)
- **Rejeux** : `interp_unit` 3 165 traces d'étapes (contre 1 266 avant la
  jonction), `deep_sweeps` et `get_cmap` identiques ; `lazy_cmap` ; les
  autres rejeux par `scripts/check.sh`, vert sur le code final (natif et
  `wasm32-wasip1`, `converter_unit` avec son corpus : 366 s et 392 s ;
  construction JS et Wasm, tests Node, budget de taille, 45 auto-tests de
  l'oracle).
- **Statistique `CreationOrder`** (l'étape `layout` sans `--oracle-dir`,
  comparée page à page à la trace de Python, lignes `ids` exclues). Python
  ne s'accorde pas avec lui-même : sur une page où des boîtes sont à égale
  distance, l'ordre des `id()` (des adresses, que la graine de hachage et
  les allocations du processus déplacent) change d'une exécution à
  l'autre, et l'analyse avec lui. Mesure de la revue (`wave3/fix`, code
  final ; cache de l'oracle reconstruit, plus trois exécutions de l'étape
  Python avec `PYTHONHASHSEED` = 1, 2, 3, chacune dans un processus neuf ;
  les rangs des `ids` comptés dans le bloc de leur page, et non plus en
  appariant la k-ième ligne `ids` du fichier à sa k-ième page, ce qui
  décalait les pages d'un fichier dont une page avait levé avant son
  analyse, `poppler-90-0-fuzzed.pdf`) : sur les 3 659 pages des 20
  premières pages de `all`, 2 208 classent au moins deux boîtes ou groupes
  (ligne `ids` à deux entrées ou plus) ; 3 642 pages ont les quatre tirages
  (les 17 autres, de 14 fichiers à mot de passe, un seul) :
  - Python est stable sur ses quatre tirages pour 3 409 pages (93,6 %) :
    toutes les 1 437 pages à moins de deux boîtes, et 1 972 des 2 205 pages
    à deux boîtes ou plus (89,4 %) ; les 233 autres (10,6 %) donnent deux
    (122), trois (38) ou quatre (73) analyses différentes ;
  - sur les pages stables, `CreationOrder` donne le résultat de Python sur
    3 362 (98,6 % ; 1 925 des 1 972 à deux boîtes ou plus, 97,6 %) ;
  - sur les 233 pages instables, il donne l'un des tirages de Python sur
    124 (53 %) ;
  - contre le seul tirage du cache (ce que mesurait la jonction) : 3 436
    pages identiques sur 3 659 (93,9 %), 1 985 des 2 208 à deux boîtes ou
    plus (89,9 %), 1 759 fichiers sur 1 853 ; la jonction trouvait 93,6 %
    et 89,4 % sur un autre tirage : l'écart est celui de Python avec
    lui-même.

  Le tirage de référence est celui de l'étape, qui alloue le vidage brut
  avant l'analyse : il n'est pas celui de `pdfplumber.open(f,
  laparams={})` (autres allocations), qui varie de même (la revue :
  4 pages sur 101 instables entre deux processus du flux de pdfplumber,
  10 entre deux de l'étape, sur 60 fichiers enclins aux égalités). La
  parité n'en dépend pas : `pdftrace` reçoit le tirage du cache (`ids`). C'est
  une statistique, pas un critère de parité : la bibliothèque et l'API JS
  gardent `CreationOrder`.
- **Temps** (étape `layout`, 20 premières pages de `all`, 1 836 fichiers
  sans mot de passe, 3 640 pages ; Python : `seconds` des constructions du
  cache, un processus par trace, `-j 3` ; Rust : `_times.tsv`, un
  processus par lot) : Python 1 410 s (1 050 s sans les 0,196 s de
  démarrage et d'imports par processus), `pdftrace` 198 s ; par page
  387 ms (288 ms sans démarrage) contre 54 ms, soit 5,3 fois plus rapide
  hors démarrage (médiane par fichier 4,9 ; de 0,5 à 13,5 sur les 214
  fichiers de plus de 50 ms des deux côtés). Fichier le plus lent :
  `pdfjs-issue19624-text.pdf`, 6,7 s contre 43,7 s. Points chauds de
  `pdftrace --stage layout` (régions instrumentées sur une copie, temps
  inclusifs) : sur 150 fichiers tirés au hasard, l'écriture des lignes
  `raw` de la trace 65 % (le banc d'essai : `canon` de chaque attribut),
  l'interprétation avec le convertisseur 15 % (dont `render_string`
  5 %, `nextobject` 5 %, `LTChar` 4 %), les lignes `analyzed` 5 %,
  `group_textboxes` 2 % ; sur les 30 fichiers les plus lents, lignes
  `raw` 61 %, `group_textboxes` 13 % (78 % de `pdfjs-issue19624-text.pdf` :
  l'algorithme quadratique de pdfminer, tas de toutes les paires),
  interprétation 12 %. Pour la bibliothèque seule, l'interprétation et
  `group_textboxes` sont donc les cibles ; la trace n'existe que dans
  `pdftrace`.

- **Mutations** (`--profile mutants`, `-j 1`, `heavy.sh`) :
  - passe ordinaire du diff de la jonction (`--in-diff` : `pdfinterp.rs`,
    `recursion.rs`, `page.rs`, `layout.rs`, `py_process.rs` ; rejeux
    `interp_unit`, `psparser_unit`, `converter_unit` sans `corpus`,
    `lazy_cmap`, `layout_unit`) : 28 mutants, 1 tué, 27 non compilables ;
    passe `Ok(..)` (diff des fichiers réécrits) : 7 mutants, 2 tués, 4 non
    compilables, 1 survivant, `class_call_leaf` rendu `Ok(())` (ses sites :
    `PDFGraphicState()`, `PDFColorSpace(...)`, `PDFStream(d, data)`,
    `PDFPageInterpreter(...)`, tous dominés par un point antérieur au
    moins aussi profond — `set_ctm`, `resolve1`, la compilation du motif
    de `re.sub`, la chaîne d'`__init__` du périphérique —, plus des unités C
    seules) ; les non compilables `class_call` (des gardes d'unités C) et
    les corps de `get_cmap`, `get_colorspace`,
    `PDFPageAggregatorWithMarkedContent::create`, inchangés hors de la
    ligne de l'appel de classe, déjà remplacés à la main par INTERP et
    CONVERTER (tués) ;
  - passe ordinaire des étapes devenues réelles (`stages/layout.rs`,
    `interp.rs`, `fonts.rs`, `common.rs`, fichiers entiers ; `--lib`,
    `interp_unit`, `lazy_cmap`) : 234 mutants, 40 tués, 184 non
    compilables, 10 survivants ; repris avec `document_unit` (le rejeu des
    étapes `objects`/`streams`/`psparser`), `fetch_order` et `canon_id`
    sont tués ; `canon_opt`, sans appelant depuis la vague 2, est
    supprimé. Équivalents : `marked`/`marked_mut` de `FontsDevice` et
    `TracingDevice` rendant un état neuf (l'état des balises ne fait que
    remplir `mcid`/`tag` des objets `LT*`, que ces étapes n'écrivent pas ;
    `decode_text` est appelé quand même, et ne lève jamais : `IndexError`
    rattrapée) ; `page_frames > 0` en `>=` (`py_frames(0)` ne vérifie
    rien) ;
  - passe `Ok(..)` des mêmes fichiers : 42 mutants, 26 tués, 8 non
    compilables, 8 survivants, tous des méthodes de `FontsDevice` (et
    `receive_layout` de `TracingDevice`) qui délèguent au périphérique de
    pdfplumber et dont l'étape n'écrit pas l'effet : `begin_tag`,
    `end_tag`, `do_tag` (moins profonds que le parseur de la même
    `execute`, mesure d'INTERP ; `decode_text` ne lève pas), `end_page` et `receive_layout` (appelés depuis
    `process_page`, peu profond ; `laparams=None`), `paint_path` et
    `render_image` (aucune exception hors `RecursionError` pour des points
    numériques, et la frontière de `layout` sur `depth-subpaths` est celle
    d'`interp_text`, donc `paint_path` ne la décide pas). **Faux pour
    `paint_path`** (revue) : `depth-subpaths` dessine la dernière Form
    depuis la page avant la chaîne, ce qui compile le motif de
    `re.finditer` à faible profondeur ; sur `depth-subpaths-first`, où la
    première compilation est au bas de la chaîne, `fonts` et `layout`
    échouent un niveau avant `interp_text`, et le mutant est tué (« Corrections
    de la revue »). Les non
    compilables aux types sans `Default` remplacés à la main par une valeur
    (`render_char` de `FontsDevice` et de `TracingDevice` → `0.0`, `1.0`,
    `traced_render_char` → `0.0`, `page_layout` sans `process_page`) : 5
    mutants, tous tués ;
  - mutants des cadres à la main, contre les étapes réelles
    (`interp_unit`, `stage_traces` : `interp`, `fonts` et `layout` sur
    `adv-interp`, fenêtres et entrées plus profondes comprises) : chaque
    instruction qui portait un survivant d'INTERP ou de CONVERTER (149
    mutants : les 132 d'INTERP et les 34 de CONVERTER encore présents,
    regroupés par instruction, plus quelques instructions identiques des
    mêmes fonctions) et les 16 que la jonction a écrites (appels de
    classe de `pdfinterp`, `EOL_COMPILE_DEPTH` ±1, motif toujours ou
    jamais compilé, retour en cache de `re_cache::compile_frames` ±1 ou
    supprimé) : 165 mutants, 8 tués (les 7 de `re_cache`/`EOL`, et le
    cadre de `PDFContentParser.__init__`), 157 survivants : les
    survivants des deux chantiers le restent avec le vrai interpréteur et
    le vrai périphérique (leurs justifications tiennent), et les appels de
    classe de `pdfinterp` sont des unités C seules ou dominés (ci-dessus).

### Corrections de la revue (branche `wave3/fix`)

Constats de la revue de la vague 3, vérifiés un à un (scripts et mesures
de la revue sous `/tmp/claude-review-w3-*`) :

- **Premier `str(x)` de `literal_name`** (psparser.py:121) : exécuté une
  première fois sous la forme générique de `CALL`, il prend une unité C de
  plus (`pycompat::specialize`, « Profondeur de récursion »). Avant la
  correction, un `Tf`, `Do`, `CS`/`cs` dont l'opérande est imbriqué au
  premier niveau qui échoue en Python (9 994 pour un tableau sous `Tf`,
  9 993 pour un dictionnaire, 9 996 pour une ressource `/ColorSpace`)
  passait en Rust. Corrigé ; vérifié par les fenêtres `climit-*`
  d'`adv-interp` (dont deux pages identiques : la première échoue, la
  seconde exécute l'appel spécialisé) et par les cas `deep_first` de
  `oracle/unit/pdftypes.py` (chacun dans un fils où le site n'a jamais
  tourné : 21 des 72 cas diffèrent sans la correction). L'état « site
  exécuté » est un état de processus : `py_process.jsonl` le note
  (`call_sites`), `oracle/codegen/py_symbols.py` vérifie que les imports
  de l'oracle n'exécutent aucun site, et les générateurs qui passent par ce
  site près de la limite (`pdftypes`, `psparser --fuzz`, `cmaps`) l'exécutent
  d'abord (`warm_call_sites`), ce que leurs rejeux reproduisent.
- **Unités C de `func(*args)` sous une chaîne de Forms** : voir « Mutants
  des cadres » ci-dessus ; fenêtres `climit-inline-odd-<d>` et
  `climit-dict-odd-<d>` (`d` = 0, 1, 50, 300 Forms : 9 995, 9 993, 9 895,
  9 395 en Python, deux unités par niveau).
- **`paint_path` de pdfplumber au point le plus profond** : sur une chaîne
  dont la dernière Form peint plusieurs sous-chemins sans que la page
  l'ait peinte avant (`depth-subpaths-first` : le premier `re.finditer` du
  processus compile son motif là), `fonts`, `interp` et `layout` échouent
  à 327 et `interp_text` à 328 ; la justification « la frontière de
  `layout` sur `depth-subpaths` est celle d'`interp_text` » ne valait que
  pour une chaîne dont la page a peint la dernière Form d'abord.
  `depth-subpaths-compiled` (le motif compilé par la page) complète.
- **Pile Wasm** : voir « Profondeur de récursion » (8 Mio,
  `scripts/wasm-depth-gate.sh`).
- **CMaps paresseuses et état du processus** : la relance d'un appel nul
  repart de l'état du processus d'avant l'appel
  (`pdfminer::process_state`, `crates/wasm/src/ops.rs`) ; `lazy_cmap.rs`
  mène l'appel nul et la relance dans un même processus et vérifie, sur des
  chaînes de 318 à 336 Forms dont la dernière montre la première image en
  ligne du processus, que la relance est l'exécution complète, et que sans
  la remise elle ne l'est pas à certaines longueurs.
- **Journalisation par défaut** (constat écarté pour le port, documenté) :
  dans la configuration par défaut de Python, un `log.warning` de
  pdfminer est émis et formate le `%r` de ses arguments ; la référence
  reste la journalisation de pdfminer coupée (`verification.md`,
  « Étapes » ; la docstring de `oracle/stages/__init__.py`, qui disait
  le contraire, est corrigée).
- **Statistique `CreationOrder`** : refaite avec la variation de Python
  lui-même (ci-dessous).
- **Mutants à la main repassés** contre les nouvelles fenêtres (`climit-*`,
  `depth-subpaths-first`, `depth-subpaths-compiled`, rejeu `interp_unit`
  `stage_traces` restreint à elles, `INTERP_ONLY`) : chaque survivant de la
  jonction qui ne retire que des unités C (`eval_entry`, `class_call`,
  `class_call_leaf`, `c_called_leaf` de `pdfinterp`, `pdfdevice`,
  `converter`, `layout`, `page.rs`, `pdfinternals`), les cadres du chemin
  de `paint_path` de pdfplumber (`converter::paint_path`,
  `marked_content::paint_path`, `tag_cur_item`, le corps de
  `PDFDevice.paint_path`) et le survivant de la passe `Ok(..)`
  `FontsDevice::paint_path` : 31 mutants, 2 tués (`func(*args)`
  d'`execute`, par `climit-tf-list-9994` : ses unités sont tenues pendant
  le `str()` de l'opérande de `do_Tf` comme pendant toute Form imbriquée ;
  `FontsDevice::paint_path` → `Ok(())`, par `depth-subpaths-first-326` de
  `fonts`), 29 survivants. Ceux-ci restent équivalents : leurs unités ne
  sont tenues que le temps d'un constructeur ou d'un appel au
  périphérique, sous lequel aucun `repr` récursif ne s'exécute (le
  `repr`/`str` d'un opérande profond a lieu dans le parseur ou dans
  l'opérateur, avant), et les unités en cours y restent à trois au plus
  par cadre Python, loin des 10 000 ; les cadres de `paint_path` et de
  `tag_cur_item` sont dominés (« Chantier CONVERTER », cadres dominés : la
  compilation du motif, huit cadres plus bas, ou la méthode de `super()`
  au même niveau passent avant).
- **Parité** (code final, `oracle.verify --timeout 600`, caches de
  l'oracle reconstruits : `oracle/stages/__init__.py`, `oracle/depth.py`
  et `layout.py` ont changé) : `interp`, `fonts`, `layout` identiques sur
  `all` (1 853 chacune) ; `interp_text`, `fonts_text`, `interp`, `fonts`,
  `layout` sur `adv-interp` (353 : 279 et les fenêtres `climit-*`,
  `depth-subpaths-first`, `depth-subpaths-compiled`) ; `interp`, `fonts`,
  `layout` sur `adv-pages` (285) et `adv-objects` (333) ; régression
  `psparser`, `objects`, `streams`, `pages` sur `quick` (150). Toutes les
  données unitaires régénérées (`oracle/depth.py` est dans les sources de
  chaque générateur), tous les rejeux identiques.
- **Mutations** (`--profile mutants`, `-j 1`, `heavy.sh` ; diff
  `e226e54..wave3/fix` de `specialize.rs`, `logging.rs`, `re_cache.rs`,
  `psparser.rs`, `cmapdb.rs`, `process_state.rs`, `py_process.rs`,
  `common.rs`, `wasm/src/ops.rs` ; rejeux `--lib`, `pdftypes_unit`,
  `psparser_unit`, `cmaps_unit`, `lazy_cmap`, `interp_unit`,
  `document_unit`, `js_runtime_unit`) : passe ordinaire 57 mutants, 34
  tués, 19 non compilables, 4 survivants (`py_symbols_restore` et
  `logging::restore` rendus vides, `PyProcess::from_value` rendu par
  défaut ou cherchant le site par `!=`), tués après l'ajout des tests
  unitaires de `process_state` (chaque partie remise, lue à la profondeur
  des mêmes appels) et de `py_process` (un état enregistré non vide) : 5
  mutants repassés, 5 tués ; passe `Ok(..)` : 5 mutants, 4 tués
  (`literal_name`, `terminated`, `str_call` → `Ok(None)`), 1 non
  compilable (`Ok(Some(Default::default()))` : `Guard` sans `Default`),
  remplacé à la main par un site jamais spécialisé (tué par `deep`). Les
  `build.rs` (la pile Wasm) ne changent rien en natif : c'est
  `scripts/wasm-depth-gate.sh` qui les vérifie (avec 1 Mio, le profil
  `dist` piège sur chaque fenêtre `depth-*` d'`adv-interp`).
- **`scripts/check.sh`** vert sur le code final (natif, `wasm32-wasip1`,
  la porte `wasm-depth-gate` : 188 fenêtres d'`adv-interp` × `layout`,
  `interp`, `fonts`, 79 d'`adv-pages`, 23 d'`adv-objects`, le profil
  `dist` en Wasm identique au natif ; paquet JS et construction Wasm
  `dist`, budget de taille ; auto-tests de l'oracle).
- **Finalisation** (revue de l'intégration) : `interp_text` et
  `fonts_text` repassés sur `all` (1 853 identiques chacune, caches de
  l'oracle reconstruits) ; les deux tests d'appel nul de
  `js_runtime_unit` (`a_void_call_*`), qui sortaient sans rien vérifier en
  Wasm, y sont ignorés dans la passe commune (le saut se voit dans les
  comptes) et relancés seuls par `scripts/check.sh`, un processus WASI
  chacun (pas de fil neuf par test en Wasm : les CMaps chargées
  resteraient dans le fournisseur et les caches de `CMapDB`) ; la
  comparaison Wasm/natif sur `quick` est versionnée
  (`oracle/wasm_parity.py`, fournisseur de CMaps et `--oracle-dir`
  passés explicitement, `verification.md`, « Définition de « fini » »).

## Vague 4 : la couche page de pdfplumber

Base commune : branche `wave4/base` (depuis `rewrite/rust-core` à
`762bbe5`). Deux chantiers parallèles, PAGE et DOC, partent de cette base
et ne modifient pas le contrat ci-dessous sans accord : un changement d'une
étape Python invalide son cache et celui des étapes qui en dépendent
(`plumber_text`, `plumber_tables` et `plumber_doc` dépendent de
`plumber_objects`, les deux dernières de `plumber_text`).

### Ce que contient la base

- **La spécification** : les étapes Python `oracle/stages/plumber_objects.py`,
  `plumber_text.py`, `plumber_tables.py`, `plumber_doc.py` (parcours et
  format des lignes en tête de chaque fichier). Elles mènent pdfplumber
  comme un utilisateur : `pdfplumber.open(path, pages=[1..N],
  password=..., **options)` (`--max-pages` devient l'option `pages` :
  `PDF.pages` parcourt tout l'arbre des pages et ne construit que les
  `Page` choisies), pages paresseuses, aucune rustine. Les options
  (opérations de texte, réglages de tables, pages dérivées, sérialisations,
  options d'ouverture) sont des tables de données ; chaque page en prend une
  tranche déterministe (`select` : décalage tiré du SHA-256 du fichier et du
  rang de la page), si bien que le corpus couvre toutes les variantes et
  qu'une trace reste bornée. Les tables passent au Rust par
  `oracle/codegen/plumber_ops.py` → `crates/pdftrace/src/stages/plumber_ops.rs`
  (JSON typé ; `--check` dans `scripts/check.sh`) ; les aides partagées des
  quatre étapes sont dans `stages/plumber_common.rs` (`select`, `exc`,
  `digest`, `bounded`, tables).
- **Les interfaces Rust**, squelettes qui compilent (corps `todo!()` avec
  la référence upstream, la paresse, le cache et le plan de comptage des
  cadres en commentaire) : `pdf.rs`, `page.rs`, `container.rs`,
  `convert.rs`, `structure.rs`, `utils/exceptions.rs`, le reste de
  `utils/pdfinternals.rs`.
- **Changements partagés, faits dans la base** : `PyErr::wrapping` et
  `PyErr::cause` (`PdfminerException(e)` garde `e` : `str()` identique, la
  classe de la cause visible en JavaScript ; les traces écrivent `!PdfminerException("…")
  cause !PDFSyntaxError("…")`) ; `PyValue::Page(PageRef)` (le `/P` des
  annotations, que `Page.annots` remplace par la page : `<Page:n>`, égalité
  par identité ; écrit `Page{"page_number": n}` dans les traces et
  `{"page_number": n}` en JavaScript en attendant l'API des pages) ;
  `page_layout` (le corps de `Page.layout`) déplacé de `pdftrace` dans
  `pdfplumber::page` (les étapes `interp`, `fonts`, `layout` l'importent) ;
  `oracle.verify` passe toujours `--cmap-dir` (outillage, ci-dessous).
- **Laissé au premier commit de PAGE**, parce que ces fichiers sont dans
  l'empreinte de tous les caches (`oracle/depth.py` : de toutes les
  étapes de l'oracle et de toutes les données unitaires ;
  `oracle/corpus.py` : de toutes les données unitaires, 2,3 Go, des heures
  à régénérer) et qu'il vaut mieux les invalider une seule fois, avant
  toute construction de cache de la vague : le site d'appel `decode_text
  str(s)` dans `_call_sites` de `oracle/depth.py`, et un nom `adv-<nom>`
  pour un générateur `oracle/adversarial/<nom>.py` qui n'est pas une étape
  (`oracle/corpus.py` n'accepte aujourd'hui que `adv-<étape>` ; il faut
  `adv-plumber` et `adv-structure`). DOC rebase sur ce commit.

### Décisions

- **Journalisation** : la référence reste la journalisation coupée
  (« Étapes » de `verification.md`), y compris le `logger.warning` de
  `PDF.__init__` (logger `pdfplumber.pdf`). Le `warnings.warn` de
  `Page.annots` (`raise_unicode_errors=False`) garde les filtres par défaut
  de Python : montré une fois par texte, catégorie et ligne
  (`pycompat::warnings`, registre comparé à Python par `diagnostics.jsonl`
  de `oracle/unit/js_runtime.py`) ; ses cadres sont à mesurer.
- **`id()` du layout** : toute étape `plumber_*` qui analyse (`laparams`)
  prend le rang des `id()` dans la trace Python (`ORACLE_INPUT`,
  `--oracle-dir`), comme `layout`. Seule `plumber_objects` analyse (sa
  seconde ouverture, `laparams={}`) ; l'ordre brut des caractères n'y étant
  pas visible, une boîte est désignée par son contenu (empreinte de ses
  caractères `LTChar{…}` triés, un groupe de celles de ses éléments) : deux
  éléments de même empreinte sont interchangeables (mêmes caractères, même
  géométrie). La bibliothèque reçoit l'ordre par `OpenOptions::ids`
  (`IdsFactory`) ; la bibliothèque et l'API JS gardent `CreationOrder`.
  Sonde de la base (40 fichiers de `quick` riches en images, annotations,
  structure et tables, chaque étape tracée deux fois avec
  `PYTHONHASHSEED` = 1 et 98765) : `plumber_text`, `plumber_tables` et
  `plumber_doc` identiques ; `plumber_objects` ne diffère que par ses
  lignes `ids` sur 12 fichiers (les objets écrits avant elles identiques :
  les adresses changent, pas forcément l'analyse), comme l'étape `layout`.
- **Recherche** : `plumber_text` écrit, avant chaque résultat de `search`,
  les correspondances du `re` de Python (`spans`, `ORACLE_INPUT`) ; le Rust
  les donne à `Page::search` et recopie la ligne (« Frontière
  JavaScript » : `re` n'est pas porté).
- **Images en ligne** : `"inline-<k>"` dans le port, `inline{"n": k}` dans
  les traces ; atteint `page.images[].name` et, avec `laparams`, le `name`
  des figures (écart documenté, `public-api.md`) ; unique dans
  l'interprétation d'une page seulement : répété d'une page à l'autre dans
  `pdf.images` et `pdf.objects`.
- **Hors périmètre** (en-têtes des modules, `public-api.md`) :
  `display.py` (`to_image`, pypdfium2 et PIL), `repair.py` (Ghostscript
  dans un sous-processus : `repair=True` lève `NotPorted`), `cli.py`. Les
  autres options de `pdfplumber.open` sont portées : `pages`, `laparams`,
  `password`, `strict_metadata`, `unicode_norm`, `raise_unicode_errors`.
- **Paresse et caches** : ceux de pdfplumber exactement (`PDF.pages` et
  sa liste partielle après une exception, `Page.layout`, `Page.objects`,
  arêtes, `lru_cache` de `get_textmap`, `pdf.objects` sur les mêmes dicts,
  `doctop` cumulé sur les pages choisies, `flush_cache`/`close`) : ils
  décident l'ordre, l'identité, la récursion et l'état des flux
  (`rawdata` vidé par un décodage).
- **Pilotage** : `Page::layout` mène `page_layout` (un
  `PDFResourceManager` par document, dans `PdfCore`) ; les étapes de
  `pdftrace` appellent la bibliothèque (`PDF::open`, `Page`).
- **Mesures de la spécification** (`--set quick`, `heavy.sh`,
  `--oracle-only -j 3 --timeout 600`, 150 fichiers, aucune erreur ni
  délai) : `plumber_objects` 86 s (218 s cumulées, 188 Mo de traces ;
  309 Mo sans les références `lchars`), `plumber_text` 113 s (249 s,
  38 Mo), `plumber_tables` 90 s (229 s, 33 Mo), `plumber_doc` 110 s
  (275 s, 4 Mo) ; fichiers les plus lents : `highlights.pdf` 53 s
  (`plumber_text`), `pdfjs-issue5509.pdf` 49 s (`plumber_doc`). Sur
  `all` (×12) : environ 3 h 15 de calcul cumulé (1 h 10 avec `-j 3`) et
  3,2 Go de cache ; construire avec `--timeout 600`.

### Contrat entre les chantiers

Fixé dans `wave4/base` (signatures et sémantique documentées dans le code) :

- `pdfplumber::pdf` : `Source` (`Path` : fichier ouvert par le `PDF`,
  `Stream` : objet fichier de l'appelant, `FileKind`), `OpenOptions`,
  `IdsFactory`, `PdfCore` (`doc`, `rsrcmgr`, `laparams`, `unicode_norm`,
  `raise_unicode_errors`, `ids`), `PDF` (`open`, `close`, `pages`,
  `objects`, `metadata`, `annots`, `hyperlinks`, `structure_tree`,
  `PdfCache`).
- `pdfplumber::page` : `page_layout`, `Page` (champs dans l'ordre de
  `__init__`, `PageKind::{Original, Cropped, Filtered}`, `CropFn`,
  `FilterFn`, `PageCache`, `TextMapCache`, `SearchArgs`, `as_value`) et
  ses méthodes (`new`, `layout`, `objects`, `process_object`,
  `iter_layout_objects`, `parse_objects`, `point2coord`, texte, tables,
  `crop`, `filter`, `dedupe_chars`, `annots`, `hyperlinks`,
  `structure_tree`, `close`, `repr`), `fix_fontname_bytes`,
  `tuplify_list_kwargs`, `test_proposed_bbox`, `ALL_ATTRS`,
  `CP936_FONTNAMES` ; `impl Container` et `impl TablePage`.
- `pdfplumber::container` : `Container` (méthodes requises `pages`,
  `objects`, `to_dict`, `container_cache`, `attribute`, `flush_cache` ;
  fournies : listes par sorte, arêtes, `to_json`, `to_csv`), `Objects`,
  `ContainerCache`, `SerializeOptions`.
- `pdfplumber::convert` : `Serializer`, `AttrFilter`, `get_attr_filter`,
  `to_b64`, constantes.
- `pdfplumber::structure` : `PDFStructTree` (`new(doc, Scope)`,
  `element_bbox`, `find_all`, `find`…), `PDFStructElement` (`all_mcids`,
  `to_dict`, `repr`, `find_all`, `find`), `Matcher`, `Scope`.
- `pdfplumber::utils::exceptions` : `wrap`, `pdfminer_exception`,
  `malformed_pdf_exception` ; `pdfinternals` : `decode_text_value`,
  `resolve_and_decode`, `decode_psl_list`.
- `pdftrace::stages` : `plumber_common` (`select`, `exc`, `digest`,
  `bounded`, `table`), `plumber_ops`, `StageInput::oracle` pour
  `plumber_objects` (`ids`) et `plumber_text` (`spans`).
- Qui écrit quoi : PAGE écrit `pdf.rs` hors métadonnées, `page.rs` hors
  `annots`/`hyperlinks`/`structure_tree`, `container.rs`, `convert.rs`, le
  reste de `pdfinternals.rs` (en premier : DOC en a besoin, avec le site
  d'appel `decode_text str(s)`) ; DOC écrit `structure.rs`, les
  métadonnées de `PDF::open`, `Page::annots`, `hyperlinks`,
  `structure_tree`, `PDF::annots`, `hyperlinks`, `structure_tree`. Chacun
  teste avec les étapes de l'autre au fur et à mesure qu'elles passent ;
  jusque-là, un rejeu unitaire de son côté (pages construites par les
  objets d'un PDF, par exemple).

### Chantier PAGE

`pdf.py`, `page.py` (objets, layout, texte, tables, pages dérivées),
`container.py`, `convert.py` (avec `pycompat::json` : l'encodeur de
`json.dumps` de CPython, et `pycompat::csv` : `csv.DictWriter`, guillemets
minimaux, `\r\n`), le reste de `pdfinternals.py` ; les étapes Rust
`plumber_objects`, `plumber_text`, `plumber_tables` ; le corpus adverse
`oracle/adversarial/plumber.py` → `adv-plumber` (vérifié par les quatre
étapes) : objets de toute sorte et attributs atypiques (`mcid` imbriqué à
10^4 niveaux et autour de la limite des unités C, `MCID` de tout type,
`/ColorSpace` d'image en tableaux imbriqués autour de la limite des
cadres de `resolve_all`, noms de polices en `bytes` dont les cinq de
`CP936_FONTNAMES`, mediabox qui ne commence pas à 0, `/Rotate` de 0 à
270, boîtes inversées ou dégénérées, Form XObjects imbriqués, images en
ligne nommées comme le port les nomme), `pages=` de tout type, `doctop`
sur pages choisies, `laparams` de tout type (clé inconnue, valeurs hors
domaine), pages aux égalités de distance de boîtes, textes et tables
dégénérés, `cases_depth` pour chaque chemin récursif
(`iter_layout_objects` sur des figures imbriquées, `resolve_all`,
`get_text` des conteneurs, `Serializer` et l'encodeur JSON).

#### Réalisation (branche `wave4/page`)

- **Bibliothèque** (`crates/pdfplumber`) : `pdf.rs` (`PDF.open` et
  `__init__` : `LAParams(**laparams)` avec les `TypeError` de `**` et des
  mots-clés inconnus, `_validate`, le mot de passe, `PdfminerException`
  de `PDFDocument(...)` ; `pages` avec `iter_pages` repris par
  `enumerate` et `next(gen)` repris du C, `CreatePages::resumed_from_c`
  ajouté à `pdfpage.rs` ; `doctop` cumulé sur les pages choisies, `page_number
  not in pp` par `py_contains` ; `objects`, `close`, `to_dict`,
  `getattr`) ; `page.rs` (`Page.__init__`, `DerivedPage`, `CroppedPage`,
  `FilteredPage` dans un `Page` à `PageKind` ; `layout` mène
  `page_layout` avec l'`IdSource` de `OpenOptions::ids` ; `objects`,
  `parse_objects`, `iter_layout_objects` et sa chaîne de générateurs,
  `process_object` pour chaque objet du layout, `LTAnno` compris, dans
  l'ordre de `__dict__` ; `point2coord` ; texte, recherche, tables ;
  `get_textmap` et son `lru_cache` (128 entrées, clés hachées comme
  Python : `x_tolerance=1` et `1.0` partagent un `TextMap`) ; `crop`,
  `within_bbox`, `outside_bbox`, `filter`, `dedupe_chars` ; `to_dict`,
  `getattr`, `flush_cache`, `close`) ; `container.rs` (listes, arêtes en
  cache, `to_json`, `to_csv`) ; `convert.rs` (`Serializer`,
  `get_attr_filter`, `to_b64`) ; le reste de `pdfinternals.rs`
  (`decode_text` de toute valeur avec le site `DECODE_TEXT_STR`,
  `resolve_and_decode`, `decode_psl_list`) ; `obj.rs` :
  `PyValue::repr_counted`/`str_counted` (une unité C par valeur, les
  objets PDF par `PDFObject::repr`). `pycompat` : `json` (les deux
  encodeurs de `json.dumps` : celui en C sans `indent`, une unité par
  conteneur et une pour l'appel de l'encodeur ; celui en Python avec
  `indent`, un cadre par conteneur et un par flottant, `floatstr`),
  `csv` (le `writerow` de `_csv.c` : guillemet doublé, `\` échappé par
  lui-même sans guillemets, `,`, `\r`, `\n` entre guillemets, un seul
  champ vide écrit `""`), `base64::b64encode`, `PyStr::replace`.
- **Fichier fermé** : `PDF.close` ferme le fichier qu'il a ouvert
  (`PDFDocument::close_file`) ; `pycompat::io::BytesIO` partage un état
  « fermé » entre ses copies et son `seek` lève alors l'erreur de CPython
  (`seek of closed file` pour un `BufferedReader`, vérifiée avant la
  conversion de la position, `I/O operation on closed file.` pour un
  `BytesIO`) : relire après `close()` un objet jamais lu lève
  `ValueError` comme en Python (`closed_objects` de `plumber_doc`).
- **Balayages de profondeur** (`oracle/unit/plumber_sweep.py`,
  `tests/plumber_sweep.rs`) : les étapes appellent les méthodes de page
  loin de la limite, et les fenêtres `depth-*` ne l'atteignent que dans
  l'interprétation, `resolve_all` et `Serializer`. Chaque opération de la
  couche page est donc lancée sur une page neuve depuis chaque profondeur
  entre la place de son cadre le plus profond et aucune : une
  `RecursionError` arrive au premier instant où l'opération dépasse la
  limite, si bien que chaque cadre sur le chemin de chaque profondeur
  atteinte est le premier au-delà dans une des traces. Ils ont trouvé les
  cadres que le port ne comptait pas : les annotations des fonctions
  imbriquées, évaluées à chaque appel de la fonction englobante
  (pdfplumber n'importe pas `annotations` de `__future__`) et dont chaque
  indice d'alias de `typing` passe par le `lru_cache` de `_tp_cache`,
  plusieurs cadres plus bas la première fois dans un processus
  (`pycompat::typing_cache`, état de processus comme `re_cache` ;
  `iter_pages` de `PDF.pages`, `rotate_point` de `Page.annots`,
  `process_attr`, `sorter`) ; `pathlib.Path(path)` de `PDF.open` ;
  `TableFinder.__init__`, `get_edges` et `Table.extract`, au-dessus des
  propriétés de la page qui l'interprètent ; les `calculate_area` et
  `get_bbox_overlap` de `test_proposed_bbox`.
- **Cadres mesurés** (`oracle/depth_probe.py`, sous limites) :
  `lru_cache()(f)` de `Page.__init__` et `DerivedPage.__init__` (`lru_cache`
  un cadre, puis `decorating_function` → `update_wrapper`, deux),
  l'appel du `lru_cache` (une unité C, `_PyObject_MakeTpCall`, puis
  `_get_textmap` appelé depuis le C en cas d'échec), `re.sub(lt_pat, ...)`
  de `process_object` (`re.sub` → `re._compile`, deux cadres, le motif
  déjà compilé), `json.dumps` (`dumps` → `encode` → `iterencode`, puis
  `_iterencode` repris par `list()` depuis le C et un cadre par
  conteneur), `csv.DictWriter` (`writeheader` → `writerow` →
  `_dict_to_list`, dont l'expression génératrice est reprise par le
  `writerow` en C ; `writerows` appelle `_dict_to_list` depuis le C).
- **Unités C des comparaisons** (`obj.rs`) : non comptées, à dessein.
  Une valeur d'objet de page ne dépasse pas quelques centaines de niveaux
  d'imbrication : ce qui vient du document passe par `resolve_all` (deux
  cadres par niveau de liste, un par niveau de dict : `RecursionError`
  vers 490 et 990 niveaux), et le seul attribut qui n'y passe pas, `dash`,
  n'est jamais comparé (les arêtes de tables qui le copient ne sont
  comparées qu'à leurs coordonnées) ; sa sérialisation (`to_json`,
  `to_csv`) lève `RecursionError` aux cadres de `Serializer` (trois par
  niveau) bien avant 10 000 unités. Le cas `mc-mcid-deep` d'`adv-plumber`
  (un `MCID` imbriqué 10^4 niveaux, en tableaux et en dicts) le vérifie.
- **Écarts documentés** (`docs/public-api.md`, « Not Ported ») : une
  marge de `laparams` qui n'est pas un nombre (Python la garde et échoue à
  son premier usage dans l'analyse : `NotPorted` à l'ouverture) ; un mot
  de passe qui n'est ni `None` ni une `str` (`NotPorted`) ; `getattr` d'une méthode dans `to_dict`/`to_csv`
  (`object_types=["extract_word"]` : une méthode liée, `NotPorted`) ; un
  XObject nommé comme le port nomme les images en ligne (`inline-<k>` :
  les deux noms se confondent) ; l'ordre d'un NaN dans un `set` de
  `cluster_objects` (son adresse en CPython, `utils/clustering.rs`).
- **Étapes** : `plumber_objects` (avec `ContentIds` : le rang des `id()`
  de la trace Python par clé de contenu, donné à l'analyse par
  `OpenOptions::ids`), `plumber_text` (les `spans` de la trace Python
  donnés à `Page::search`, le `TextMap` calculé où `search_spans` le
  calcule), `plumber_tables` ; `plumber_common` : `open_pdf`, `Names`
  (un nom d'image n'est en ligne que si son flux n'a pas d'identifiant
  d'objet ; les noms les plus longs remplacés d'abord dans un texte),
  `ObjectsWriter`, `box_lines`. Chaque générateur et fonction des étapes
  Python prend son cadre, pour que pdfplumber s'exécute à la profondeur
  où Python l'exécute.
- **Limite de la clé de contenu des `id()`** : deux boîtes de texte
  identiques (même texte dessiné deux fois au même endroit, en gras
  simulé) ont la même clé ; la base les suppose interchangeables, mais
  l'analyse ne l'est pas tout à fait (l'ordre d'insertion du `Plane` et
  la position dans la liste des boîtes départagent aussi). Sur `all`, un
  fichier le montre, `pdfjs-issue19624-text.pdf` (page 2 : 55 clés
  dupliquées, jusqu'à 31 copies) : Python y donne une trace différente à
  chaque exécution (cinq exécutions, cinq traces : les adresses
  changent), et le port, avec le rang des `id()` d'une trace, ne
  reconstruit pas toujours les mêmes groupes (les objets sont identiques
  sur trois tirages sur quatre, la ligne `ids` diffère). Le corriger
  demande une clé qui distingue les doublons (un rang dans l'ordre brut
  des caractères, invisible à travers pdfplumber), donc un changement de
  l'étape Python et de tous ses caches.
- **Corpus et données unitaires** : `oracle/adversarial/plumber.py` →
  `adv-plumber` (boîtes et rotations, géométrie aux bords des pages
  dérivées, chemins dégénérés et infinis, tirets et couleurs, images,
  contenu marqué de toute sorte, polices dont les noms `CP936`,
  doublons, ligatures, surrogates isolés, tolérances des mots et des
  tables, texte tourné et vertical, milliers de petits objets, annotations
  malformées, égalités de distance du layout, figures imbriquées ;
  `cases_depth` : chaînes de Form XObjects, `/ColorSpace` et `/MCID`
  imbriqués en tableaux ou par chaînes de références, tirets imbriqués pour
  `Serializer`) ; `oracle/unit/plumber.py` (les traces des quatre étapes
  `plumber_*` sur `adv-plumber`, `micro` et les petits fichiers de
  `quick`, les fenêtres de profondeur aussi depuis un et deux cadres plus
  bas ; `open.jsonl` : `pdfplumber.open` avec des `laparams`, `pages`,
  `password`, `unicode_norm` de tout type ; `utils.jsonl` :
  `decode_psl_list`, que pdfplumber n'appelle pas lui-même), rejoué par
  `crates/pdftrace/tests/plumber_unit.rs` ; `oracle/unit/plumber_sweep.py`
  (les balayages de profondeur ci-dessus, 396 sur six fichiers
  d'`adv-plumber`), rejoué par `crates/pdftrace/tests/plumber_sweep.rs`.
- **Résultats** (arbre final, fusion de `wave4/doc` comprise) :
  `plumber_objects` sur `all` (20 premières pages) 1 852/1 853 (le
  fichier aux boîtes en double, ci-dessus), `plumber_text` et
  `plumber_tables` 1 853/1 853 ; les quatre étapes `plumber_*` identiques
  sur `adv-plumber` (131 PDF) ; `plumber_unit` (1 236 traces d'étapes, 165
  ouvertures, `utils`), `plumber_sweep` (396 balayages) ; `scripts/check.sh`
  complet (natif, `wasm32-wasip1`, porte de profondeur, paquet JS).
- **Mutations** (`--in-diff` de `762bbe5..HEAD`, `--profile mutants -j 1
  --timeout 60`, `heavy.sh`, rejeux `plumber_unit` et `plumber_sweep`
  avec `PLUMBER_FAIL_FAST`, les fonctions de DOC exclues) : passe
  ordinaire 1 218 mutants (`page.rs` 363, `pdf.rs`/`convert.rs`/
  `pdfinternals.rs`/`obj.rs`/`table.rs` 255, `container.rs` 212, `pycompat`
  et `pdfminer` touchés 131, étapes 257) ; leurs survivants ont fait
  écrire les balayages de profondeur (et trouver les cadres non comptés,
  ci-dessus), les cas de `font-kinds` (le cas levait avant tout caractère :
  `fix_fontname_bytes` n'était pas atteint), d'une image au flux vide, des
  `/ID` de structure en tableaux de nombres, des mots de passe faux non
  `str`, des formes `unicode_norm` à surrogates sur des caractères au texte
  vide, de `decode_psl_list`, des identités de caches avant et après
  `flush_cache`/`close`, des `getattr` de `to_dict`/`to_csv` ; ils ont fait
  retirer du code mort (`Page::is_original`, `Page::process_object`,
  `Objects::len`/`is_empty`, les bras propres au `PDF` de son `getattr`,
  que pdfplumber ne prend jamais, la sortie anticipée de
  `PyStr::replace`). Après les passes `--iterate` : `page.rs` 5
  survivants, le reste de la bibliothèque 9, `pycompat` 3, étapes 25 ;
  les mutants de `decode_text_value` et `resolve_and_decode_obj` (code de
  DOC) sont tués par `structure_unit` (passe croisée sur eux et le fichier
  fermé d'`io.rs` : 32 mutants, 15 tués, 2 délais, 14 non compilables, 1
  survivant, `seek_closed_error`, ci-dessous) ; passe
  `Ok(..)` (`scripts/mutants-ok.sh`) 170 mutants, 132 tués, puis 53 sur
  les 70 repassés après les cas, puis `Page::close` (un balayage qui
  appelle `page.close()`) : 2 survivants ; mutants à la main des
  cadres (`py_frame`, `leaf`, `eval_entry`, `class_call`, `c_call`, les
  `Subscription::eval` retirés, 198) : 55 tués.

  Survivants, tous équivalents ou inatteignables :
  - **unités C seules** (36 mutants à la main : `eval_entry` des appels
    depuis le C, `class_call`, `c_call` de `PdfminerException`) : les
    unités C en cours restent loin de la limite (10 000) tant que les
    cadres Python, eux, l'atteignent avant (`resolve_all`, `Serializer` :
    deux ou trois cadres par niveau) ;
  - **cadres dominés** (107 mutants à la main, `iter_level` : la création
    du générateur imbriqué et sa dernière reprise) : un cadre que chaque
    balayage atteint après un chemin plus profond du même appel
    (l'interprétation sous une opération sur une page neuve, `Serializer`
    avant l'encodeur JSON, `Page.__init__` après `create_pages`, la
    création d'un générateur imbriqué que sa dernière reprise égale) ;
  - **valeurs que le port ne produit pas** : un `dict` Python dans les
    attributs d'un objet du layout (`resolve_all_value`, 3 : ce sont des
    objets PDF), une ligne de `to_csv` qui serait un `dict` PDF, la
    comparaison de deux `Page` (`PageRef::ptr_eq`, `py_eq`), un `str`
    d'objet PDF imbriqué sous `str()` (`render_counted`, 2), le `getattr`
    d'un `PDF` (jamais pris) ;
  - **écarts documentés** (`NotPorted`) : un `indent` au-delà de 2**20,
    le `getattr` d'une méthode ;
  - **équivalents** : `|` et `^` sur des bits disjoints (`b64encode`),
    `seek_closed_error` (la lecture d'un fichier fermé vérifie aussi :
    seule une position au-delà de 2**63, après `close`, distinguerait
    l'ordre des deux erreurs), l'implémentation de `Debug` de `PageRef` ;
  - **étapes** (harnais) : conditions redondantes (`Names` : un nom
    d'image en ligne et un flux sans identifiant ; `annot_value` : les clés
    `name`/`P` absentes du premier niveau), `select` (`% n` puis `% n`),
    bornes du lecteur des `spans` sur un texte bien formé, le rang de
    secours de `ContentIds` (jamais pris quand la trace concorde).

### Chantier DOC

`structure.py`, les métadonnées (`PDF.__init__`, `strict_metadata`),
`Page.annots`/`hyperlinks` et ceux du `PDF`, `structure_tree` ; l'étape
Rust `plumber_doc` ; le corpus adverse `oracle/adversarial/structure.py`
→ `adv-structure` (vérifié par `plumber_doc` et `plumber_objects`) :
arbres de structure avec et sans `/ParentTree`, `/K` entier, dict, tableau,
références, MCR et OBJR, `/Pg` absent ou hors des pages choisies, DAG,
`/RoleMap`, `/ClassMap`, attributs `/A`/`/C` en tableaux avec révisions,
`/ID`, `/T`, `/Lang`, `/Alt`, `/ActualText` de tout type (dont une liste
d'entiers > 255 suivie d'une liste imbriquée : le `str(s)` de
`decode_text`, une unité C de plus à sa première exécution),
références dans les attributs (le `deepcopy` d'`asdict`), arbres profonds
autour des premières profondeurs qui échouent (`prune`, `asdict`,
`repr`) ; métadonnées imbriquées, profondes (`resolve_and_decode`), en
UTF-16 et PDFDocEncoding, `/Info` multiples ou partagés ; annotations aux
`/Rect` atypiques, `/A` et `/URI`, `/T`, `/Contents` non décodables (avec
et sans `raise_unicode_errors`), sur pages tournées.

#### Réalisation (branche `wave4/doc`)

- **`structure.rs`** : `structure.py` fonction par fonction.
  `PDFStructTree::new(doc, Scope)` (l'appel de classe, `__init__`, la
  portée page par l'arbre des parents ou, sans `/ParentTree`, par l'arbre
  entier), `_make_element`, `_make_attributes`, `_parse_parent_tree`,
  `_parse_struct_tree` et son `prune` récursif, `_resolve_children`,
  `on_parsed_page`, `element_bbox` ; `PDFStructElement` (`all_mcids`,
  `to_dict`, `repr`). Les objets du document sont lus avec les opérateurs
  de Python quel que soit leur type (`py_in`, `py_getitem`, `py_get`,
  `py_iter`, les attributs `.name`, `.objid`, `.items()`) : un arbre de
  structure est un graphe d'objets quelconque, et chaque accès lève là où
  Python lève (`TypeError`, `AttributeError`, `KeyError`, `PDFKeyError`
  d'un flux, `AssertionError` des `assert`, `StopIteration` du `next()`
  de l'arbre des parents). Les tables `seen` sont indexées par
  `repr(ref)` (le `repr` intégré : une unité C, plus celles de l'objet).
- **Écarts d'interface** (contrat de la base amendé) : `find_all` rend un
  itérateur paresseux (`FindAll`, le générateur de Python : `find`
  s'arrête au premier élément, une exception du filtre arrive quand le
  parcours l'atteint, un DAG profond n'est pas parcouru en entier pour
  rien) au lieu d'un `Vec` ; `PDFStructElement::to_dict(file)` prend la
  classe du fichier du document (`pdfparser::FileKind`,
  `PDFDocument::file_kind`), dont dépend la copie profonde d'`asdict` ;
  `PDFStructTree::page_dict` est une table de hachage `ObjId -> int`.
- **`to_dict`** : `dataclasses.asdict` (les cadres de `_asdict_inner`, de
  `_is_dataclass_instance`, de `fields` et des expressions génératrices
  que `list()`/`dict()` reprennent depuis le C : trois cadres et deux
  unités C par niveau d'éléments, deux cadres par niveau d'un attribut
  imbriqué), puis l'élagage (itératif chez Python, récursif ici sur une
  profondeur qu'`asdict` a déjà bornée). Les feuilles passent par
  `copy.deepcopy`, dont le port suit le parcours et les cadres
  (`copy_frames`) : un nom (`PSLiteral`, `PSKeyword`) six cadres, une
  référence copie le document jusqu'à l'objet fichier,
  `TypeError("cannot pickle 'BufferedReader' instances")` pour un
  fichier ouvert par un chemin, après 22 cadres (le plus profond est dans
  les sections xref du document, avant `_parser` dans son `__dict__`), un
  flux (qu'y laisse le décodage des métadonnées en place) ses attributs,
  avec leurs références, et, pour un document chiffré, le gestionnaire de
  sécurité dont `decipher` est une méthode (14 cadres pour RC4 ; mesuré
  par `python -m oracle.depth_probe structure`). Non modélisés : un
  document dont `xrefs`, `info` ou `catalog` tiennent des objets directs
  imbriqués plus profond (la copie y descend plus bas avant l'erreur), les
  `/CF` des gestionnaires AES, la mémoire de `copy` (un objet revu n'est
  pas reparcouru), et la copie complète du document lu depuis des octets
  (JavaScript : le port garde la référence, une valeur égale, sans
  compter ses cadres).
- **`repr`** : le `__repr__` généré de la dataclass derrière
  `dataclasses._recursive_repr` (le wrapper appelé depuis le C, deux
  cadres et quatre unités C par niveau), les champs par `repr_counted`.
- **Boucle infinie de Python** : un cycle parmi les éléments qu'atteint
  l'arbre des parents d'une page fait boucler `_resolve_children` sans
  fin (chaque tour allonge les listes : Python finit à court de mémoire).
  Le port relève, à la première visite de chaque élément, les éléments
  qu'il met en file ; quand tout élément atteint a été traité une fois
  (son traitement ne dépend de rien qui change, il ne lèverait plus rien)
  et qu'un cycle est atteignable, il lève `MemoryError`. Sans oracle
  possible (Python ne rend pas la main), écarté du corpus adverse ; les
  cycles de la portée document, eux, finissent en `RecursionError` dans
  `prune`, comme en Python (`cycle-*` d'`adv-structure`).
- **Métadonnées en place** : `resolve_and_decode` décode un `dict` **dans
  le dict du document** (`obj[k] = ...`) et la suite le voit : le corpus
  a des `/Info` qui désignent le catalogue (un fichier oss-fuzz de
  pdfplumber), un dict d'annotation (`bug1978317.pdf`), des listes et
  dicts imbriqués. `PDFObject` gagne la variante `Str(PyStr)` (une `str`
  Python, que seul ce décodage produit) ; `repr`, `str`, `==`, le hachage
  des `ObjId`, la vérité, `in`, `x[0]`, `canon`, les valeurs typées et la
  frontière Wasm la traitent comme Python traite une `str`.
  `read_metadata` (`pdf.rs`) : les `/Info` fusionnés dans un nouveau
  dict, chaque valeur décodée (`resolve_and_decode_obj`), l'exception
  levée avec `strict_metadata`, sinon journalisée (logger
  `pdfplumber.pdf`, canal des diagnostics) et la valeur laissée telle
  quelle (après ce qu'elle a déjà modifié en place).
- **Annotations** (`page.rs`) : `Page.annots` (`resolve_all` des
  `/Annots`, `list(map(parse, ...))`, n'importe quel itérable, chaque
  `parse` depuis le C), `rotate_point` (`r // 90` : `TypeError` pour une
  rotation flottante ou `bytes`), `parse` (`/Rect` dépaqueté comme Python,
  `_normalize_box`/`_invert_box` d'un 4-uplet de valeurs, `/URI`, `/T`,
  `/Contents` décodés par `bytes.decode("utf-8")` puis `("utf-16")` :
  `pycompat::codecs`, les décodeurs stricts de CPython et leur
  `UnicodeDecodeError` exact ; `raise_unicode_errors=False` garde les
  octets et émet l'avertissement sur le canal des diagnostics ; `/P`
  remplacé par la page, un flux qui a `/P` lève le `TypeError`
  d'affectation), `CroppedPage` par `apply_crop_fn` de PAGE ;
  `Page.hyperlinks`, `Page.structure_tree`, `PDF.structure_tree`.
  Non modélisés : les cadres de `warnings.warn` (10 au premier message,
  6 à un autre, aucun quand le registre l'a déjà vu ; `Page.annots`
  n'est jamais appelé près de la limite).
- **`decode_text`** : le `"".join` des générateurs tient ses unités C
  jusqu'à son retour ou son exception, avant le `str(s)` du `except
  IndexError` (la base les gardait pendant `str(s)` : deux unités de
  trop, vues par les fenêtres `actualtext-str-*`).
- **Étape `plumber_doc`** (`pdftrace`) : le parcours de
  `oracle/stages/plumber_doc.py`, les cadres des générateurs délégués
  (`yield from` : `SEND` exécute un générateur en ligne, sans unité C)
  et des lambdas ; le premier `LOAD_ATTR` d'une propriété à un site
  (`pdf.structure_tree`, `page.structure_tree` dans les lambdas de
  l'étape) appelle le getter depuis le C, deux unités C
  (`pycompat::specialize::CallSite::property_load`, l'instruction se
  spécialise ensuite en `LOAD_ATTR_PROPERTY`). `StageInput` gagne
  `password_given` : l'étape Python reçoit `None` sans mot de passe, et
  ses lignes `password` diffèrent pour un mot de passe vide donné (cinq
  fichiers du corpus).
- **Corpus `adv-structure`** (`oracle/adversarial/structure.py`, 441
  PDF) : les cas de la liste ci-dessus, et 16 fenêtres de profondeur
  autour du premier niveau qui échoue en Python, cherché par dichotomie
  sur l'étape (`--probe`) : `to_dict` d'une chaîne d'éléments (330 ;
  329 avec une révision littérale à la feuille, 323 avec une référence
  dans ses attributs, 327 avec un attribut imbriqué ; 330 sans arbre des
  parents), `prune` (986), `repr` (497), un attribut imbriqué (492), les
  unités C du `repr(ref)` d'une liste imbriquée dans `/K` (9 990), du
  `str(s)` de `decode_text` à sa première exécution (9 989) et aux
  suivantes (9 990), sur la seconde page (9 992 : la propriété `page.structure_tree` de la lambda de l'étape y est chargée par l'instruction spécialisée), du `repr` d'un attribut (9 991), `resolve_and_decode`
  de dicts et de listes imbriqués (987), l'arbre des nombres des parents
  (978). Rejoué par `oracle/unit/structure.py` (`structure_unit.rs`),
  les fenêtres aussi depuis un et deux cadres plus bas.
- **Fichier fermé** (laissé ouvert par la base) : `PDF.close` ferme le
  fichier ouvert par un chemin (`PDFDocument::close_file`,
  `pycompat::io::BytesIO::close`), et la lecture suivante d'un objet
  qui n'est pas en cache lève le `ValueError` d'un `BufferedReader` fermé
  (`seek of closed file`, `read of closed file`), que `PDF.pages` enveloppe
  dans `PdfminerException` : la seule différence de `plumber_doc` sur
  `all` (`issue11549_reduced.pdf`, un objet illisible relu après
  `close()`), et le cas `closed-file-reread` d'`adv-structure`. Un
  document lu depuis des octets est le flux de l'appelant
  (`stream_is_external`) : `close()` ne le ferme pas.
- **Résultats** : `plumber_doc` sur `all` (20 premières pages), 1 853
  fichiers identiques (le dernier après la fermeture du fichier) ;
  `plumber_doc` et `plumber_objects` sur `adv-structure`, 441 identiques
  chacune ; `structure_unit` (676 traces, 5 148 cas de `codecs`, et les tests Rust du garde des cycles).
- **Mutations** (`--profile mutants -j 1 --timeout 60 -- --package=pdftrace
  --test=structure_unit`, puis `scripts/mutants-ok.sh` et des mutants à
  la main pour les corps sans `Default`) : `structure.rs` (261 mutants,
  dont 197 « unviable » de `PyResult`), `codecs.rs` (66), les fonctions
  DOC de `page.rs` avec `_normalize_box` (57), `read_metadata` et
  `PDF::structure_tree` (19), `resolve_and_decode*`/`decode_text_value`
  (30), le diff de `pdftypes.rs` (`Str`, 29), `property_load`,
  `file_kind`, `close_file` ; passe `Ok(..)` (73 mutants) ; 22 mutants à
  la main (chaque corps remplacé par une valeur concrète : tous tués, celui
  de `property_load` par la fenêtre `actualtext-str-page2`).
  Les survivants ont donné des cas (`adv-structure` : flux parmi les
  attributs, avec ou sans chiffrement, référence dans leurs listes ;
  `/Annots` décodé en `str` ; pages tournées non carrées ; cycle puis
  erreur) et des tests Rust (garde des cycles, `find` d'un élément) ;
  restent, justifiés : `(ch & 0x3FF) << 10 | (ch2 & 0x3FF)` en `^`
  (bits disjoints) dans `decode_utf16` ; `checking && !edges...` en `||`
  dans `_resolve_children` (un élément retraité avant la décision
  réinscrit les mêmes arêtes : même décision) ; dans `pdftypes.rs`, les
  bras `Str` de `==` entre deux `PDFObject` et de l'égalité et du hachage
  des `ObjId` (aucun chemin ne compare deux `str` décodées ni ne fait d'une
  `str` un identifiant d'objet : les identifiants viennent du parseur et
  des références) ; `PDFStructElement.__iter__` remplacé par le champ
  `children` (rien ne l'appelait).

### Plan de comptage des cadres

Chaque fonction ou propriété portée prend son cadre ; les appels faits
depuis le C (`map`, `filter`, `dict(...)`, `list(genexpr)`,
`"".join(genexpr)`, `sorted(key=...)`) leurs unités (`eval_entry`) ; les
appels de classe `class_call` (`PDF`, `Page`, `CroppedPage`,
`FilteredPage`, `LAParams`, `PDFResourceManager`, `PDFStructTree`,
`PDFStructElement`, `Serializer`) ; les générateurs leur cadre à chaque
reprise (`iter_pages`, `iter_layout_objects` à chaque niveau de `yield
from`, `_find_all`, `all_mcids`) ; la bibliothèque standard par des
constantes mesurées avec `oracle/depth_probe.py` (`functools.lru_cache`
et `update_wrapper` de `Page.__init__`, `re.sub` de `process_object`,
`warnings.warn`, `dataclasses.asdict`, `copy.deepcopy`,
`reprlib.recursive_repr`, `json.dumps`, `csv`) ; le `str(s)` de
`decode_text` est un site de `pycompat::specialize` (à déclarer dans
`oracle/depth.py`, `_call_sites` : premier commit de PAGE, ci-dessus) ;
chaque chargement d'une propriété est un site de `LOAD_ATTR`
(`pycompat::specialize::AttrSite`, jonction ci-dessous : deux unités C
pendant le getter tant que l'instruction n'est pas spécialisée pour la
classe de l'objet). Le code de la vague 1 (`utils/text.rs`, `table.rs`, géométrie,
regroupement) ne compte pas de cadres : il ne récurse pas et pdfplumber
l'appelle au niveau de l'utilisateur, sauf sur le chemin d'une propriété de
la page qui l'interprète (`TableFinder.__init__` et `get_edges` vers
`page.edges` et `page.extract_words`, `Table.extract` vers `page.chars`,
réalisation de PAGE ci-dessus) ; restent les unités C des
comparaisons, `hash` et `repr` de valeurs imbriquées (`obj.rs`), à compter
avec leurs cas de `adv-plumber`. Les constructeurs des vagues 0 à 2
gardent deux unités par appel de classe (sans effet observable).

### Outillage

`oracle.verify` passe toujours `--cmap-dir` à `pdftrace` (par défaut le
répertoire de `oracle/codegen/cmaps.py` sous le cache, écrit au besoin ;
un `--cmap-dir` sans `names.txt` arrête tout, code 2, avant le cache de
l'oracle) : une étape qui interprète des pages ne peut plus compter des
écarts « no CMap provider installed » faute d'avoir exporté
`PDFPLUMBER_CMAP_DIR` (`oracle/tests/test_verify.py`).

### Jonction

`wave4/page` puis la fusion de `wave4/doc`. Ensuite :

- **Wasm** : `DocState` (`crates/wasm/src/ops.rs`) tient le `PDF` de
  pdfplumber (`pdfplumber::pdf::PDF`, avec son `PDFResourceManager` et ses
  pages) au lieu des seules CMaps ; `Document::reset` le jette quand un
  appel est nul (une CMap prédéfinie manquante) et la relance rouvre le
  document depuis ses octets (`Source::Stream`, `FileKind::BytesIO`) avec
  l'état du processus remis (`pdfminer::process_state`). Des opérations
  qui interprètent des pages : `pages` (géométrie), `page_objects`,
  `extract_text`/`words`/`text_lines`/`search` (deux temps : le texte de
  la carte, puis les correspondances de la `RegExp`), `tables`, pages
  dérivées (handles), `metadata`, `annots`, `structure_tree`, `to_json`.
  Le test CJK `documents.jsonl` marqué *todo* passe par `pdf.pages`.
- **Vérification** : `oracle.verify` des quatre étapes sur `all`
  (`--timeout 600`), `adv-plumber`, `adv-structure`, et des étapes des
  vagues précédentes sur leurs corpus (les caches de l'oracle sont
  reconstruits si `oracle/depth.py` a changé) ; `oracle.wasm_parity
  --set quick --stages plumber_objects,plumber_text,plumber_tables,plumber_doc`
  (`--oracle-dir` pour les étapes `ORACLE_INPUT`) ;
  mutations de tout le code de la vague (passe ordinaire, `Ok(..)`, cadres
  à la main) ; `scripts/check.sh` natif et `wasm32-wasip1`, construction
  `dist`, porte de profondeur étendue aux fenêtres de `adv-plumber` et
  `adv-structure`.

#### Réalisation de la jonction (branche `wave4/join`)

- **Fusion** : `wave4/page` contenait déjà `wave4/doc` jusqu'à son dernier
  commit (`fa24d87`) : la fusion est vide, sans conflit.
- **Chargements de propriétés** (`pycompat::specialize::AttrSite`) :
  `oracle.verify` des quatre étapes `plumber_*` sur `adv-interp` a montré
  que ses fenêtres à la limite des unités C (`climit-*`, 40 à 48 fichiers
  par étape) différaient : Python lève `RecursionError` en interprétant
  sous `page.objects` → `parse_objects` → `self.layout`, le port non. Un
  `LOAD_ATTR` d'une propriété, à sa première exécution, trouve la propriété
  par `PyObject_GetAttr`, dont `property.__get__` appelle le getter depuis
  le C : une entrée de la boucle d'évaluation, deux unités tenues pendant
  tout le getter (l'interprétation de la page comprise) ; la seconde
  exécution spécialise l'instruction (`LOAD_ATTR_PROPERTY`, le cadre du
  getter poussé en ligne, sans unité). `AttrSite` suit le compteur
  adaptatif de CPython 3.12 (`Include/internal/pycore_code.h` ;
  `Python/bytecodes.c`, `LOAD_ATTR`, `LOAD_ATTR_PROPERTY` ;
  `Python/specialize.c`, `_Py_Specialize_LoadAttr`) : réchauffement 1, la
  classe pour laquelle l'instruction est spécialisée (son `tp_version_tag`
  : un `Container.chars` lu sur une `Page` puis sur une `CroppedPage`
  désoptimise), 52 échecs avant de se spécialiser de nouveau, le recul
  exponentiel d'une spécialisation qui échoue (un attribut de classe,
  `Page.pages = None`, lu par une instance). Chaque chargement d'une
  propriété du code de pdfplumber porté est un site (`page.py` 22,
  `container.py` 19, `pdf.py` 10, `structure.py` 3, `table.py` 6) ; le
  `getattr(self, t + "s")` de `to_dict`/`to_csv` prend l'unité de l'appel
  générique du builtin (première exécution, `CallSite::builtin_call`) et,
  pour une propriété, l'entrée du getter, toujours (`getattr` n'est pas un
  `LOAD_ATTR`). L'état des sites est un état du processus : remis à zéro
  par `py_process::start`, saisi et remis par `pdfminer::process_state`.
  Les étapes `plumber_*` de `pdftrace` modélisent leurs propres chargements
  de propriétés dont le getter peut interpréter (`pdf.pages`,
  `page.objects`, `page.chars`, `page.edges`, `page.annots`,
  `page.layout`...) et les appels de méthodes liées avec `**kwargs`
  (`CALL_FUNCTION_EX` appelle la méthode par `PyObject_Call` : deux unités),
  sous lesquels l'interprétation recommence quand les objets d'une page ont
  levé ; les chargements de getters bon marché (`width`, `height` de
  `boxes`, `Table.bbox`/`rows`/`columns`) et les `CALL` de méthodes liées
  sous lesquels rien de profond ne tourne sont laissés de côté (notés dans
  les modules).
- **Mot de passe** : `pdfminer` ne lit le mot de passe que pour un document
  chiffré (`PDFDocument::open_with`) : un mot de passe d'un type que le port
  ne modélise pas (`bytes`) ouvre un document non chiffré, comme Python
  (`open.jsonl` de `js_runtime.py` en a un), et lève `NotPorted` (non
  enveloppé) pour un document chiffré.
- **Frontière Wasm** (`js-runtime.md`, « Documents et pages ») : le `PDF`
  dans `Document::state`, les opérations des documents, pages, pages
  dérivées et tables (`crates/wasm/src/pages.rs`), chacune l'instruction
  d'un script Python au niveau du module (`ENTRY_DEPTH` : un cadre, deux
  unités C ; une propriété à son site, une méthode avec `*args,
  **kwargs`, `getattr` d'un nom), `oracle/unit/js_pages_run.py` ; un appel
  nul remet les documents qu'il a atteints dans leur état d'avant lui en
  rejouant leur historique. `Page::search_with` (un `main_group`
  quelconque) et `Page::search_text` (la première moitié de `search`) pour
  la recherche en deux appels ; `getattr(pdf, "pages")` donne les pages.
- **Résultats** (`oracle.verify`, `heavy.sh`, `--timeout 600`, fournisseur
  de CMaps passé) :
  - `all` (1 853 fichiers, 20 premières pages) : `plumber_objects`
    1 852/1 853 (`pdfjs-issue19624-text.pdf`, la limite connue de la clé
    de contenu des `id()`, ci-dessus), `plumber_text`, `plumber_tables`,
    `plumber_doc` 1 853/1 853 ;
  - `adv-plumber` (182 à la jonction, dont 51 fenêtres `climit-*` ajoutées
    par elle ; 199 et 68 fenêtres depuis `wave4/fix`, qui ajoute les 17
    `tf-list-tagged-nopg`, ci-dessous), `adv-structure` (441), `adv-pages` (285),
    `adv-objects` (333) : les quatre étapes identiques ;
  - `adv-interp` (353) : `plumber_objects` et `plumber_doc` 353/353,
    `plumber_text` 336/353 et `plumber_tables` 341/353 : les fichiers
    `op-*` et `text-*` aux coordonnées NaN, dont `cluster_objects` range
    les NaN dans un `set` par leur adresse (écart documenté,
    `public-api.md`) ; Python lui-même change cet ordre d'un processus à
    l'autre (trois traces de `002-op-cm-0.pdf` relancées : un ordre, la
    trace du cache : un autre) ; et `096-text-TL.pdf` dont l'oracle
    `plumber_text` est tué par la limite de 5 Go de `heavy.sh` ;
  - régression sur `quick` : `psparser`, `objects`, `streams`, `pages`,
    `interp`, `fonts`, `layout` 150/150.
- **Temps** (`quick`, 150 fichiers, 302 pages, 20 premières pages ;
  Python mesuré dans le processus après les imports, `pdftrace` par
  fichier, machine partagée) : `plumber_objects` 574 ms par page pour
  Python, 152 ms pour `pdftrace` (×3,8) ; `plumber_text` 681 / 81 ms
  (×8,4) ; `plumber_tables` 601 / 117 ms (×5,2) ; `plumber_doc` 765 /
  217 ms (×3,5). Les traces dominent le temps de `pdftrace` (écriture
  canonique de chaque objet). Sans trace (`crates/pdftrace/examples/
  plumber_phases.rs` et son jumeau Python, mêmes fichiers) : ouverture
  4,5 / 0,32 ms par page (×14), `page.layout` (interprétation et
  périphérique) 73 / 5,7 ms (×13), `page.objects` 41 / 12,3 ms (×3,4),
  `extract_text` 6,6 / 2,1 ms (×3,2), `extract_words` 4,7 / 1,6 ms (×3,1),
  `find_tables` 6,0 / 1,8 ms (×3,3) ; en tout 137 / 24 ms (×5,7). Point
  chaud : `page.objects` (`process_object` de chaque objet du layout :
  `resolve_all`, dicts d'attributs, conversions de valeurs), la moitié du
  temps du port hors traces, deux fois l'interprétation ; puis le code de
  la vague 1 (texte, mots, tables), trois fois plus rapide que Python
  seulement. Fichier lent relativement : `issue12841_reduced.pdf` (0,2 s
  contre 0,07 s pour `plumber_objects`).
- **Fenêtres `climit-*` d'`adv-plumber`** : les fenêtres des unités C
  d'`adv-interp` sont dichotomisées sur les étapes de l'interpréteur ; les
  étapes `plumber_*` y échouent toutes au même endroit. `adv-plumber` a les
  siennes (un opérande de `Tf` imbriqué avant la scène, sur une page, deux,
  et une page balisée pour `element_bbox`), de la première profondeur qui
  lève dans une étape `plumber_*` moins deux jusqu'à la limite des unités
  C (9 984 à 10 000) : chaque opération qui interprète la page y rencontre
  la limite à sa profondeur propre. `adv-plumber` : 182 fichiers à la
  jonction (199 depuis `wave4/fix`), les quatre étapes identiques ; `wasm-depth-gate.sh` les passe aux quatre
  étapes.
- **Mutations** (code changé par la jonction depuis `83ff2d3`) :
  - `cargo mutants`, frontière (`crates/wasm`, `ops.rs`, `pages.rs`,
    `handles.rs`) : 112 mutants, 86 détectés, 5 non viables, 21 manqués ;
    après les tests ajoutés, la passe ciblée : 44 mutants, 33 détectés,
    4 non viables, 7 manqués, dont `run_op` (`&&` → `||`, la condition
    retirée depuis : `run_op` enregistre toute opération qui atteint un
    document). Restent, équivalents : `|` → `^` dans `Handles::iter`/
    `iter_mut` (bits disjoints de l'étiquette et de l'index), `Object::reset`
    → `()` (le rejeu remplit chaque objet de nouveau), les gardes
    `p.freed`/`t.freed` de `Core::void` → `false` (code retiré depuis :
    les handles libérés sont libérés), `calls +=` → `-=` (identifiants distincts aussi en
    décroissant, arithmétique enveloppante) ;
  - `cargo mutants`, bibliothèque (`page.rs`, `pdf.rs`, `obj.rs`,
    `table.rs`, `container.rs`) : 48 mutants, 29 détectés, 2 non viables,
    17 manqués ; passe ciblée : 22 mutants, 8 détectés, 14 manqués ; les
    fenêtres `class-*` refaites (dernière opération sur un second
    document) tuent les quatre constantes de `Page` (`class_name`,
    `property_class`). `is_property` réduit aux noms que `to_dict` et
    `to_csv` construisent (`doc_attr`/`page_attr` : `NotPorted` pour un
    nom qui ne finit pas par `s`, `width`, `bbox`, `layout`..., où Python
    rend l'attribut) : 6 mutants, 2 détectés, 4 manqués. Restent,
    équivalents : `is_property` → `true` (deux unités de plus pendant un
    getter qui ne descend pas : un attribut absent, `_objects`,
    `cached_properties`, `pages` d'une page), le bras `pages` supprimé ou
    sa comparaison inversée (`PDF.pages` ne contient aucune récursion C),
    `==` → `!=` dans `to_csv` (la branche de l'autre classe calcule les
    mêmes pages, ses unités tenues pendant `pages`),
    `<impl Container for PDF>::class_name` (`""`, `"xyzzy"` : un `PDF` ne
    partage de site qu'avec des pages, dont la classe diffère dans les
    deux cas ; la seule autre lecture, `is_property(_, "pages")`, tient
    deux unités pendant `PDF.pages`, qui ne contient aucune récursion C),
    `TablePage::property_class` par défaut (`Some("")`, `Some("xyzzy")` :
    la page d'objets de la frontière et des rejeux a `chars` et `edges`
    en attributs simples, dont le « getter » ne descend pas) ;
  - passe `Ok(..)` (`scripts/mutants-ok.sh`) : 135 mutants, 115 détectés,
    13 non viables, 7 manqués, tous dans du code de `ops.rs`/`pages.rs`
    réécrit depuis et repassé par la passe ciblée ; étapes (`pdftrace`) :
    10 mutants, 10 détectés ;
  - sites d'unités C (`scripts/unit-site-mutants.py`, que cargo-mutants
    ne mute pas, 138 sites) : 63 détectés, 75 survivants, chacun
    justifié : getters bon marché sous le site (`width`, `height`, `bbox`
    de `rotate_point`, `point2coord`, `process_object`, `_get_textmap`,
    `to_dict`, la géométrie des pages de `doc_pages`, le tri de `sorter`, `Table.rows`) ; getters
    en cache sous le site (`parse_objects` avant la racine, `layout` après
    `objects_lines`, `hyperlinks` après `annots`, `TableFinder` après
    `find_tables`, `dedupe_chars` après les objets) ; sites dominés, dont
    un voisin du même chemin a déjà calculé ou levé (`edges` lit `lines`
    avant `rect_edges`/`curve_edges`, `PDF.objects` lit `p.objects.keys()`
    avant `p.objects[kind]`, `TableFinder.get_edges` lit les arêtes
    verticales avant les horizontales) ; `PDF.pages`, `annots`,
    `hyperlinks`, `close`, `structure_tree`, qui n'interprètent aucune
    page et ne contiennent aucune récursion C (les pages, `to_csv`,
    `to_dict`, l'arbre de structure, les annotations du pilote des
    étapes) ; opérations des étapes après la première qui a interprété la
    page (`plumber_text`, `plumber_tables` s'arrêtent à la première qui
    lève ; sinon les objets sont en cache) ; la seconde moitié de la
    recherche (carte de texte en cache), `crop` (ne lit que `bbox`),
    `close`, l'entrée de `pdfplumber.open` (aucune récursion C à
    l'ouverture avant la limite des cadres Python). (La justification
    donnée ici pour `ELEMENT_BBOX_PAGE_OBJECTS` était fausse : l'arbre de
    page de `plumber_doc` l'atteint quand les éléments n'ont pas de
    `/Pg` ; tué depuis, « Correctifs de la revue ».)

#### Correctifs de la revue (branche `wave4/fix`)

Revue indépendante de la jonction (page, document, profondeur et erreurs,
tests/performances/Wasm), chaque constat revérifié :

- **Rejeu d'un appel nul** (`crates/wasm/src/ops.rs`) : le rejeu d'un
  document repartait de l'état du processus d'avant l'appel nul ; une
  entrée qui avait levé `RecursionError` (un nom interné, un site
  spécialisé, une CMap en cache plus tard) réussissait au rejeu et
  laissait des objets ou des pages en cache que Python n'a jamais, ou
  atteignait une police dont la CMap manquait, que le
  `debug_assert!` de la version `release` laissait passer (texte vide sans
  erreur). Chaque `Entry` garde l'état de départ de sa première exécution
  (`ProcessState` : les symboles internés deviennent une chaîne
  persistante, `psparser::Interned`, qu'un instantané partage pour le
  prix d'un compteur de références, et le reste est partagé avec
  l'instantané précédent s'il n'a pas changé) et sa fin (l'exception ou
  rien) ; le rejeu repart de cet état et doit finir de même, objets créés
  et CMaps comprises, sinon les documents deviennent inutilisables
  (`RuntimeError` à toute opération qui les atteint). Fenêtres
  `depth-replay-{depth,cmap,pages}-*` de `js_runtime.py` (huit chacune).
- **Handles libérés** : les pierres tombales restaient dans la table
  (`MemoryError("too many handles")` après 2^20 − 1) ; l'historique nomme
  maintenant les objets par leur identité (`Target::Object`) et un rejeu
  refait hors de la table un objet libéré. L'historique, lui, ne
  raccourcit qu'à la libération du document (documenté, `js-runtime.md`).
- **Chaîne de pages dérivées** : `Drop for Page` défait la chaîne des
  parents en boucle (la « trashcan » de CPython) : 10^5 rognages
  successifs se libèrent sur une pile de 256 Kio (débordement avant, et
  dès 12 000 sous Wasm).
- **Liaison des arguments** (`pycompat::call`) : un mot-clé qui nomme un
  paramètre déjà lié (`self`, `pattern`, `strip`, `return_chars`, `bbox`,
  `cls`, `path_or_fp`) lève le `TypeError` de CPython avant l'appel (dans
  `Page`, `Table.extract`, `TableSettings`, `LAParams`, `PDF.open` et les
  instructions de la frontière) ; un mot-clé inconnu s'écrit `'%S'`, pas
  `repr` ; un `**` qui n'est pas un dict lève « argument after ** must be
  a mapping » ; `TableSettings.resolve` de n'importe quelle valeur
  (`ValueError("Cannot resolve settings: …")`, son `str()` compté). Les
  options de `to_dict`/`to_json`/`to_csv` sont des valeurs Python
  (`object_types` itéré, `t + "s"`, `CSV_COLS_REQUIRED + include_attrs`,
  `set(exclude_attrs)`), `stream=None` accepté.
- **`page_filter`** : la frontière calcule les objets du parent comme
  `mask_of` (un cadre, un site de chargement à lui) : fenêtre
  `climit-tf-list-filter`.
- **Valeurs `Page`** : une sorte de valeur de la frontière (PJB1 `0x12`,
  JSON `"page"`, `PageRef` en JS), le handle de la page.
- **Avertissements** : `pycompat::warnings` (registre des filtres par
  défaut, état du processus) ; le `logger.warning` de `PDFStructTree`
  atteint les diagnostics ; `diagnostics.jsonl`.
- **`ELEMENT_BBOX_PAGE_OBJECTS`** : sorte `tf-list-tagged-nopg` des
  fenêtres `climit-*` d'`adv-plumber` (éléments sans `/Pg`, 17 fenêtres) ;
  le mutant manuel du site y diffère (2 fenêtres sur 34 balisées).
- **API JS** (types, `public-api.md`) : `pdf.pages` et `pdf.metadata`
  sont des promesses (pages paresseuses), les pages dérivées aussi.
- **Vérification** : `plumber_*` identiques sur `adv-plumber` (199),
  `adv-structure` (441), `adv-pages` (285), `adv-objects` (333) et sur
  `all` (1 853, sauf `pdfjs-issue19624-text.pdf` pour `plumber_objects`,
  limite connue du rang des `id()`) ; sur `adv-interp`, `plumber_objects`
  et `plumber_doc` identiques, `plumber_text` (16) et `plumber_tables`
  (12) diffèrent par l'ordre de coordonnées NaN dans `cluster_objects`
  (écart documenté, `public-api.md`) ; étapes `psparser` à
  `interp_text` identiques sur `quick` (150) ; rejeux de la frontière
  (`js_runtime_unit`, 2 formats) identiques. Revue de l'intégration
  (`8fdcf2d`) : les quatre étapes `plumber_*` repassées sur `quick` (150),
  `adv-plumber` (199) et `adv-structure` (441) avec un cache d'oracle neuf
  (traces Python régénérées), toutes identiques, et `plumber_doc` sur `all`
  (1 853/1 853), qui confirme la résolution de la fusion PAGE × DOC
  (`f3fb6e3` : les cadres de `PDF.annots`/`hyperlinks` et d'`apply_crop_fn`
  de PAGE, `_crop_fn` et la fonction d'`utils` comptés une fois chacun).
- **Mutations** (lignes changées depuis `wave4/join`, `--in-diff`) :
  frontière 62 mutants, 54 détectés, 6 non viables, 2 manqués (le bras
  `strict_metadata` d'`open`, tué depuis par une session ; la garde de
  `label_pages`, équivalente : la recherche du handle vivant retrouve
  celui que la première occurrence a créé) ; pdfminer 44 mutants, 19
  détectés, 23 manqués, tous dans le partage des instantanés
  (`CMapDBCaches::same`, `Rest::same`, `capture`, `PyInterned::is_empty`),
  tués depuis par des tests unitaires (repassés à la main dans une copie)
  sauf trois équivalents : `same` → `false` et la garde de `capture` →
  `false` (aucun partage : mémoire seulement ; le test qui exige le
  partage tue les deux) et le bras `(Some, Some)` de `Interned::restore`
  supprimé (le parcours remonte alors jusqu'à la racine : même ensemble,
  plus lentement), `fmt` de `Debug for PyInterned` (texte de débogage) ;
  pycompat et pdfplumber 24 mutants, 19 détectés, 4 non viables, 1 manqué
  (`warnings::reset`, testé depuis). Passe `Ok(..)` : frontière 148
  mutants, 131 détectés, 15 non viables, 1 délai (détecté), 1 manqué (la
  garde de `label_pages`, ci-dessus) ; bibliothèque 47 mutants, 1 manqué
  (`bind` des méthodes de `Page`, que la frontière précède : tué par un
  test qui appelle la bibliothèque directement). À la main : les corps
  `Ok(..)` sans `Default` d'`open` (options ignorées), `mask_filter`
  (tout vrai), `get_attr_filter` (`AttrFilter::All`), le cadre de
  `mask_of` (fenêtre `figures-filter`) et le site `MASK_OF_OBJECTS`
  (fenêtre `climit-tf-list-filter`) : détectés.
- **Laissés ouverts, documentés** (`public-api.md`, `port-strategy.md`) :
  les noms `inline-<k>` répétés d'une page à l'autre (décision de la
  vague 4 gardée), les flux d'images copiés entiers dans chaque résultat,
  le texte des `log.*` de pdfminer, la copie profonde d'un document lu
  depuis des octets par `structure_tree`.

## Vague 5 : l'API JavaScript publique

Base commune : branche `wave5/base` (depuis `rewrite/rust-core` à
`73b88b9`). Trois chantiers parallèles, API, BOUNDARY et PERF, partent de
cette base et ne modifient pas le contrat ci-dessous sans accord écrit
(une note datée dans cette section, sur la branche qui le change).

### Ce que contient la base

- **Le contrat** : [`public-api.md`](public-api.md) (membres, options,
  asynchronie, valeurs, arguments, alias, durée de vie, mode worker,
  erreurs, table « Operations » : membre → opérations de la frontière →
  instructions de `oracle/unit/js_pages_run.py`) et `js/src/types.ts`.
- **L'épine de l'API** (`js/src/api.ts`) : chaque membre est un *job* du
  cœur, implémenté sur les opérations de `crates/wasm/src/pages.rs` ;
  documents (`pages` gardées jusqu'à `close()`, `metadata`, listes,
  `annots`, `structure_tree`, sérialisations, `close`, `release`,
  `asyncDispose`, `page(ref)`), pages (géométrie, listes, texte, mots,
  lignes, `search` en un job, pages dérivées puis leur `page_info`,
  `filter` en un job, tables et `Table`, sérialisations, `close`).
  En attente (rejettent `NotImplemented`) : `debugTablefinder`,
  `structTree` (`PDFStructTree`), `PDFStream` par référence.
- **Changements partagés faits dans la base** (frontière et paquet) :
  file FIFO des requêtes et des jobs par cœur (`Core.exclusive`,
  `Serial`, dans la page et dans le worker : sans elle, la relance d'un
  appel nul laissait passer un appel fait après lui) ; les erreurs portent
  l'exception qu'elles enveloppent (`cause`, `errors.rs` → `error.cause`) ;
  `doc_open_file` (`open(path)` de Node = `pdfplumber.open(path)`,
  `Source::Path`) ; `open()` transmet toute option sauf `logLevel` et
  `onWarning` ; format de résultat typé (`set_format("typed")`,
  `FORMAT_TYPED` : PJB1 sans tables, décodé en valeurs typées,
  `js/src/core/typed.ts`) ; crochets d'oracle (feature `oracle-hooks`,
  `crates/wasm/src/oracle.rs`, `dist/pdfplumber-oracle.wasm`, jamais
  livré) ; `page_search` prend des spans en points de code
  (`code_points`) ; `NotImplemented` d'un `NotPorted` garde module et
  classe.
- **Le harnais** (ci-dessous) : sessions `oracle/unit/js_api.py` et leur
  rejoueur, traces des étapes `plumber_*` par l'API (`js/trace/`,
  `oracle.verify --engine js-api`).
- **Les mesures de référence** (« Référence PERF », ci-dessous) et leurs
  outils : `crates/pdftrace/examples/plumber_phases.py` (jumeau Python de
  `plumber_phases.rs`), `js/bench/phases.mjs` (les mêmes phases par l'API
  publique, build `dist`).

### Contrat de l'API (décisions)

- **Un job par membre, FIFO.** Le cœur est un processus Python qui
  exécute une instruction après l'autre : chaque membre est un job mis en
  file au moment de l'appel (`Core.exclusive`), ses opérations
  s'exécutent dans l'ordre des appels quel que soit l'ordre des `await`,
  chacune jusqu'au bout (relances d'appel nul comprises). `search` (la
  `RegExp` entre `page_search_text` et `page_search`) et `filter` (la
  fonction entre `page_objects` et `page_filter`) sont un job chacun. Un
  membre ne lève jamais de façon synchrone (promesse rejetée), hors la
  géométrie (`width`, `height`), propriété synchrone comme en Python.
- **Instruction et récursion.** Le plan d'un membre est sa ligne de la
  table « Operations » ; chaque opération est une instruction au niveau du
  module de `js_pages_run.py` (`ENTRY_DEPTH`) : une `RecursionError`
  arrive par l'API là où elle arrive dans ce script, ce que les sessions
  vérifient (fenêtres de profondeur d'`adv-plumber`). Les étapes de
  l'oracle appellent pdfplumber depuis des générateurs imbriqués
  (cadres et sites de chargement à eux) : sur les fenêtres de profondeur
  des corpus adverses, une trace écrite par l'API peut lever plus loin ou
  moins loin que la trace de l'étape, par construction ; la parité de ces
  fichiers par l'API est celle des sessions.
- **Caches.** `pdf.pages` : le tableau de la première lecture résolue,
  jusqu'à `close()` (qui vide `_pages` en Python) ; une lecture rejetée
  n'est pas gardée. Les autres getters exécutent leur instruction à chaque
  lecture ; un chantier peut garder les valeurs résolues (jamais les
  rejets : la seconde exécution d'une instruction qui a levé peut réussir,
  un site spécialisé tenant deux unités C de moins), à condition que le
  plan (les sessions) le dise.
- **Durée de vie.** `close()` = `PDF.close()` / `Page.close()` (le document
  reste utilisable, comme en Python) ; `release()` libère le document dans
  le cœur (pages, tables, flux, historique) ; ensuite tout membre qui a
  besoin du cœur rejette `DocumentReleased` (erreur JavaScript, sans
  équivalent Python) ; `await using` = `close()` puis `release()` ; un
  `FinalizationRegistry` libère un document injoignable (une page, une
  table le retiennent). Libérer ne change aucun résultat.
- **Valeurs et arguments** : ceux de `js-runtime.md` (« Valeurs ») ; une
  `Page` dans un résultat est un `PageRef`, `pdf.page(ref)` donne le même
  `PDFPlumberPage` que `pdf.pages` ; un objet de résultat rendu en argument
  traverse par valeur (un flottant entier devient un `int`, visible
  seulement dans un texte).
- **Alias** : tout nom pdfplumber à `_` a son alias camelCase (et
  l'inverse), un seul membre ; options et clés restent celles de
  pdfplumber.
- **Erreurs** : `PDFPlumberError` (`name` = classe Python, `cause` =
  l'exception enveloppée), `NotImplemented` (membre en attente ou
  `NotPorted`), `DocumentReleased`, `CoreCrash`.
- **Mode worker** : même API ; jobs en file dans le thread appelant, la
  fonction de `filter` et la `RegExp` de `search` y tournent aussi ; le
  worker exécute ses messages un par un (`Serial`).
- **Journalisation** : la configuration de référence reste « logging
  coupé » pour les résultats et la récursion ; le texte transmis à
  `onWarning` est un diagnostic seulement.
- **Hors périmètre** : `display.py`/`to_image`, `repair.py`, `cli.py`.

### Harnais

**Sessions (spécification de l'API).** `oracle/unit/js_api.py`
(générateur à empreinte, `python -m oracle.unit js_api`, après `plumber`
dont il lit `adv-plumber`) écrit `<cache>/unit/js_api/<fp>/sessions.jsonl`,
une session par ligne :

```
{"in": [session], "out": [{"id": n, "value": JSON typé}, ...]}
session = {"name", "node_only", "steps": [{"calls": [appel...], "await": [k...]}]}
appel   = {"id", "on": [id d'appel, index...] | null, "get"|"call": membre,
           "args": [JSON typé...], "input": {"bytes_of": rel}|{"bytes": hex}|{"path": rel},
           "predicate": nom (filter)}
```

Les appels d'une étape sont faits sans attendre, puis attendus dans
l'ordre de `await`. Le générateur traduit chaque appel en instructions de
`js_pages_run.py` selon la table « Operations » (le plan des membres est
écrit là, une fois), exécute la session dans un processus neuf
(`js_pages_run.py` : cibles `[cas, index]`, `doc_open_file`, `causes`,
motifs compilés `{"$compiled": source}`), et déduit des résultats ce que
chaque appel doit donner : une valeur, une `Error{name, message, cause}`
(Python, ou de l'API : `DocumentReleased`), `Unresolved{}` (un appel sur
un objet jamais fait : aucun côté ne l'exécute). Valeurs neutres : un objet
de l'API est un `Named` (`PDFPlumberPage{géométrie}`, `Table{bbox, cells,
rows, columns}`, `PDFPlumberDocument{}`, `Page{page_number}`), le nom
d'une image en ligne `"<inline k>"` des deux côtés (rang par page ;
l'écart `str(id())`/`inline-<k>` est documenté et vérifié par les traces),
l'ensemble des directions de `validate_directions` dans l'ordre du port.
Couverture de la base : 40 sessions, 1 595 appels (dix fichiers réels
avec chaque membre et alias de la première page et du document, chaînes
de pages dérivées, prédicats, tables et `Table`, sérialisations,
`close` puis pages refaites, `release` puis `DocumentReleased` ; options
d'ouverture et mots de passe ; `open(path)` (Node) ; ordre d'attente
inversé ; pages CJK aux CMaps paresseuses ; 26 PDF d'`adv-plumber`, dont
des fenêtres de profondeur et d'unités C). Rejoueur :
`js/test/shared/api-sessions.mjs`, **par l'API publique seulement** ; lancé
par vitest sur les sources (`js/test/node/api.test.ts`) et sur le build
`dist` (`api-dist.test.ts`), par Playwright dans Chromium, Firefox et
WebKit, dans la page et en worker (`js/test/browser/api.spec.ts`, sessions
`node_only` exclues). Un membre en attente entre dans les sessions avec
son opération (BOUNDARY ajoute l'instruction à `js_pages_run.py`, API la
ligne du plan et les appels).

**Traces par l'API publique.** `js/trace/pdftrace-js` (Node, build `dist`
avec `dist/pdfplumber-oracle.wasm`) a la ligne de commande de `pdftrace`
et écrit les traces des étapes `plumber_objects`, `plumber_text`,
`plumber_tables`, `plumber_doc` en suivant leur parcours Python
(`js/trace/plumber_*.mjs`, tables d'options générées par
`oracle/codegen/plumber_ops.py` dans `js/trace/plumber-ops.json`,
`--check`) ; `python -m oracle.verify --engine js-api --stage S --set X`
le lance à la place de `pdftrace`. Égalité octet pour octet :

- chaque fichier dans un cœur neuf (un processus Python), au format typé
  (`set_format("typed")`) : la valeur publique que l'API rend est la
  projection (`project`) de la valeur typée, que l'étape lit dans
  `core.lastTyped` juste après l'appel (file FIFO : rien entre les deux) ;
  un `float` est un `PyFloat` (`3` et `3.0` distincts), un `tuple` un
  `PyTuple`, un `dict` un `PyDict` (ordre des clés de Python, clés
  entières comprises), un `int` un nombre ou un `bigint` ;
- `js/trace/canon.mjs` : `canon` d'`oracle/canon.py` sur ces valeurs
  (`repr` de flottant de CPython depuis les chiffres les plus courts de
  `toExponential`, `json.dumps(ensure_ascii=True)` par unité UTF-16 — une
  paire de substituts s'écrit comme Python écrit un caractère astral —,
  octets au-delà de 256 par SHA-256, `Stream(...)` avec le `rawdata` lu à
  la fin de l'appel, exceptions par `pythonModule`/`pythonQualname`) ;
- crochets internes (`pdfplumber-js/internal`, jamais publics), là où la
  sémantique publique diffère volontairement de Python, chacun documenté
  dans `js/src/hooks.ts` et `crates/wasm/src/oracle.rs` : rang des `id()`
  de l'analyse (`open(..., {[ORACLE]: {ids}})` → `oracle_doc_open`, et
  `oracle_page_ids` pour écrire la ligne `ids`) ; spans de `re`
  (`search(..., {[ORACLE]: {spans}})`, en points de code,
  `page_search` `code_points`) ; identité des objets
  (`oracle_object_indices` : les pages dérivées de Python tiennent les
  dicts du parent, les résultats JS sont des copies) ; format typé (une
  vue sans perte, pas une entrée).

Résultat sur `quick` (150 fichiers, `heavy.sh`, `-j 3`) :
`plumber_objects` 150/150 et `plumber_text` 150/150 identiques par
l'API (149 avant la correction des spans en points de code :
`lone-surrogates.pdf`, une paire de substituts que JS fusionne) ;
`plumber_tables` et `plumber_doc` identiques hors les lignes des
opérations en attente (`finder ...` de `debug_tablefinder` ; `tree`,
`find_all`, `find`, `mcids`, `element_bbox`, `element_repr` de
`PDFStructTree`).

Vérification de la base (commit de fin de `wave5/base`, `heavy.sh`) :
`scripts/check.sh` vert (42 min ; vitest : 123 tests, dont les 40 sessions
sur les sources et sur `dist`, et `canon.mjs` contre les 6 292 cas
d'`oracle/unit/canon.py` ; `plumber_objects` et `plumber_text` par l'API
sur `micro` : 17/17) ; `scripts/check-browsers.sh` vert (345 tests en
17 min : les tests du runtime et les 39 sessions qui ne sont pas
`node_only`, dans Chromium, Firefox et WebKit, dans la page et en worker) ;
`plumber_text` par l'API sur `quick` repassé après la correction des
spans : 150/150.

### Chantiers et propriété des fichiers

| Chemins | API | BOUNDARY | PERF |
|---|---|---|---|
| `js/src/api.ts`, nouveaux modules `js/src/api/*`, `types.ts`, `search.ts`, `exports.ts`, `index.ts`, `browser.ts`, `browser-internal.ts`, `errors.ts`, `hooks.ts` | écrit | lit | lit |
| `js/src/core/*`, `pdfobjects.ts`, `worker/*`, `platform/*`, `internal.ts` | lit | écrit | lit (mesures) |
| `crates/wasm/src/{ops,pages,oracle,cmaps,errors,handles,abi,strings}.rs`, `crates/wasm/tests/*` | lit | écrit | lit |
| `crates/wasm/src/wire/*`, `js/src/core/wire.ts` | lit | ajouts seulement (protocole 1) | écrit |
| `crates/pdfplumber`, `crates/pdfminer`, `crates/pycompat` | — | sites `log.*` et `pycompat::logging` (protocole 2), analyse des CMaps | écrit |
| `crates/pdftrace/examples/*`, `js/bench/*`, `scripts/size.mjs` | — | — | écrit |
| `oracle/unit/js_api.py`, `js/trace/*`, `js/test/shared/api-sessions.mjs`, `js/test/node/api*.test.ts`, `js/test/browser/api.spec.ts`, `oracle/verify.py` (`--engine`) | écrit | lit | lit |
| `oracle/unit/js_pages_run.py`, `oracle/unit/js_runtime.py`, `js/test/shared/tests.mjs` | ajouts d'instructions coordonnés (protocole 3) | écrit | — |
| `docs/public-api.md`, `README.md` | écrit | — | — |
| `docs/js-runtime.md` | — | écrit | section « Mesures » |
| `docs/architecture.md` « Vague 5 » | sa sous-section | sa sous-section | sa sous-section et « Référence PERF » |
| `scripts/check.sh`, `check-browsers.sh`, `build-*.mjs` | — | écrit | — |

Protocoles :

1. **Fil (`wire`)** : PERF possède l'encodage (allocation, table de
   chaînes, colonnes, décodeur) ; BOUNDARY y ajoute seulement des sortes
   de valeurs (le flux par référence : nouvelle étiquette, nouvelle
   méthode de `Writer`, nouveau cas du décodeur), en un commit à part,
   annoncé dans sa sous-section ; aucun ne change le format typé ni le
   sens d'une étiquette existante sans l'autre. La jonction résout les
   conflits textuels en gardant les deux.
2. **`log.*` de pdfminer** : BOUNDARY ajoute le texte des messages aux
   sites existants (fermetures de formatage, `pycompat::logging`) en un
   commit de bout en bout, sans toucher au comptage des cadres ; PERF ne
   reformate pas les lignes qui portent un appel de journalisation. Toute
   autre modification de `crates/pdfminer` par BOUNDARY (l'analyse des
   CMaps d'un document) vit dans un module nouveau.
3. **Instructions de `js_pages_run.py`** : BOUNDARY ajoute l'instruction de
   chaque nouvelle opération (ajout pur d'une branche du `if`), API ajoute
   à `js_api.py` la ligne du plan du membre et ses appels. Toute
   modification de `js_pages_run.py` invalide les données `js_runtime` et
   `js_api` (empreinte) : à régénérer.
4. **Traces et caches** : aucun chantier ne change `oracle/stages/*`,
   `oracle/canon.py`, `oracle/typed_json.py`, `oracle/depth.py` (ils sont
   dans l'empreinte des caches de l'oracle).

**Chantier API** : le reste de la surface (`debugTablefinder`,
`PDFStructTree` et ses éléments, `PDFStream` par référence côté API,
mémoïsation éventuelle des getters, `FinalizationRegistry` des pages et
tables, `README.md` : démarrage rapide et migration depuis l'ancien port
`legacy-pdfjs`), les sessions de chaque membre et option (dont les
nouveaux), les quatre étapes par l'API identiques sur `all` et les corpus
adverses (hors fenêtres de profondeur, couvertes par les sessions), les
tests de mutation de la couche API (Stryker hors ligne s'il tourne depuis
`node_modules`, sinon mutants à la main de chaque branche, chaque
survivant justifié).

**Chantier BOUNDARY** — opérations que l'API attend :

| Opération | Instruction (`js_pages_run.py`) | Résultat |
|---|---|---|
| `page_debug_tablefinder [settings?]` | `obj.debug_tablefinder(*args)` | `{edges, intersections: [[point, {v, h}]...], cells, tables: [table + handle...], settings}` |
| `struct_tree [page handle ou None]` (sur le document) | `PDFStructTree(obj, page)` | handle de l'arbre, `children` (éléments : champs du dataclass, `children`, chemin) |
| `struct_find_all [matcher]`, `struct_find [matcher]` (arbre ou élément) | `obj.find_all(m)` (liste), `obj.find(m)` | chemins des éléments ; `matcher` = nom, `{"regexp": ...}` (spans JS de `match` au début du type), ou masque calculé par JS |
| `struct_element_bbox [path]` | `tree.element_bbox(el)` | bbox ou exception |
| `struct_all_mcids [path]`, `struct_element_repr [path]` | `list(el.all_mcids())`, `repr(el)` | — |
| `page_info` : `width`, `height` | lus séparément | valeur ou exception chacune |
| `stream_rawdata`, `stream_data`, `stream_get_data`, `stream_get_rawdata` [handle] | `s.rawdata`... | octets ; flux par référence (étiquette de fil, durée de vie liée au document) |
| `doc_cmap_scan` (préchargement) | aucune (analyse sans effet Python, sur un document à part) | noms de CMaps probables |

Aussi : arguments `PSLiteral`, `PDFObjRef`, `PDFStream`, `PageRef`
décodés comme les objets qu'ils représentent ; texte des `log.*` de
pdfminer vers `onWarning` comme `logging` le formaterait (nom du logger,
niveau, `%`-formatage, une fois par enregistrement) ; préchargement des
CMaps d'un document pour que les appels nuls deviennent rares, sans
changer aucun résultat ni la récursion (une CMap présente dès le départ
= Python avec toutes ses CMaps) ; croissance de l'historique des appels
nuls (aujourd'hui il ne raccourcit qu'à la libération) ;
`docs/js-runtime.md`.

**Chantier PERF** : chemins chauds de `crates/pdfplumber`,
`crates/pdfminer`, `crates/pycompat` (`page.objects`/`process_object` :
`resolve_all`, dicts d'attributs, conversions ; texte, mots, tables de la
vague 1 ; layout), coût et allocations de l'encodage, taille et vitesse du
Wasm `dist` ; mesuré en natif (`plumber_phases.rs` et son jumeau Python),
dans Node (`js/bench/phases.mjs`) et dans les navigateurs ; chaque
changement garde toutes les traces et toutes les fenêtres de profondeur
(cadres et unités C) identiques ; avant/après avec la méthode écrite.

**Fini pour la vague 5** (en plus de `verification.md`) : chaque membre et
option de `public-api.md` implémenté et vérifié contre Python *par l'API
publique* (sessions et traces), dans Node, Chromium, Firefox, WebKit et en
mode worker ; performances mesurées avant/après, méthode notée ; aucune
optimisation ne change une trace.

### Référence PERF (base, `quick`)

Méthode : `quick` (150 fichiers, 20 premières pages, 302 pages),
`scripts/heavy.sh` (un job à la fois, 5 Go), machine partagée de 8 cœurs ;
les mêmes phases que la jonction de la vague 4 : `pdfplumber.open` et
`pdf.pages` (« ouverture »), puis par page `page.layout`, `page.objects`,
`extract_text()`, `extract_words()`, `find_tables()`. Natif :
`target/release/examples/plumber_phases <cmap-dir> < liste` (médiane de
3 passes) ; Python : `crates/pdftrace/examples/plumber_phases.py` (le
jumeau, CPython 3.12, après les imports, 1 passe) ; Node :
`js/bench/phases.mjs` sur le build `dist` (`wasm-opt -O3`), par l'API
publique, valeurs décodées (médiane de 3 passes ; `page.objects` y contient
`layout`, la phase n'est pas visible séparément) ; une seconde lecture de
`page.objects` (objets en cache dans le cœur) mesure la frontière seule
(encodage PJB1, copie, décodage). Liste : `path<TAB>mot de passe` de
`oracle.corpus --set quick`. Mémoire : `/usr/bin/time -v` (RSS maximale),
taille de la mémoire linéaire du module à la fin (elle ne rétrécit
jamais). Taille : `node scripts/size.mjs`.

| ms par page | ouverture | layout | objects | texte | mots | tables | total |
|---|---|---|---|---|---|---|---|
| Python 3.12 | 4,61 | 75,7 | 49,0 | 6,85 | 4,73 | 3,96 | 144,8 |
| Rust natif | 0,34 | 6,26 | 13,2 | 2,52 | 1,90 | 2,24 | 26,4 |
| Wasm, Node, API publique | 1,29 | (dans objects) | 26,7 | 2,70 | 2,30 | 2,07 | 35,1 |

- Rapports : natif ×5,5 sur Python (×12 pour `layout`, ×3,7 pour
  `objects`, ×2,7 pour le texte, ×2,5 les mots, ×1,8 les tables) ; l'API
  dans Node ×4,1 sur Python, ×1,33 au-dessus du natif.
- `page.objects` par l'API (26,7 ms) = frontière seule 4,9 ms (seconde
  lecture : 1,47 s pour les 302 pages) + calcul dans le Wasm ≈ 21,8 ms,
  contre 19,4 ms en natif (`layout` + `objects`) : le Wasm calcule à ×1,12
  du natif ; la frontière coûte 18 % de la phase. Point chaud inchangé :
  `page.objects` (`process_object`), la moitié du temps natif.
- Fichiers les plus lents (les mêmes partout) : `pdfjs-issue5509.pdf`
  (2,5 s natif, 3,0 s Node, 14,6 s Python), `highlights.pdf`,
  `issue-71-duplicate-chars-2.pdf`, `target-2024-ar.pdf`.
- Mémoire : RSS maximale 366 Mio en natif, 428 Mio pour Python, 520–550 Mio
  pour Node (600 Mio avec la seconde lecture des objets) ; mémoire linéaire
  du Wasm à la fin : 294 Mio (308 150 272 octets), atteinte par le plus gros
  document (elle ne rétrécit pas).
- Taille (`dist`, `wasm-opt -O3`) : `pdfplumber.wasm` 2 416 107 octets
  bruts, 890 959 gzip, 628 778 brotli (budget brotli 1 Mo) ; JS navigateur
  36 231 / 12 733 / 11 502 ; `dist/src` (Node) 100 404 / 30 539 / 26 792 ;
  CMaps 2 976 819 (`UniJIS-UCS2-H` + `to-unicode-Adobe-Japan1` : 155 903).
- Traces : écrire les traces par l'API (`--engine js-api`, `-j 3`) prend
  98 s pour `plumber_objects` sur `quick`, 75 s `plumber_text`, 63 s
  `plumber_tables`, 213 s `plumber_doc` (format typé, sans tables,
  canon en JavaScript).
- Mesures dans les navigateurs : `js/bench/browser-phases.mjs` (chantier
  PERF, ci-dessous) ; Firefox de Playwright (Nightly) n'est pas une
  référence de vitesse (`port-strategy.md`).

### Chantier PERF : réalisation (branche `wave5/perf`)

Branche `wave5/perf`, depuis `wave5/base` (`acf9fa6`). Aucune optimisation
ne change une trace : chacune garde le même calcul, les mêmes contrôles de
cadres et d'unités C dans le même ordre, et le même résultat ; ce qu'elle
retire, ce sont des copies, des allocations et des recherches. L'invariant
qui le garantit est écrit en commentaire à chaque site (« Performance: »
ou « performance: »).

**Méthode.** `perf` n'est pas permis sur la machine
(`perf_event_paranoid` = 4) : les profils natifs viennent de
`scripts/perf/sprof.c`, un échantillonneur chargé par `LD_PRELOAD`
(SIGPROF sur le temps CPU, pile par pointeurs de cadre : build
`RUSTFLAGS="-C force-frame-pointers=yes"`, lignes avec
`CARGO_PROFILE_RELEASE_DEBUG=line-tables-only`, tampon fixe de 32 Mio),
lus par `sprof_report.py` et `sprof_lines.py` ; les allocations, de
`scripts/perf/mcount.c` (nombre d'appels à `malloc`/`realloc`, pile d'un
appel sur 16) ; le Wasm dans Node, de `node --cpu-prof` sur un `dist` dont
le module garde ses noms (`wasm-opt -O3 -g`), lu par `prof_sum.py` et
`prof_callees.py`. Chaque mesure tourne sous `prlimit` ou dans le cgroup
de `heavy.sh`. Les phases : `plumber_phases.rs` (natif ; `PLUMBER_PHASES`
choisit les phases, pour compter les allocations d'une phase),
`js/bench/phases.mjs` (Node, API publique, `dist`),
`js/bench/browser-phases.mjs` + `phases.html` (nouveau : les mêmes phases
par l'API du paquet navigateur, dans Chromium, Firefox et WebKit, dans la
page et en worker), `crates/wasm/examples/encode_phases.rs` (nouveau :
l'encodage PJB1 des objets d'une page seul, avec une empreinte FNV-1a des
blocs). Avant et après se mesurent dans le même créneau de `heavy.sh`, en
alternant les deux builds (`wave5/base` reconstruit à part, avec les seuls
outils de mesure en plus), médiane de 3 passes ; les deux `dist` sont
optimisés (`wasm-opt -O3`, `node scripts/build.mjs` ; `check.sh` construit
le sien sans `wasm-opt`).

**Ce qui a changé** (par ordre de gain) :

- `Page.process_object` (`page.rs`), la moitié du temps natif : le dict
  est construit avec sa capacité, et une clé qu'il ne peut pas encore
  tenir (un attribut du `__dict__`, dont les clés sont distinctes ; une
  clé que `process_object` ajoute et qu'aucune classe de layout n'a) est
  ajoutée sans recherche (`Obj::push_new`, vérifié en debug) ; les clés
  sont faites une fois par fil (`AttrKeys`), `object_type`, les commandes
  de chemin et les littéraux du port aussi (`obj::static_str`) ;
  `resolve_all` d'une valeur que seul l'appelant tient (un tuple qu'il
  vient de faire) se fait sur place (`resolve_all_owned` : mêmes cadres ;
  Python fait un nouveau tuple, dont nul ne peut comparer l'identité avec
  celle de l'ancien, que lui seul tenait) ; le tampon des items est
  réutilisé ; les points de `original_path` passent par `point2coord` sans
  leur tuple (`point2coord_with`, mêmes cadres) ; `decode_text` d'une `str`
  ne fait plus une `str` par caractère et rend la même `str` quand
  PDFDocEncoding l'envoie sur elle-même (`decode_text_pystr`) ; le nom de
  l'espace de couleur est partagé entre les caractères consécutifs (cache
  d'une référence faible : il ne garde pas l'espace en vie, et son adresse
  ne peut pas en nommer un autre) ; `mediabox[i]` lit l'item d'une liste
  PDF sans convertir la liste (`py_getitem_index`).
- Valeurs partagées au lieu de copiées : `FontName::Str` et
  `MarkedContentAttr::Tag` (et `cur_tag`) tiennent une `PyStr` (tous les
  caractères d'une police tiennent la même, comme en Python) ; `ExtraAttrs`
  garde `mcid` et `tag` sans allocation (`Attrs`, deux attributs en
  ligne) ; `Unichr::to_pystr` partage les textes courts récents. L'identité
  d'une `str` n'est pas modélisée (immuable, jamais comparée par `is` dans
  le code porté).
- Interpréteur : la méthode d'un opérateur sans les trois remplacements
  quand son mot-clé n'a ni `*`, ni `"`, ni `'` (`keyword_op`) ; un seul
  vecteur par segment de chemin ; `int_hash` d'un petit entier sans
  vecteur d'octets (chaque recherche de largeur hachait un `Vec`).
- Arithmétique : `add`, `sub`, `mul` et les comparaisons d'un `int` et
  d'un `float` vont droit à `Num` (le résultat du chemin général, sans
  ses conversions d'objets PDF ni ses tests de séquence) ; les compteurs
  de `pycompat::recursion` sont `#[inline]` (l'erreur `#[cold]`).
- Libération : le `Drop` de `PyValue` sort tout de suite pour un scalaire
  ou un conteneur encore partagé, et la liste de travail des conteneurs
  imbriqués est gardée entre deux libérations. Allocations sur 30 fichiers
  de `quick`, toutes phases : 41,5 M `malloc` + 6,56 M `realloc` → 12,4 M
  + 0,83 M.
- Frontière (`crates/wasm/src/wire`, `js/src/core/wire.ts`) : les tables
  PJB1 lisent leurs cellules une fois, rangée par rangée (`Cell`),
  écrivent nombres et indices d'une colonne en place, et retrouvent la
  `str` d'une cellule par son adresse quand les cellules voisines la
  partagent (`intern_cell`) ; une valeur faite de scalaires et de
  séquences sur trois niveaux au plus (points, chemins) s'écrit sans liste
  de travail (`write_shallow`). Le format ne change pas : les blocs de
  `page_objects` des 150 fichiers de `quick` sont identiques octet pour
  octet entre `wave5/base` et `wave5/perf` (`encode_phases`). Côté
  JavaScript, `Reader.item` lit les cellules génériques (scalaires, listes
  et tuples sur trois niveaux) sans les cadres de `value()`
  (`js/test/node/wire.test.ts` : mêmes valeurs que `value()` pour chaque
  sorte, imbriquées au-delà de trois niveaux et sur 100 000 niveaux).

**Résultats** (`quick`, 302 pages, ms par page, médiane de 3 passes, même
créneau ; « frontière » = seconde lecture de `page.objects`) :

| ms par page | ouverture | layout | objects | texte | mots | tables | total | frontière |
|---|---|---|---|---|---|---|---|---|
| natif, base | 0,32 | 5,60 | 13,14 | 2,13 | 1,61 | 1,79 | 24,59 | — |
| natif, PERF | 0,28 | 4,50 | 5,17 | 1,54 | 1,25 | 1,59 | **14,33** | — |
| Node, API, base | 1,12 | (dans objects) | 23,62 | 2,10 | 1,87 | 1,76 | 30,47 | 4,21 |
| Node, API, PERF | 1,06 | (dans objects) | 13,87 | 1,63 | 1,52 | 1,39 | **19,47** | 2,88 |

- Natif ×1,72 (objects ×2,5, layout ×1,24, texte ×1,38, mots ×1,29,
  tables ×1,13) ; Node par l'API ×1,56 (objects ×1,70, frontière ×1,46).
  Rapport à Python (144,8 ms, référence ci-dessus) : natif ×10, API dans
  Node ×7,4. Le Wasm calcule à ×1,14 du natif (objects sans la frontière,
  10,99 ms, contre layout + objects natifs, 9,67 ms).
- Mémoire : RSS maximale native 366 → 318 Mio ; Node 588 → 557 Mio ;
  mémoire linéaire du Wasm à la fin 295 → 266 Mio (309 002 240 → 279 117 824
  octets).
- Navigateurs (`browser-phases.mjs`, une passe, objects / frontière en
  ms par page, base → PERF) : Chromium 148 page 26,67 → 14,17 / 4,57 →
  2,79, worker 25,68 → 14,83 / 4,89 → 3,11 ; WebKit 26.4 page 24,98 →
  14,67 / 3,89 → 2,86, worker 25,35 → 14,59 / 4,54 → 3,25 ; Firefox 150
  (Nightly de Playwright) page 139,7 → 78,8 / 16,5 → 11,8, worker 140,3 →
  76,7 / 16,5 → 11,6. Texte, mots, tables : −13 à −35 % partout.
- Encodage seul (`js/bench/node.mjs --passes 5`, corpus de
  `js-runtime.md`, Node 26.12) : PJB1 un appel par page 602 → 425 ms
  (cœur 365 → 236, décodage 236 → 189), tout en un appel 820 → 577 ms ;
  JSON inchangé (2 847 → 2 786).
- Taille (`dist`, `wasm-opt -O3`) : `pdfplumber.wasm` 2 416 107 →
  2 466 476 octets bruts (+2,1 %), 628 778 → 632 096 brotli (+0,5 %).
- `all` en natif (1 853 fichiers, 3 654 pages, une passe de chaque build) :
  24,65 → 14,38 ms par page (×1,71 ; objects 12,34 → 4,79), RSS maximale
  546 → 501 Mio.
- Les 12 fichiers les plus lents de `all` (natif, base ; 176 pages :
  `cms-1500.pdf`, `pdfjs-issue19281.pdf`, `pdfjs-issue5509.pdf`,
  `i1040nr.pdf`, `pdfjs-hudsonsurvey.pdf`...), médiane de 3 passes : natif
  109,0 → 69,1 ms par page (×1,58 ; objects 53,1 → 23,3, layout 28,6 →
  23,4, tables 12,9 → 10,7) ; Node par l'API 139,5 → 88,6 (×1,58 ;
  frontière 23,1 → 15,6). Ce sont des formulaires et des plans denses en
  chemins : le layout et les tables y pèsent plus, et y gagnent moins.

**Ce qui reste** (profil natif sur les fichiers les plus lents, après
PERF) : l'allocateur (≈ 20 % des échantillons, `malloc`, `free` et leurs
fonctions internes), les copies et libérations de `PyValue` (≈ 9 %),
`process_object` lui-même (≈ 5 %), l'analyseur PostScript des flux de
contenu (`Lexer::parse1`, `nextobject` : ≈ 4 %), l'extraction des mots
(≈ 5 %, `extract_wordmap`, `char_begins_new_word`, `cluster_objects`).
Sur les fichiers les plus lents, `find_tables` est surtout `page.edges`
(les arêtes de chaque courbe, `curve_to_edges`) et `merge_edges` : des
dicts faits et copiés, pas un algorithme à revoir (Python fait le même
travail, dans le même ordre). Le
gros des allocations restantes vient des dicts d'objets eux-mêmes (un
`Rc` par dict, un tuple par `matrix`, `pts`, couleur) : un arena ou des
dicts à forme fixe changeraient le modèle de valeurs de `crates/pdfplumber`
(hors de ce chantier). La frontière coûte encore ≈ 20 % de
`page.objects` dans Node (2,9 ms sur 13,9).

**Vérification** (commit de ce chantier) : `scripts/check.sh` vert ;
`scripts/check-browsers.sh` vert (345 tests) ; les 13 étapes identiques
sur `quick` (150/150 chacune) ; `oracle.wasm_parity` sur `quick`, 13
étapes, 0 différence, `dist wasm = native` ; `plumber_objects` et
`plumber_text` par l'API (`--engine js-api`) 150/150 sur `quick` ; les 13
étapes sur `all` (1 853/1 853 chacune, sauf `plumber_objects` 1 852 : la
ligne `ids` connue de `pdfjs-issue19624-text.pdf`) et sur les cinq corpus
adverses (`adv-objects` 333, `adv-pages` 285, `adv-plumber` 199,
`adv-structure` 441, `adv-interp` 353 : tous identiques, sauf les écarts
connus d'`adv-interp`, `plumber_text` 336 et `plumber_tables` 341, les
mêmes fichiers `op-*`/`text-*` aux coordonnées NaN que sur `wave5/base`,
`port-strategy.md`).

**Profondeur de `process_object`** (`oracle/unit/plumber_frames.py`,
`crates/pdftrace/tests/plumber_frames.rs`, nouveaux). Ni les traces des
étapes ni les balayages de `plumber_sweep` n'atteignent la limite de
récursion dans `Page.process_object` : l'interprétation va plus profond
d'abord, et un balayage ne voit que si l'opération lève. Tous les mutants
à la main des instructions qui comptent les cadres des chemins PERF (et de
`process_object` lui-même) survivaient. Le générateur prend, pour chaque
objet de la première page de 26 fichiers (`micro`, `adv-plumber` :
courbes, rectangles, lignes, images, figures, contenu marqué, MediaBox
décalées), le layout calculé d'abord, la profondeur la plus grande de
l'appelant d'où `page.process_object(obj)` ne lève pas `RecursionError`,
le long de chaque compteur (`py` : cadres Python en plus ; `c` : listes
imbriquées comparées, une unité C par niveau), et l'issue à cette
profondeur ; de même pour `decode_text` et `resolve_all` seuls. Chaque
essai tourne dans un processus forké depuis le même cadre ; la profondeur
de l'appelant est mesurée au niveau 0 puis déplacée d'un cadre ou d'une
unité par niveau (vérifié à 1, 7 et 30 niveaux). Le rejeu passe par
`Page::process_object` (méthode publique de `page.py:353`),
`decode_text_value`, `resolve_all_value` et `resolve_all_owned` : **cadres
Python identiques** pour chaque objet et chaque fonction, sur `wave5/base`
comme sur `wave5/perf`.

**Écart connu, pour la jonction : les unités C** (vérifiées seulement avec
`PLUMBER_FRAMES_C=1`). Le port compte une unité C de moins que Python au
point le plus profond de chaque objet (336 cas) et de chaque fonction (18),
**à l'identique sur `wave5/base`** (le rejeu y donne les mêmes lignes) :
écarts de la vague 4, que les chemins PERF reproduisent sans les changer.
Causes trouvées : `isinstance` avec un tuple de classes prend une unité
le temps de la vérification (`object_recursive_isinstance`, « in
__instancecheck__ » : `isinstance(x, (list, tuple))` de `resolve_all`,
`(LTChar, LTTextContainer)` et `(LTCurve,)` de `process_object`) ; les
abonnements `typing` des annotations de `process_attr` prennent des unités
C que `pycompat::typing_cache` ne compte pas (il ne compte que les cadres :
`Optional[Tuple[str, Any]]` en prend 6 quand il est en cache) ;
`decode_text` d'une `str` en prend 4 (3 pour la chaîne vide), le port 2. Un
correctif de la première cause (unité transitoire dans les trois
`resolve_all` et les deux `isinstance` de `process_object`) aligne les
rectangles et les lignes ; il n'est pas dans cette branche (il change le
comptage, donc toute la vérification) : à faire avec les deux autres, puis
`PLUMBER_FRAMES_C` par défaut.

**Tests de mutation** (`cargo mutants --in-diff` sur le diff de
`wave5/base`, `--profile mutants -j 1`, `heavy.sh`) :

- `crates/wasm/src/wire` (110 mutants) : 79 tués, 23 non viables, 8
  survivants ; deux tests unitaires ajoutés (le type de colonne qu'écrit la
  table est celui de `column_kind` ; le contrat d'`is_shallow`), puis les
  50 mutants de ces fonctions : 41 tués, 7 non viables, 2 survivants
  équivalents (`|` → `^` dans `kind_of` : drapeaux de bits distincts).
- `crates/pycompat/src/num.rs` (12) : 11 tués ; `*v < 0` → `<= 0` dans
  `int_hash` équivalent (le hash de 0 est 0 dans les deux branches).
- `crates/pdfminer` et les vidages de `pdftrace` (37, sans le test `corpus`
  de 5 minutes de `converter_unit`) : 14 tués, 19 non viables, 4
  survivants équivalents : le seuil de longueur de `shared_text` (×3 : il
  choisit quelles chaînes passent par le cache, le résultat est la même
  `str`) et la garde de `keyword_op` remplacée par `false` (toujours le
  chemin général, même nom de méthode).
- `crates/pdfplumber` (154, avec `table.rs` et `geometry.rs`) : 24 tués,
  121 non viables, 9 survivants ; un test de libération profonde par sorte
  de conteneur tue les 4 mutants du chemin rapide de `Drop` (relancés : 6
  tués sur 7) ; restent équivalents : la borne de capacité de la liste de
  travail de `drop_nested` (garder ou non le tampon), `int_float_operands`
  → `None` ou sans un bras (le chemin général donne le même `Num`),
  `Obj::with_capacity` → `Default` (une capacité).
- Passe `Ok(..)` (`scripts/mutants-ok.sh`, fonctions écrites ou changées
  par PERF) : 5 tués sur 5.
- Mutants à la main (`PyResult` et types sans `Default`, instructions de
  comptage) : tués, `Attrs::push` qui inverse les deux premiers attributs,
  `as_slice` qui perd le second, `with_items` vide, `int_float_operands`
  qui inverse ou convertit un opérande, `classify` qui rend tout générique,
  et, par `plumber_frames`, `resolve_all_owned` sans son cadre (séquence ou
  scalaire) ou sans le cadre de son expression génératrice,
  `decode_text_pystr` avec `leaf(1)`. Survivants justifiés : `Attrs::push`
  d'un troisième attribut qui le perd (jamais plus de deux : `mcid`,
  `tag`) ; le cadre de `point2coord_with` et l'unité par point de
  `original_path` (plus haut que la chaîne de `pts`, qui passe avant dans
  le temps : jamais le premier à dépasser la limite, chez Python non
  plus) ; le cadre de `process_attr` pour un attribut hors `ALL_ATTRS`
  (même raison, code de la vague 4) ; les `eval_entry` de
  `resolve_all_owned` et `decode_text_pystr` (unités C : l'écart connu
  ci-dessus empêche de les départager ; ils reproduisent ceux du chemin
  général).
- `js/src/core/wire.ts` (`Reader.item`, 18 mutants à la main) : 11 tués ;
  7 survivants qui envoient une cellule à `value()` plus tôt ou plus tard
  (garde toujours ou jamais, `depth < 0`, colonne par `value()`,
  `SHALLOW` 0 : mêmes valeurs) ou testent ce qu'un bloc de table ne peut
  pas contenir (une cellule en fin de bloc : la table de chaînes la suit).

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
