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
  `{"in": [<JSON typé>], "out": "<texte attendu>"}` par cas ;
  `crates/pdftrace/tests/canon_unit.rs` vérifie que Rust décode chaque valeur,
  écrit le même `canon` et ré-encode le même JSON typé, au caractère près.
- Côté Rust, une `str` Python est un `value::PyStr` dans le harnais (un
  `String`, ou ses points de code quand elle contient un surrogate), et un
  `pycompat::PyStr` dans le code porté (octets `surrogatepass`) ; les deux se
  convertissent sans perte (`value.rs`).
- Une NaN des cas unitaires de `text_tables*` porte, dans sa charge utile,
  le numéro de son objet Python dans le cas (`tag_nans` de
  `oracle/unit/_nan_objects.py`) : le rejeu sait quelles valeurs sont le même
  objet (identité des NaN, `crates/pdfplumber/src/obj.rs`).
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
| `fonts` | les polices obtenues par l'interpréteur (le `fontmap` de la page, les entrées ajoutées au cache du `PDFResourceManager`) : classe et attributs, puis, pour chaque chaîne affichée, `decode` et, par code, `to_unichr`, `char_width`, `char_disp` |
| `interp` | chaque appel de l'interpréteur au périphérique de pdfplumber (`set_ctm`, pages, figures, `paint_path`, `render_image`, `render_string`/`render_char` et l'avance rendue, balises), avec les états graphique et texte, et où remontent les exceptions |
| `fonts_text`, `interp_text` | `fonts` et `interp` sur le `PDFTextDevice` de pdfminer (`render_char` rend `0`) au lieu du périphérique de pdfplumber : l'interpréteur vérifié sans convertisseur (`oracle/stages/interp_text.py`) |
| `layout` | le `LTPage` brut (`laparams=None`, le défaut de pdfplumber) puis analysé (`LAParams()`), tous attributs (`mcid`, `tag` compris), et le rang des `id()` de l'analyse (`ids`) ; non reproductible d'une construction du cache à l'autre sur les pages où des boîtes sont à égale distance (les `id()` sont des adresses : un tirage), la parité suivant le tirage du cache (`pdftrace --oracle-dir`) |
| `plumber_objects` (vague 4) | `pdfplumber.open(path, pages=[1..N], …)` : boîtes et `doctop` de chaque `Page`, `page.objects` de toute sorte, tous attributs, annotations, liens, `pdf.objects`/`annots`/`hyperlinks` ; puis la même chose avec `laparams={}` (boîtes et lignes de texte, caractères par référence à la première ouverture, `ids` : rang des `id()` de l'analyse, clé de contenu ; `ORACLE_INPUT`) |
| `plumber_text` (vague 4) | `extract_text` (simple, `layout=True`, jeux d'options), `extract_text_simple`, `extract_words`, `extract_text_lines`, `search` (correspondances du `re` de Python, `spans`, `ORACLE_INPUT`), `dedupe_chars`, sur la page et sur des pages rognées, filtrées, dédoublonnées |
| `plumber_tables` (vague 4) | `debug_tablefinder` (arêtes, intersections, cellules, tables, lignes, colonnes, texte), `find_table`, `extract_tables`, `extract_table` pour `lines`, `lines_strict`, `text`, `explicit` (lignes tirées de la page) et des variantes de réglages, sur la page et des pages dérivées |
| `plumber_doc` (vague 4) | métadonnées (`strict_metadata` des deux façons), arbre de structure (document et pages, `find_all`, `find`, `all_mcids`, `element_bbox`, `repr`), `crop`/`within_bbox`/`outside_bbox` (strict ou non, relatif ou non), `filter`, `dedupe_chars`, `to_dict`/`to_json`/`to_csv`, `close`, options `unicode_norm`, `raise_unicode_errors`, `pages`, mots de passe |

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
- La journalisation (`logging`) est coupée pendant une étape. pdfminer ne
  lit jamais son niveau de log, mais un appel de log coûte des cadres Python
  (`log.debug(...)` exécute `Logger.debug` puis `Logger.isEnabledFor` : deux
  cadres sous l'appelant, trois au premier appel d'un niveau sur un logger,
  `_acquireLock` remplissant le cache), ce qui décide où tombe une
  `RecursionError` dans une récursion profonde (`create_pages` journalise à
  chaque nœud). La référence est donc la journalisation coupée
  (`logging.disable(logging.CRITICAL)`), comme l'exécute l'oracle et comme
  se comporte l'API JS sans `onWarning` ; c'est aussi, au cadre près, la
  configuration courante qui fait taire pdfminer
  (`logging.getLogger("pdfminer").setLevel(logging.ERROR)` : `isEnabledFor`
  remplit son cache par `getEffectiveLevel`, un cadre plus bas, comme par
  `_acquireLock` ; mesuré : 4 cadres au premier appel, 3 ensuite, dans les
  deux cas). Ce n'est pas la configuration par défaut d'un utilisateur
  (aucun gestionnaire, niveau WARNING) : `log.debug` et `log.info` y
  coûtent la même chose, mais `log.warning` y est actif et exécute `_log`,
  `findCaller`, `makeRecord` et le gestionnaire `lastResort`, bien plus
  profonds (11 cadres sous l'appelant au plus profond, `Formatter.format`
  → `PercentStyle._format`, mesuré par une sonde bornée, le même avec
  `basicConfig()`, `multiprocessing` ou `asyncio` importés, `stderr`
  redirigé), et il formate le `%r` de ses arguments : sur un opérande
  imbriqué près de la limite C, ce `repr` lève `RecursionError`, que
  `StreamHandler.emit` relance, et la page échoue (la revue l'a montré pour
  `w` et pour l'avertissement de `TJ` de `render_string_horizontal`, sur un
  tableau de 20 000 niveaux) ; une chaîne de Form XObjects dont la Form la
  plus profonde avertit échoue un niveau plus tôt (326 au lieu de 327). Une
  configuration DEBUG fait de même pour chaque `log.debug`. Non modélisé :
  la référence reste la journalisation coupée, une configuration fixe que
  l'oracle exécute et que le port reproduit ; modéliser la configuration
  par défaut changerait la référence de toutes les vagues (chaque
  avertissement de `pdfinterp`, `pdfdevice`, `pdffont`, `pdfpage`,
  `pdfdocument`, `cmapdb`, `psparser`, avec le `repr` de ses arguments, et
  les fenêtres de profondeur de tous les corpus adverses). Le port compte
  les cadres de la journalisation coupée comme les autres
  (`pycompat::logging`, « Profondeur de récursion » de `architecture.md`).
- `oracle.stages.entry_depth()` mesure la profondeur (cadres Python, unités
  C restantes, `oracle/depth.py`) où s'exécute le corps d'une fonction
  d'étape ; `pdftrace::stages::STAGE_ENTRY` la reprend et un test unitaire
  la compare à la mesure. Les générateurs unitaires mesurent de même la
  profondeur de leurs points d'appel (`depth_here`, `at_depth`) et le Rust
  rejoue chaque cas depuis la même profondeur.
- Une étape par page traite les `--max-pages` premières pages (20 par défaut,
  0 = toutes), créées paresseusement : le générateur `create_pages` n'est plus
  relancé après la dernière page demandée.
- Les étapes `fonts`, `interp` et `layout` mènent les pages comme pdfplumber
  (un `PDFResourceManager` par document, un
  `PDFPageAggregatorWithMarkedContent` et un `PDFPageInterpreter` par page,
  `process_page` ; `architecture.md`, « Vague 3 »). `interp` et `fonts`
  observent les appels par une sous-classe du périphérique (point
  d'extension, pas de rustine), qui n'écrit rien pendant l'interprétation ;
  `layout` n'ajoute aucune surcharge et analyse le `LTPage` brut à la
  profondeur où `end_page` l'analyserait. Une adresse mémoire ne figure dans
  aucune trace : les noms `str(id(obj))` des images en ligne deviennent
  `inline{"n": k}`, et l'étape `layout` écrit le rang des `id()` de son
  analyse (lignes `ids`), que `pdftrace` relit (`ORACLE_INPUT`, ci-dessous).

Les quatre étapes `plumber_*` mènent pdfplumber comme un utilisateur
(`pdfplumber.open`, `oracle/stages/plumber_objects.py` `open_pdf`) ;
`--max-pages` y devient l'option `pages` (`PDF.pages` parcourt tout l'arbre
des pages et ne construit que les pages choisies). Leurs jeux d'options sont
des tables (`oracle/codegen/plumber_ops.py` les écrit pour le Rust) dont
chaque page prend une tranche déterministe (`select`, tirée du SHA-256 du
fichier et du rang de la page) ; un résultat de plus de 65 536 caractères
s'écrit par sa longueur et son empreinte (`outd`). Une exception de
pdfplumber qui en enveloppe une autre s'écrit suivie de ` cause ` et de
celle-ci.

Les modules sans PDF en entrée (filtres, inflate, chiffrement, parseur de
CMap, analyse de mise en page, tables à partir de bords) ont aussi des **tests
différentiels unitaires** : `oracle/unit/<module>.py` génère des cas (graine
fixe) et les sorties Python, Rust les rejoue (`cargo test`).

- Chaque générateur se lance comme module depuis le dépôt dont le Rust rejoue
  les cas (`python -m oracle.unit.<module>`) et écrit, avec les outils de
  `oracle/unit/_common.py`, dans `<cache>/unit/<module>/<empreinte>/`
  (`<cache>` : `PDFPLUMBER_CACHE`, sinon `$PDFPLUMBER_ROOT/.cache`) :
  `<nom>.jsonl`, une ligne `{"in": [...], "out": ...}` au format typé
  ci-dessous, et, en dernier, `meta.json`, dont `sources` donne le SHA-256 de
  chaque module de `oracle/` chargé par le générateur (lui-même, `canon.py`,
  `typed_json.py`…) et `environment` l'environnement de l'oracle
  (`oracle/environment.py`, voir « Outils »). L'empreinte hache les deux :
  des worktrees aux générateurs différents partagent le cache sans jamais
  mélanger leurs fichiers. Les tests Rust
  (`crates/pdftrace/tests/<module>_unit.rs`, via `pdftrace::unit::UnitData`)
  prennent le répertoire le plus récent dont les sources sont celles de leur
  dépôt, et échouent s'il n'y en a pas, en affichant la commande qui
  régénère les données. `python -m oracle.unit` régénère tout ce qui manque
  ou est périmé, environnement compris (`--check` : constat seul).
- Les mêmes tests Rust tournent aussi compilés en WebAssembly
  (`wasm32-wasip1`, exécutés par le WASI de Node via
  `scripts/wasi-runner.mjs`, dans `scripts/check.sh`) : `usize` de 32 bits,
  pas de libm de l'hôte, arithmétique du navigateur.
- Générateurs actuels (tous enregistrés dans `oracle/unit/__main__.py`),
  avec le test Rust qui rejoue chacun (`crates/pdftrace/tests/`) :

  | Générateur (`oracle/unit/`) | Modules Rust vérifiés | Rejeu | Contenu |
  |---|---|---|---|
  | `canon.py` | `pdftrace::{canon, typed_json}` | `canon_unit.rs` | `canon` et JSON typé (ci-dessus), valeurs imbriquées sur 2·10^5 à 10^6 niveaux |
  | `pycompat.py` | `pycompat` | `pycompat_unit.rs` | sémantique CPython, `PyStr` (surrogates isolés, `unicodedata.normalize` NFC/NFKC/NFD/NFKD, `repr`, `json`, UTF-16), `pow` et `atan2` de la libm (`math.atan2`), `hash()` numérique, limites de récursion |
  | `exceptions.py` | `psexceptions`, `pdfexceptions`, `pycompat::err` | `exceptions_unit.rs` | classes d'exception de pdfminer et pdfplumber |
  | `pdftypes.py` | `pdftypes`, `pycompat::specialize` | `pdftypes_unit.rs` | modèle d'objets, références, flux, déchiffrement, `uint_value`, objets imbriqués à la frontière des limites de récursion (`repr`/`str` en unités C, `resolve_all`/`decipher_all` en cadres) ; `deep_first` : `literal_name` comme premier `str(x)` de son site dans le processus (chaque cas dans un fils créé avant toute exécution du site), les autres cas avec le site déjà exécuté (`warm_call_sites`) |
  | `psparser_base.py` | `psparser` | `psparser_base_unit.rs` | `PSStackParser` sur un fichier : `nexttoken`, `nextline`, `revreadlines`, `seek`, `pop` |
  | `psparser.py` | `psparser`, `pdfparser` (`PDFContentParser`) | `psparser_unit.rs` | flux de contenu des pages et Form XObjects de l'ensemble `all` (`corpus.json`), corpus adverse `--fuzz 4000` (`fuzz.json`) suivi de jobs fixes (chaque fragment qui lève, chaque fragment posé sur une frontière de bloc, objets imbriqués autour de la limite de `repr`/`str` et sur 10^6 niveaux) ; comparés job par job par empreinte de la sérialisation canonique, chaque job rejoué depuis la profondeur où pdfminer l'a exécuté |
  | `utils.py` | `utils`, `casting`, `data_structures`, `pdfcolor`, `pdfplumber::ctm` | `utils_unit.rs` | matrices et points (ints, floats, grands ints, infinis, NaN, zéros signés), `get_bound`, `drange`, `bbox2str`/`matrix2str`, `parse_rect`, `uniq`/`fsplit`/`pick`, `nunpack`, `decode_text`, nombres romains et alphabétiques, `safe_*` de `casting`, `Plane`, `NumberTree`, espaces de couleur, `CTM` |
  | `layout.py` | `layout` | `layout_unit.rs` | pages brutes (réelles, par `PDFPageAggregatorWithMarkedContent`, et adverses) reconstruites puis analysées sous plusieurs `LAParams`, depuis la profondeur de Python et avec l'ordre des `id()` de Python ; objets `LT*` (`LTChar` vertical, `LTFigure` d'une Form XObject aux `/Matrix`/`/BBox` quelconques, attributs `mcid`/`tag`), lignes de texte, `list.sort` des clés du layout (NaN compris) |
  | `converter.py` | `converter`, `pdfplumber::page` (`PDFPageAggregatorWithMarkedContent`), constructeurs de `layout`, `pdfdevice::device`, `pycompat::re_cache`, `pdftrace::{layout_dump, font_case}` | `converter_unit.rs` | suites d'appels de l'interpréteur au périphérique de pdfplumber, rejouées sans l'interpréteur sur un `PDFPageAggregatorWithMarkedContent` neuf, chaque appel depuis sa profondeur, sous `laparams=None` puis des `LAParams` (avec l'ordre des `id()` de Python, injecté avant `end_page`), sorties au format de l'étape `layout` (lignes `raw`, `analyzed`/`obj`/`group`, `ids`) ; `adversarial` : toutes les formes de `paint_path` jusqu'à 4 opérateurs et des plus longues, rectangles dans tous les sens et dégénérés, coordonnées NaN/infinies/énormes/entières, figures imbriquées aux `/BBox`/`/Matrix` de tout type, images (en ligne comprises), caractères de polices de toute sorte (`Spec` de `fonts.py`, cids indéfinis ou hors de la limite de `str()`), balises et propriétés de toute sorte, appels hors grammaire ; `depth` : pour chaque sorte d'appel et chaque chemin le plus profond, la frontière de la `RecursionError` (recherche dichotomique) et les cas autour ; `--corpus` : les appels des 20 premières pages de l'ensemble `all` (3 657 pages de 1 853 fichiers, 9 571 exécutions ; 2 pages de plus de 60 000 appels écartées ; le rejeu Python redonne la page de l'interprétation sur toutes), polices copiées comme celles de `fonts.py --corpus` (`corpus.json`) ; appels compactés (colonnes `zlib` des caractères d'une chaîne et des coordonnées d'un chemin, liste longue compressée : 188 Mo) |
  | `zlib.py` | `pycompat::zlib` | `zlib_unit.rs` | `zlib.decompress` et `zlib.decompressobj` de CPython sur zlib 1.3 : flux de compresseurs réels, troncatures, inversions de bits, flux deflate fabriqués aux tables invalides ou de taille maximale (`ENOUGH_LENS`, `ENOUGH_DISTS`), en entier, octet par octet, par morceaux, avec `max_length`, à travers les blocs de sortie de CPython |
  | `filters.py` | `lzw`, `ascii85`, `runlength`, prédicteurs de `utils`, `pdftypes` (`PDFStream.get_data`, `decompress_corrupted`), `ccitt` (par `decode`) | `filters_unit.rs` | LZW, ASCII85, ASCIIHex, RunLength, prédicteurs TIFF/PNG, `decompress_corrupted`, `PDFStream.get_data` sur des chaînes de filtres générées (références indirectes comprises), puis chaque flux de l'ensemble `all` (`corpus.bin`, `corpus.json` ; ~38 300 flux dont ~315 en `/CCITTFaxDecode`, aucun cas écarté) |
  | `ccitt.py` | `ccitt` | `ccitt_unit.rs` | arbres de classe, `BitParser.add`, le parseur mené par ses méthodes internes (cas des tests upstream compris), `ccittfaxdecode` sur des flux G4 générés (encodeur T.6 de test) et adverses, cas profonds près des limites de cadres et d'unités C |
  | `ccitt_corpus.py` | `ccitt` | `ccitt_unit.rs` (`corpus`) | chaque appel de `ccittfaxdecode` que pdfminer fait sur l'ensemble `all` (flux et images en ligne) ; les appels de plus de `CALL_TIMEOUT_S` s en Python sont écartés et listés dans `meta.json` |
  | `crypto.py` | `arcfour`, `saslprep` (et sa NFKC 3.2.0), `utils::unpad_aes`, `pdfdocument::security` | `crypto_unit.rs` | RC4, `unpad_aes`, tables de `stringprep`, NFKC d'Unicode 3.2.0, `saslprep`, gestionnaires de sécurité générés (révisions 2 à 6, `/V` 0 à 6, `/CF`, `/StmF`/`/StrF`, mots de passe utilisateur et propriétaire), PDF chiffrés du corpus, cas profonds |
  | `cmaps.py` | `glyphlist`, `latin_enc`, `fontmetrics`, `encodingdb`, `cmapdb` | `cmaps_unit.rs` | tables, `name2unicode`, `get_encoding`, scripts d'appels sur chaque classe de `cmapdb`, toutes les CMaps prédéfinies lues par le fournisseur des fichiers compacts de `oracle/codegen/cmaps.py`, `CMapParser` sur un corpus adverse `--fuzz 3000` (`synthetic.json`) et, avec `--corpus`, sur chaque flux CMap embarqué de l'ensemble `all` (`corpus.json`) |
  | `fonts.py` | `pdffont` (`mod.rs`, `type1.rs`, `cff.rs`, `truetype.rs`, `frames.rs`, `unpack.rs`), cadres de `cmapdb` (`CMapDB`, `CMapParser`) | `fonts_unit.rs` | `get_widths`/`get_widths2` sur des tableaux générés (références manquantes ou cassées comprises), `getdict`, `CFFFont`, `TrueTypeFont` + `create_unicode_map` et `Type1FontHeaderParser` sur des programmes générés et mutés, dictionnaires de police générés passés par `PDFResourceManager().get_font(None, spec)` (toutes les classes, chaînes `Type0`, chaque CMap prédéfinie) avec attributs, `decode`, `to_unichr`, `char_width`, `char_disp`, `string_width` ; `depth` : pour chaque police et chaque requête, la profondeur limite (nombre de cadres en plus) mesurée par recherche dichotomique, rejouée au seuil et un cadre plus bas, police construite une fois avant (CMaps prédéfinies chargées) et, pour celles qui en chargent, caches de `CMapDB` vidés avant chaque construction (`get_font_cold` : premiers chargements) ou après la création d'un `PSStackParser` (`get_font@parsed` : le cache du logger de `psparser` rempli, comme par le parseur d'un document) ; `/ToUnicode` avec `usecmap` ; polices de documents chiffrés (`fonts` et `depth`) : chaque flux chiffré pour un gestionnaire de sécurité de chaque sorte (RC4 40 et 128 bits, V4 RC4/AES-128/identité, AES-256, clé refusée par AES), le cas portant `[docid, param, password]` et le rejeu déchiffrant par le gestionnaire porté ; avec `--corpus`, chaque police utilisée par les 20 premières pages de l'ensemble `all` (objets atteints, erreurs de `getobj`, chaînes affichées ; pour un document chiffré, copiés d'une seconde instance, flux encore chiffrés) et chaque programme embarqué (`corpus_fonts`, `corpus_programs`), et les frontières de récursion des premières polices de chaque document chiffré (`corpus_depth`) |
  | `text_tables.py` | `pdfplumber::{obj, utils::{clustering, geometry, generic, text}, table}`, `pycompat::listsort` | `text_tables_unit.rs` | tri de CPython (timsort complet), opérateurs Python du modèle d'objets, `cluster_*`, géométrie, motifs de `extract_text_lines`, fonctions de `table.py`, `TableSettings.resolve`, pages adverses (`--fuzz`) passées par les opérations de page (`_plumber_ops.py`), pages aux coordonnées NaN (`_nan_objects.py`, ci-dessous), dont des NaN qu'objets et arêtes partagent, pages aux caractères surrogates isolés, arguments de l'utilisateur de tout type (`extra_attrs`, `split_at_punctuation`, lignes explicites, `text_settings`, chaînes en coordonnées) |
  | `text_tables_real.py` | les mêmes | `text_tables_unit.rs` (`pages_real`) | opérations de page sur les premières pages de l'ensemble `quick`, sur des documents riches en tables et sur `corpus/micro/lone-surrogates.pdf` du dépôt (caractères dont le texte est un surrogate isolé : `IdentityUnicodeMap`, `bfrange` de nombres) |
  | `document.py` | `pdfparser`, `pdfdocument` (xrefs, `getobj`, flux d'objets, mot de passe, plans, libellés, noms), `pdfpage`, `pdfplumber::page` (boîtes), `pdfinternals.resolve_all`, `pycompat::bytes_format`, le comptage des cadres (`psparser`, accesseurs, filtres, `pycompat::logging`) | `document_unit.rs` | traces des étapes `objects`, `streams`, `pages`, `psparser` sur `adv-objects` et `adv-pages` (régénérés d'abord ; fenêtres de profondeur autour de la limite des cadres comprises), `micro` et les petits fichiers de `quick` ; `objects` sur un `io.BytesIO` (erreurs de `seek`/`read` d'un `BytesIO`) ; `api` (`get_outlines`, `get_page_labels`, `lookup_name`, `get_dest`, `PDFPage.get_pages`, `repr` des pages) ; chaque trace dans un processus fils créé pour elle (`os.fork`), comme `verify` en lance un par trace, et `py_process.jsonl` (les noms internés au départ des fils), dont le rejeu repart ; `bytes % int` (formats générés, et les largeurs qu'aucune machine ne peut allouer : `MemoryError`) ; `PDFXRefFallback.PDFOBJ_CUE` sur chaque caractère Latin-1 |
  | `interp.py` | `pdfinterp` (tout le module), `pdfdevice`, et par les étapes `interp`, `fonts`, `layout` : `converter`, `pdfplumber::page`, l'analyse du layout, `pdftrace::stages` | `interp_unit.rs` | traces des étapes `interp`, `fonts`, `layout` (la trace Python donnée en entrée de l'oracle : l'ordre des `id()`), `interp_text` et `fonts_text` sur `adv-interp` (régénéré d'abord, fenêtres de profondeur comprises), `micro` et les petits fichiers de `quick`, chaque trace dans un processus fils (`py_process.jsonl`), CMaps prédéfinies du `cmap_dir` de `meta.json` |
  | `interp_deep.py` | les cadres des chemins de l'interpréteur vers le périphérique (`pdfinterp`, `pdfdevice`) | `interp_unit.rs` (`deep_sweeps`) | `interp_text` avec `DeepTracingDevice` (chaque méthode du périphérique descend d'abord `DESCENT` = 60 cadres : chaque appel au périphérique devient plus profond que tout ce qui le précède sur sa page) sur chaque fichier de `adv-interp` hors chaînes `depth-*`, l'interprétation de chaque page (`page_layout`) descendue de 17 profondeurs successives (`down`) qui font passer la limite de récursion d'un cadre au-dessus du point le plus profond des pages du fichier (mesuré par un crochet de profil borné) à 15 cadres plus haut ; puis, sans descente (le périphérique de l'étape), de chaque profondeur entre ce point et le cadre de `page_layout` (ce que la page fait avant son premier appel : `begin_page`, `init_resources`, polices, espaces de couleur, `/ProcSet`, `init_state`) ; le document est ouvert et les lignes écrites à la profondeur de l'étape ; la première trace complète, les autres par leur SHA-256. `api.jsonl` (`resource_manager_get_cmap`) : `PDFResourceManager.get_cmap(name, strict)`, qu'aucune page n'appelle, sur des CMaps identité, prédéfinies, des `UnicodeMap` et des noms absents, et depuis chaque profondeur à 30 cadres de la limite |
  | `structure.py` | `pdfplumber::structure`, les métadonnées de `pdfplumber::pdf` (`read_metadata`), `Page.annots`/`hyperlinks`/`structure_tree`, `utils::pdfinternals` (`decode_text`, `resolve_and_decode`), `pycompat::codecs`, `pdftrace::stages::plumber_doc` | `structure_unit.rs` | traces de l'étape `plumber_doc` sur `adv-structure` (régénéré d'abord ; fenêtres de profondeur aussi depuis un et deux cadres plus bas) et sur les fichiers de `quick` qui ont un arbre de structure, des métadonnées imbriquées ou des annotations ; `plumber_objects` sur les cas d'annotations ; `bytes.decode("utf-8")` et `("utf-16")` de suites d'octets générées (`codecs`) |
  | `js_runtime.py` | `crates/wasm` (`errors`, `cmaps`, `ops`, `wire`) | `crates/wasm/tests/js_runtime_unit.rs` (et les tests JS de `js/test/shared/`, Node et navigateurs) | MRO de chaque classe d'exception, CMaps prédéfinies par le protocole paresseux, texte, mots, `dedupe_chars`, `TextMap.search`/lignes, tables sur des objets de pages réelles ; documents CJK pour la couche page |
  | `plumber.py` | `pdfplumber::{pdf, page, container, convert, utils::pdfinternals}` (et `structure` par `plumber_doc`), `pycompat::{json, csv}`, `pdftrace::stages::plumber_*` | `plumber_unit.rs` | traces des étapes `plumber_objects`, `plumber_text`, `plumber_tables`, `plumber_doc` sur `adv-plumber` (régénéré d'abord, fenêtres de profondeur comprises, aussi depuis un et deux cadres plus bas), `micro` et les petits fichiers de `quick`, chaque trace dans un processus fils (`py_process.jsonl`), les étapes `ORACLE_INPUT` recevant leur trace Python ; `open.jsonl` : `pdfplumber.open` avec des `laparams` (clés inconnues, `boxes_flow` de tout type et hors domaine, marges entières, booléennes, infinies, NaN), `pages` (listes, tuples, vides, flottants, booléens, `str`, entiers), `password`, `strict_metadata`, `unicode_norm`, `raise_unicode_errors` sur deux petits documents : `doctop`, objets et rang des `id()` de l'analyse |

  Les tables générées (`oracle/codegen/`) sont vérifiées par `--check` dans
  `scripts/check.sh` : `libm_tables.py`, `atan2_tables.py`, `unicodedata_tables.py`, `saslprep_tables.py`,
  `pdfminer_tables.py` (`glyphlist`, `latin_enc`, `fontmetrics`), `pdffont_tables.py` (`NIBBLES`, `CFFFont.STANDARD_STRINGS`),
  `ccitt_tables.py`, et les tables d'options des étapes `plumber_*`
  (`plumber_ops.py` → `crates/pdftrace/src/stages/plumber_ops.rs`). Les
  CMaps prédéfinies converties par
  `oracle/codegen/cmaps.py` ne sont pas versionnées : elles vivent dans
  `<cache>/cmap/<empreinte>/`, que le générateur `cmaps.py` remplit et
  désigne au rejeu (`cmap_dir` de `meta.json`).
- Un résultat Python qui dépend d'adresses mémoire (l'ordre d'un `set` de
  NaN : `hash(nan)` est l'adresse de l'objet) n'est jamais comparé tel quel.
  Le port fixe un ordre que CPython peut donner ; le générateur fabrique des
  objets dont le hachage donne cet ordre et vérifie, en observant chaque
  `set` construit (rejeu de `setobject.c`, pour plusieurs valeurs des bits
  d'adresse non choisis), que CPython l'a bien donné ; sinon le cas est
  écarté, de façon déterministe (`oracle/unit/_nan_objects.py`).

```sh
export PDFPLUMBER_ROOT=/chemin/du/dépôt/principal   # .venv, corpus/, .cache/
PYTHONPATH=. $PDFPLUMBER_ROOT/.venv/bin/python -m oracle.unit   # ce qui manque ou a changé (corpus réel : ~3 min)
cargo test --release
scripts/check.sh                                               # tout : fmt, clippy, données, tests Rust et Python
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
  étape non implémentée. `pdftrace --list-stages` liste les étapes, suivies de
  `(not implemented)` tant qu'elles ne le sont pas.
- `pdftrace --oracle-dir DIR` : pour une étape dont le module Python
  déclare `ORACLE_INPUT` (`layout`), la trace de l'oracle de chaque fichier
  (`DIR/<sha256>.txt`, ou `.pw-<12 hexa>.txt`, nommée comme le cache), que le
  Rust lit (`StageInput::oracle`) ; `verify` la passe toujours pour ces
  étapes, et les autres étapes la refusent. Sans elle, `layout` analyse
  avec l'ordre par défaut de la bibliothèque (`CreationOrder`). C'est le
  cas de `scripts/wasm-depth-gate.sh`, qui compare le Wasm au natif sans
  cache de l'oracle : ses traces `layout` ne sont celles que `verify`
  compare à Python que parce que ses fenêtres n'ont aucune page dont
  l'analyse dépend du rang des `id()` (mesuré à la finalisation de la
  vague 3 : les 188 fichiers `depth-*` et `climit-*` d'`adv-interp` donnent
  la même trace `layout` native avec et sans `--oracle-dir`) ; la porte
  vérifie la pile, pas la parité.
- `pdftrace --cmap-dir DIR` (par défaut `$PDFPLUMBER_CMAP_DIR`) : le
  fournisseur des CMaps prédéfinies (`<cache>/cmap/<empreinte>/`, le
  `cmap_dir` des données unitaires `cmaps` ou `interp`). Les étapes qui
  interprètent des pages (`fonts`, `interp`, `layout`, `fonts_text`,
  `interp_text`, `plumber_*`) en ont besoin : sans lui, une police à CMap
  prédéfinie donne `RuntimeError("no CMap provider installed …")` (8
  fichiers d'`adv-interp`, `*-font-cmap-*` et
  `*-depth-font-predefined-cmap-*`). `oracle.verify` le passe donc
  toujours (`--cmap-dir`, par défaut le répertoire de
  `oracle/codegen/cmaps.py` sous le cache, écrit au besoin ; un répertoire
  sans `names.txt` arrête `verify` avant le cache de l'oracle, code 2).
- Les deux outils acceptent aussi `--option=valeur` ; `verify` et le cache
  passent toujours le mot de passe sous la forme `--password=P` (un mot de
  passe peut commencer par `-`). Avec `--out`, `pdftrace` ajoute après
  chaque fichier la ligne `<sha256>\t<secondes>` à `DIR/_times.tsv`.
- `python -m oracle.verify --stage S --set NOM [-j N] [--max-pages N]
  [--pdftrace CHEMIN] [--cmap-dir DIR] [--limit K] [--files …] [--json]` : construit au besoin
  le cache de l'oracle (`-j` processus, 3 par défaut), lance `pdftrace` par lots
  de fichiers de même mot de passe, compare octet pour octet, puis affiche le
  décompte (identiques / différents / plantages Rust / délais Rust / délais
  oracle / erreurs oracle / total) et, pour chaque fichier en écart, son chemin
  et la première ligne différente avec 3 lignes de contexte de chaque côté.
  La limite `--timeout` vaut par fichier des deux côtés : un lot est tué
  quand aucun fichier ne s'achève pendant `--timeout` secondes, et un fichier
  dont le temps (`_times.tsv`) la dépasse est un délai Rust, même si sa trace
  est juste. Un lot qui échoue garde les traces qu'il a achevées et rejoue
  chacun des autres fichiers seul pour isoler le coupable. `--json` donne un
  résumé lisible par machine ; `--oracle-only` ne fait que remplir le cache.
  Code de sortie : 0 seulement si au moins un fichier a été comparé et que
  tout est identique ; 1 sinon (écart, plantage ou délai Rust, erreur oracle,
  délai oracle sauf avec `--allow-timeouts`, rien de comparé) ; 2 si l'étape
  n'existe pas côté Rust, si le binaire manque (vérifié par
  `pdftrace --list-stages` avant de construire le cache de l'oracle) ou si
  l'ensemble ne peut pas être listé (nom inconnu, `adv-<étape>` jamais
  généré, répertoire absent : un ensemble vide ne passe jamais).
- Cache : `.cache/oracle/<étape>/<empreinte>/<sha256 du PDF>.txt`
  (`<sha256>.pw-<12 hexa du SHA-256 du mot de passe>.txt` avec un mot de
  passe). L'empreinte couvre les sources de l'étape (et des étapes de son
  `DEPENDS`), de `stages/__init__.py`, `stages/_common.py`, `canon.py`,
  `typed_json.py`, `trace.py` et `environment.py`, l'environnement de
  l'oracle (`oracle/environment.py` : interpréteur, versions de tous les
  paquets installés, SHA-256 des fichiers installés de pdfminer et pdfplumber,
  zlib, bibliothèque C, drapeaux du processeur qui choisissent son `pow`) et
  `--max-pages` pour une étape par page : tout changement invalide le cache.
  Écriture atomique (fichier temporaire puis renommage). Chaque trace est
  calculée dans son propre processus ; au-delà de 120 s (`--timeout`), le
  fichier reçoit la trace `!Timeout <limite>`. Un délai n'est jamais un
  succès : `verify` le compte à part et échoue (sauf `--allow-timeouts`, qui
  le liste), et le recalcule de lui-même dès qu'il dispose d'une limite plus
  grande (`--retry-timeouts` force le recalcul).
- Racine du corpus : la variable `PDFPLUMBER_ROOT` (par défaut la racine du
  dépôt) donne l'emplacement de `corpus/`, `upstream/` et `.cache`
  (`PDFPLUMBER_CACHE` peut désigner un autre cache). Dans un worktree Git, elle
  pointe vers le dépôt principal, qui contient les PDF téléchargés et le venv.

### Mesurer une profondeur de cadres

Le port modélise par des constantes les cadres Python que des appels de la
bibliothèque standard prennent quand ils ne dépendent pas de l'entrée
(`_load_data` de `cmapdb`). `oracle/depth_probe.py` les mesure : un crochet
de profil (`sys.setprofile`) ne garde que deux entiers, profondeur courante
et maximale (`max_depth`), et ne copie les noms de la pile qu'à chaque
nouveau maximum (`deepest_path`). Ne jamais copier la pile à chaque appel :
une sonde qui le faisait a atteint 18 Go et gelé la machine. Toute sonde
tourne sous limites explicites ; `prlimit` prend des octets (`--as=4G`
pose une limite de 4 octets et tout plante au démarrage) :

```sh
timeout 15m prlimit --as=4294967296 -- $PDFPLUMBER_ROOT/.venv/bin/python -m oracle.depth_probe cmaps
```

Les constantes mesurées sont aussi vérifiées par les cas de l'ensemble
`depth` de `oracle/unit/fonts.py` (premiers chargements, `get_font_cold`).

## Ensembles de PDF (`oracle/corpus.py`)

| Nom | Contenu |
|---|---|
| `micro` | `corpus/micro` (générés, petits, chacun vise une fonctionnalité) |
| `upstream` | `upstream/pdfplumber-python/tests/pdfs`, `upstream/pdfminer-six/samples`, `corpus/upstream-contract` |
| `real` | `corpus/external`, `corpus/external-holdout`, `corpus/parity-cycles` |
| `pdfjs` | `upstream/pdfjs/test/pdfs` (937 PDF, beaucoup de cas tordus) |
| `quick` | tout `micro`, plus les fichiers de `all` dont le SHA-256 commence par un octet < 18 (~7 % ; 150 PDF en tout aujourd'hui) : l'appartenance ne dépend que du contenu du fichier, en ajouter n'en retire aucun |
| `all` | tout ce qui précède (1 853 PDF distincts aujourd'hui) |
| `adv-<étape>` | corpus adverse généré pour une étape (voir plus bas ; la vague 4 étend le nom à un générateur que plusieurs étapes vérifient, `adv-plumber`, `adv-structure` : `architecture.md`, « Vague 4 ») ; plusieurs étapes peuvent vérifier le même ; aujourd'hui `adv-objects` (333 PDF), `adv-pages` (285) et `adv-interp` (353), sur lesquels chaque étape implémentée est vérifiée à l'intégration d'une vague (`interp_text` et `fonts_text` : `adv-interp`) |

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

Générateurs actuels (chacun s'exécute comme module, `python -m
oracle.adversarial.objects` ; `oracle/unit/document.py` les relance avant
d'écrire ses cas) :

- `oracle/adversarial/objects.py` → `adv-objects` (333 PDF) : tables xref
  cassées ou absentes (repli `PDFXRefFallback` et son motif d'en-tête),
  offsets faux, `startxref` invalide, flux xref aux `/W`, `/Index`,
  prédicteurs et champs atypiques (flottants, noms, chaînes, négatifs),
  flux d'objets aux `/N`, `/First`, index et références imbriquées
  anormaux (ids flottants, booléens, noms, références dans le repli),
  chaînes et boucles `/Prev`/`/XRefStm`, mises à jour incrémentales,
  trailer ou `/Root` absents, déchets avant `%PDF` et après `%%EOF`, fins
  de ligne `\r` seules et `\r\n`, `/Length` indirects, négatifs, énormes,
  dans un tableau non fermé ou égaux au flux lui-même (`RecursionError`),
  en-têtes d'objets discordants, frontières de blocs de 4 096 octets, et
  chaque chiffrement standard (RC4 40/56/128, AES-128, filtres Identity et
  V2, AES-256 révisions 5 et 6) avec le mot de passe utilisateur,
  propriétaire, faux ou vide (mots de passe dans `manifest.json`) ;
  `cases_depth` : chaînes `/Prev` (tables, flux xref, flux en bas) et de
  `/Length` de part et d'autre du premier niveau qui échoue en Python,
  `obj`/`endobj` dans un flux d'objets, le mot-clé `stream` en fin de
  fichier, `/Length` autour de `PY_SSIZE_T_MAX - 33` (`MemoryError`,
  `OverflowError`), un champ de flux xref de 100 000 octets, un id flottant
  déchiffré (`struct.error`) et un id booléen (`True`, un entier pour
  `struct.pack`).
- `oracle/adversarial/interp.py` → `adv-interp` (353 PDF ; vérifié par `interp_text`,
  `fonts_text`, `interp`, `fonts` et `layout`) : chaque opérateur de
  l'interpréteur sans opérande, avec un de moins, un de trop, et chaque
  position recevant chaque sorte de valeur du parseur de contenu (entiers,
  grands entiers, flottants infinis, booléens, `null`, noms, chaînes,
  chaînes numériques `(nan)`/`(1e400)` que `float()` lit, tableaux,
  dicts), mots-clés écrits comme les méthodes (`T_a`, `_q`) ou décodés vers
  un opérateur (`q\x80`), chaque opérateur suivi de `J` sans opérande (qui prend ce que l'opérateur a laissé sur la pile), `/ProcSet` par référence, chaque `/Rotate` ; `Tz`/`TL`/`Ts`/`Tc`/`Tw` extrêmes, `TJ` sur
  tout objet (nombre, chaîne, nom, dict, flux d'image en ligne `~>`),
  espacement des mots mono- et multi-octets, `'` et `"` ; polices Type 3
  aux `/FontMatrix` atypiques, `Type0` verticales (`/W2`, `/DW2`), CMap
  prédéfinie ou inconnue, polices absentes ou mal typées, cache par
  `objid` ; chaque espace de couleur (`ICCBased` et son `/N` de tout type,
  `DeviceN`, `Indexed`, `Separation`, `Pattern`, tableau vide), `scn` de 0
  à 5 composantes (et les opérandes que `scn`/`SCN` laissent sur la pile, pris par `J`/`j` ; les espaces qui lèvent dans `init_resources` à part), motifs colorés et non colorés ; chemins et peintures,
  `cm` dégénérés, `q`/`Q` déséquilibrés ; images en ligne aux frontières
  de blocs de 4 096 octets, coupées entre deux flux, mal formées ;
  `BMC`/`BDC`/`EMC`/`MP`/`DP` déséquilibrés et toutes sortes de
  propriétés ; Form XObjects aux `/BBox`, `/Matrix`, `/Resources`
  atypiques, cycles, tableaux de contenus aux membres cassés ;
  `cases_depth` : chaînes de Form XObjects autour du premier niveau qui
  échoue, cherché par dichotomie pour `interp_text`, `interp`, `fonts` et
  `layout` (le périphérique de pdfplumber sans les surcharges de trace),
  avec 28 chemins différents au point le plus profond
  (`docs/architecture.md`, « Vague 3 » ; dont `subpaths-first`, où le
  `paint_path` de pdfplumber et la première compilation du motif de
  `re.finditer` sont le point le plus profond, et `subpaths-compiled`, le
  motif compilé par la page) ; `cases_climit` (`climit-*`) : opérandes
  imbriqués autour de la limite des unités C, cherchée par dichotomie pour
  les mêmes étapes : `str()` d'un opérande de `Tf` (tableau,
  dictionnaire), `Do`, `CS`, `cs` et d'une ressource `/ColorSpace`
  (`literal_name`, dont le premier `str(x)` d'un processus prend une unité
  de plus ; variantes à deux pages, la seconde exécutant l'appel
  spécialisé), `repr()` des entrées impaires d'une image en ligne et d'un
  dictionnaire au bas de chaînes de 0, 1, 50 et 300 Form XObjects (deux
  unités par niveau, celles de `func(*args)`).
- `oracle/adversarial/plumber.py` → `adv-plumber` (vérifié par les quatre étapes `plumber_*`) : la couche page de pdfplumber, cas listés en tête du générateur (`architecture.md`, « Vague 4 », « Réalisation » du chantier PAGE) ; `cases_depth` : chaînes de Form XObjects (l'interpréteur puis `iter_layout_objects`), `/ColorSpace` d'image et `/MCID` imbriqués en tableaux ou par chaînes de références (`resolve_all`), tirets imbriqués (`Serializer` de `to_json`/`to_csv`, `plumber_doc`). Pas de NaN dans les clés que `cluster_objects` met dans un `set` (leur ordre suit leur adresse en CPython).
- `oracle/adversarial/pages.py` → `adv-pages` (285 PDF) : héritage (`/Resources`,
  `/MediaBox`, `/CropBox`, `/Rotate`, depuis les nœuds et le catalogue),
  boucles et partages dans l'arbre, `/Kids` absents ou atypiques (entiers,
  dicts, flux, noms), repli sans `/Pages`, arbres profonds jusqu'à la
  limite de récursion, toutes sortes de `/Rotate` (dont `bytes % 360`), de
  boîtes et de `/Contents`, cycles de références (`MalformedPDFException`
  de pdfplumber), libellés de pages de tous les styles, plans, arbres de
  noms et destinations ; `cases_depth` : pour chaque forme de récursion
  (arbre de pages, avec chaque chemin le plus profond possible au dernier
  niveau : journalisation, flux, nom interné pour la première fois, xref
  de repli, chiffrement, un flux d'objets par nœud avec chaque filtre ;
  arbre de nombres des libellés ; `resolve_all` de pdfplumber ; plans et
  arbre de noms par `api_trace`), une fenêtre de longueurs autour du
  premier niveau qui échoue en Python (`docs/architecture.md`,
  « Profondeur de récursion »).
- `oracle/adversarial/structure.py` → `adv-structure` (441 PDF ; vérifié
  par `plumber_doc` et `plumber_objects`, rejoué par
  `oracle/unit/structure.py`) : arbres de structure (`/StructTreeRoot`
  absent ou de tout type, `/K` de toute sorte et imbrication, MCR et OBJR
  vers une annotation, un élément, une autre page, DAG, cycles de la
  portée document, `/Pg` absents, atypiques ou hors des pages choisies,
  `/RoleMap`, `/ClassMap`, attributs `/A`/`/C` en tableaux avec
  révisions, références dans les attributs, `/BBox`, MCID absents ou
  dupliqués, textes `/ID`, `/T`, `/Lang`, `/Alt`, `/ActualText` de tout
  type), arbres des parents (absent, sans `/StructParents`, imbriqués,
  avec `null`, parents qui n'atteignent pas la racine), métadonnées
  (`/Info` de tout type, partagées, imbriquées, qui se désignent
  elles-mêmes, désignent le catalogue, une page, un élément, deux `/Info`),
  annotations (`/Rect`, `/A`, `/URI`, `/T`, `/Contents` de tout type,
  chaînes non décodables, flux, pages tournées) ; `cases_depth` : seize
  fenêtres autour du premier niveau qui échoue en Python (`--probe`, par
  dichotomie sur l'étape) : `to_dict`, `prune`, `repr`, unités C de
  `repr(ref)`, de `str(s)` (première exécution du site et suivantes) et du
  `repr` d'un attribut, `resolve_and_decode`, arbre des nombres des
  parents (`docs/architecture.md`, « Vague 4 », chantier DOC). Pas de
  cycle par l'arbre des parents d'une page : Python y boucle sans fin.

## Mutations

`cargo mutants` (crate `cargo-mutants`) mute le code Rust d'un module
(`<` en `<=`, constante changée, branche supprimée…) et relance les tests de
parité. **Toute mutation survivante** est soit tuée par un nouveau cas
adverse, soit justifiée par écrit (mutation équivalente, avec preuve). La
commande et le résultat sont notés dans le rapport du module.

Une mutation peut faire boucler le code sans consommer son entrée, souvent en
allouant : sur la machine partagée, un mutant a atteint 16,5 Go. D'où trois
règles :

- **Plafond mémoire et créneaux** : chaque `cargo mutants`, génération de
  corpus adverse ou passe sur un ensemble complet passe par
  `scripts/heavy.sh <commande>`. Le script attend l'un des `HEAVY_SLOTS`
  créneaux partagés par tous les worktrees (`.cache/locks/` ; exporter
  `HEAVY_SLOTS=1` : depuis l'incident d'octobre 2026, les travaux lourds
  passent un à un sur la machine), puis lance la commande
  dans `systemd-run --user --scope --quiet -p MemoryMax=5G -p MemorySwapMax=0
  -p OOMPolicy=continue`. Pour `cargo mutants`, utiliser `-j 1 --timeout 20`.
  Les tests corrects restent sous 250 Mo. `OOMPolicy=continue` : le noyau
  tue le processus qui dépasse le plafond (le binaire de test d'un mutant,
  donc un mutant détecté) et la commande continue ; avec la politique par
  défaut (`stop`), systemd arrête tout le scope et `cargo mutants` s'arrête
  sur « interrupted » au milieu de sa passe.
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
- **Autres gardes du profil `mutants`** (à exclure de la même façon) :
  `OutputGuard` de `pycompat::zlib` (un appel d'inflate n'écrit pas plus que
  `avail_out`, ni hors de proportion avec l'entrée : `-E OutputGuard`), le
  rapport sortie/entrée des prédicteurs (`utils.rs`), `check_growth` de
  `cmapdb.rs` (`-E check_growth`), les `debug_assert!` « progress guard »
  des boucles de `utils/text.rs` et `table.rs`.

Les tests à donner à `cargo mutants` pour un module sont ses rejeux
(colonne « Rejeu » du tableau des générateurs), en arguments de `cargo
test` après `--` : `-- --package=pdftrace --test=<rejeu>` (cargo-mutants
27.1 lance `cargo test --package=<crate mutée> -- …` : les formes
`--test-package pdftrace --cargo-test-arg=--test=<rejeu>` et
`--test-workspace=true` ne changent pas le paquet testé, la référence
échoue sur « cargo test failed in an unmutated tree » et rien n'est
testé ; vérifier dans `mutants.out/log/baseline.log` que la commande
contient `--package=pdftrace --test=<rejeu>`). Plusieurs `--test` au
besoin, par exemple `utils_unit` et `layout_unit` pour
`utils.rs`, `filters_unit` et `zlib_unit` pour `pycompat::zlib`), et pour
les fichiers partagés tous les rejeux qui les exercent (`utils.rs` :
`utils_unit`, `layout_unit`, `filters_unit`, `crypto_unit` ; `pdftypes.rs` :
`pdftypes_unit`, `filters_unit`, `document_unit` ; `pdfparser.rs`,
`pdfdocument/*.rs`, `pdfpage.rs`, `pdfplumber/src/page.rs`,
`utils/pdfinternals.rs`, `pycompat/src/bytes_format.rs` : `document_unit` ;
`pdfinterp.rs` : `interp_unit`, `psparser_unit`, `document_unit` ;
`pdfdevice.rs` : `interp_unit` ; `converter.rs`, `pdfplumber/src/page.rs` :
`converter_unit` et `interp_unit` ; `pdftrace/src/stages/{interp,fonts,
layout,common}.rs` : `interp_unit`, `lazy_cmap` ; `pycompat/src/specialize.rs`
et le site de `literal_name` : `pdftypes_unit`, `psparser_unit`,
`cmaps_unit`, `interp_unit` ; `pdfminer/src/process_state.rs` et les
fonctions d'instantané qu'il appelle, `wasm/src/ops.rs` : `lazy_cmap`,
`js_runtime_unit` ; `pdftrace/src/stages/py_process.rs` : `interp_unit`,
`document_unit`, `lazy_cmap`).

```sh
HEAVY_SLOTS=1 scripts/heavy.sh cargo mutants -f crates/pdfminer/src/psparser.rs \
  -E "ProgressGuard|check_progress|read_position" \
  --profile mutants -j 1 --timeout 20 -- --package=pdftrace --test=psparser_unit
```

Pour un fichier dont seule une partie a changé (le comptage des cadres
ajouté à `psparser.rs`, `pdftypes.rs`, aux filtres), `--in-diff
<diff>` limite la passe aux lignes du diff.

**Passe `Ok(..)` des fonctions qui renvoient `PyResult`.** cargo-mutants
(27.1) ne reconnaît un `Result` qu'à son nom : pour l'alias
`pycompat::PyResult<T>`, il remplace le corps par `PyResult::new(..)` ou
`PyResult::from(..)`, qui ne compilent jamais (« unviable »), et ne tente
jamais `Ok(..)`. Le remplacement d'un corps entier par un résultat constant,
la mutation la plus forte, manque donc à la passe ordinaire. Ces mutants
« unviable » ne comptent pas comme testés : chaque module lance aussi
`scripts/mutants-ok.sh`, qui copie le checkout, réécrit `PyResult<T>` en
`Result<T, pycompat::PyErr>` dans les fichiers donnés par `-f`, et ne lance
que les mutants `with Ok(` ; le rapport du module note les deux résultats.
Dans cette passe aussi, `Ok(Default::default())` est « unviable » quand le
type n'implémente pas `Default` (`PDFObject`, les enums du port) : chacun de
ces corps est alors remplacé à la main, sur une copie, par une valeur concrète
de chaque variante pertinente (`Ok(PDFObject::Null)`, `Ok(Some(Variante))`…),
et le test doit échouer (`test result: FAILED`, pas une erreur de
compilation).

```sh
scripts/heavy.sh scripts/mutants-ok.sh -f crates/pdfminer/src/arcfour.rs \
  --profile mutants -j 1 --timeout 60 -- --package=pdftrace --test=crypto_unit
```

Dans le crate `pycompat` lui-même, la réécriture donne `Result<T,
crate::PyErr>`. Avec `--in-diff`, le diff doit venir des fichiers réécrits
(la même substitution `perl` appliquée à la version de base et à la
version courante, puis `diff -u a/<fichier> b/<fichier>`) : un diff du
dépôt ne correspond plus à la copie que voit cargo mutants.

## Définition de « fini » pour un module

1. Port fidèle : un module Rust par module Python, références upstream sur
   chaque fonction, aucun cas spécial.
2. Parité sur `all` (20 premières pages) pour ses étapes : 100 % identique, hors
   `!Timeout` de l'oracle (listés, `--allow-timeouts`).
3. Corpus adverse de l'étape : 100 % identique.
4. Mutations : aucune survivante non justifiée.

La version Wasm est vérifiée par les mêmes rejeux unitaires compilés pour
`wasm32-wasip1` (`scripts/check.sh`), par `scripts/wasm-depth-gate.sh` (le
profil `dist` sur les fenêtres de profondeur des corpus adverses, comparé au
natif) et, à l'intégration d'une vague, par `pdftrace` compilé en Wasm
(`dist`, sous le WASI de Node) comparé au natif sur `quick` pour les étapes
de la vague : `scripts/heavy.sh .venv/bin/python -m oracle.wasm_parity
--set quick [--stages interp,fonts,layout]` (`oracle/wasm_parity.py`, les
deux binaires construits d'abord). Les deux binaires reçoivent ce que
`oracle.verify` donne au natif, si bien qu'une trace identique est la
parité avec Python du Wasm partout où `verify` trouve le natif identique :
le fournisseur de CMaps passé explicitement (`--cmap-dir`, quel que soit
l'environnement : sans lui, les deux binaires s'accorderaient sur une trace
« no CMap provider » pour les fichiers à CMap prédéfinie, 3 des 150 de
`quick`), et pour `layout` le cache de l'oracle (`--oracle-dir`, le rang
des `id()` de Python ; fichiers dont l'oracle n'est pas `ok` écartés comme
dans `verify`). Un fichier par processus, la sortie Wasm redirigée vers un
fichier (le WASI de Node écrit un tube sans bloquer et une longue trace y
échoue par `EAGAIN`). Vague 3 (finalisation) : `interp`, `fonts`, `layout`, 150 fichiers chacune, identiques.
`oracle.verify` ne mène pas le binaire Wasm lui-même : `--out` nomme ses
fichiers temporaires par le numéro de processus, que WASI n'a pas.
