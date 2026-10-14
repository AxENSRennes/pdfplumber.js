# psparser : TS vs Rust/Wasm

Expérience de décision : faut-il écrire le cœur du port en TypeScript ou en Rust
compilé en Wasm ? On porte le point chaud de pdfminer (`psparser.py` +
`PDFContentParser`, 25 à 48 % du temps de pdfplumber selon le profilage) dans les
deux langages, avec la même structure, puis on mesure en comptant le coût de
renvoi des résultats au JS.

## Parité (oracle : pdfminer.six réel, via `.venv`)

Chaque job est un ensemble de flux de contenu décodés. Pour chaque job, Python
écrit le SHA-256 d'une sérialisation canonique de tous les `(pos, obj)` produits
par `PDFContentParser.nextobject()`, y compris l'exception finale.

| Corpus | Jobs | TS | Rust/Wasm |
|---|---|---|---|
| Réel : 1 072 PDF (corpus du repo + suite de tests pdf.js), 20 premières pages, pages + Form XObjects, 63,8 Mo | 3 459 | 3 459/3 459 identiques | 3 459/3 459 identiques |
| Adverse généré : tokens à cheval sur les blocs de 4 096 octets et les frontières de flux, échappements, images inline, 5 types d'exception Python | 4 000 | 4 000/4 000 identiques | 4 000/4 000 identiques |

Le corpus réel seul ne suffit pas : des tests de mutation (passer `BUFSIZ` à 4095,
casser `\\\r\n`, l'insertion du `\n` au changement de flux, la limite de 4 300
chiffres, etc.) passaient inaperçus. Le corpus adverse les détecte tous.

## Performance (médiane de 5 passes sur les 63,8 Mo, 10,75 M objets, en ms)

Chaque mode passe chaque objet au même consommateur, qui le range dans un tableau
JS comme le ferait la pile d'opérandes de l'interpréteur.

| Mode | Node 26 (V8) | Chromium 148 (V8) | WebKit 26.4 (JSC) | Firefox 151 (installé) |
|---|---|---|---|---|
| TS v1, premier jet | 1 992 | 1 765 | 701 | 1 903 |
| TS v2, optimisé | **555** | **575** | **487** | 1 273 |
| Wasm, bande décodée en objets JS | 568 | 586 | 520 | **745** |
| Wasm seul, rien ne revient au JS | 375 | 375 | 373 | 495 |

- Références : pdfminer sous CPython **17 225 ms** (3,7 Mo/s) ; même code Rust en
  natif **289 ms** (221 Mo/s).
- Instanciation du Wasm : 2 à 3 ms.
- Taille : TS 12 Ko minifié (4,6 Ko gzip) ; Wasm 129 Ko (55 Ko gzip, sans
  `wasm-opt`) plus 1 Ko de décodeur JS.
- Le Firefox 150 fourni par Playwright (build Nightly patchée) exécute ce Wasm
  environ 7× plus lentement (2 643 ms en Wasm seul), avec ou sans automation. On
  utilise donc le Firefox installé, lancé en headless avec un profil temporaire.

## Lecture

1. **Le premier jet TS était 3,6× trop lent pour des raisons propres à V8** :
   tableaux recréés pour chaque objet, `length = 0`, chaîne construite pour
   chaque mot-clé. Après une passe guidée par le profil, son profil ressemble à
   celui du Rust : le découpage en tokens domine. La qualité de
   l'implémentation pèse plus que le langage.
2. **Si la frontière JS/Wasm est au grain du token**, le Wasm ne gagne rien sur
   V8 et JSC (égalité avec TS v2). Il gagne 1,7× sur Firefox.
3. **Si tout reste dans le Wasm** (seuls les résultats finaux traversent), le
   Wasm est 1,3 à 1,5× plus rapide que TS sur V8 et JSC, 2,6× sur Firefox, et le
   même code compilé en natif est 60× plus rapide que CPython (cible PyO3).
4. Ce n'est qu'un étage : l'interpréteur, les polices et la géométrie des tables
   peuvent déplacer ces ratios dans un sens ou dans l'autre.

## Reproduire

```sh
.venv/bin/python experiments/psparser-bench/dump_corpus.py            # corpus réel + oracle (~80 s)
.venv/bin/python experiments/psparser-bench/dump_corpus.py --fuzz 4000  # corpus adverse + oracle
(cd experiments/psparser-bench/rust && ~/.cargo/bin/cargo build --release --target wasm32-unknown-unknown \
  && cp target/wasm32-unknown-unknown/release/psparser.wasm psparser.wasm)
npx tsx experiments/psparser-bench/ts/verify.ts ts --corpus fuzz      # ou: wasm, --corpus corpus
npx tsx experiments/psparser-bench/ts/bench-node.ts
FIREFOX_BIN=/Applications/Firefox.app/Contents/MacOS/firefox node experiments/psparser-bench/run-browsers.mjs
(cd experiments/psparser-bench/rust && ~/.cargo/bin/cargo run --release --example bench)
```

Fichiers : `ts/psparser.ts` (TS v2), `ts/psparser-v1.ts` (premier jet, gardé
pour la comparaison), `rust/lib.rs` (Rust optimisé), `rust/lib-v1.rs` (premier jet
Rust, non compilé, natif 330 ms), `ts/wasm.ts` (chargeur + décodeur de bande),
`ts/canon.ts` (sérialisation canonique partagée avec Python).
