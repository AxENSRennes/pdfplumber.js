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
  (écart non modélisé, `verification.md`, « Étapes »).
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
  `inline-<k>`.
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
  de Python ; ses cadres sont à mesurer.
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
  des figures (écart documenté, `public-api.md`).
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
  de passe qui n'est ni `None` ni une `str` (`NotPorted`) ; le fichier
  fermé d'un `PDF` ouvert par chemin après `close()` (Python lève
  `ValueError: seek of closed file` à la lecture suivante du document ; le
  port lit encore : seul un objet jamais lu avant la fermeture le
  montrerait) ; `getattr` d'une méthode dans `to_dict`/`to_csv`
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
  `password`, `unicode_norm` de tout type), rejoué par
  `crates/pdftrace/tests/plumber_unit.rs`.

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
`oracle/depth.py`, `_call_sites` : premier commit de PAGE, ci-dessus). Le code de la vague 1 (`utils/text.rs`, `table.rs`, géométrie,
regroupement) ne compte pas de cadres : il ne récurse pas et pdfplumber
l'appelle au niveau de l'utilisateur ; restent les unités C des
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
