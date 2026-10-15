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
- **Avertissements** (`logging`, `warnings.warn`) : sans effet sur la sortie ;
  ils remontent par un canal de diagnostics (option `onWarning` côté JS).
- **Réglages** : `pdfminer.settings.STRICT = False`, comme l'oracle.

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
le navigateur), et seuls les documents qui en ont besoin les chargent.

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
