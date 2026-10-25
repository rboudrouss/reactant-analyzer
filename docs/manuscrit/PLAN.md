# Plan du manuscrit

Titre de travail : *Analyse statique de programmes React par interprétation
abstraite — anatomie complète de reactant-analyzer*. Instantané du dépôt au
commit `e67b10a` (27 septembre 2026). Langue : français. Public : un lecteur
qui connaît Rust et React côté utilisateur, pas forcément l'analyse statique.
But : à la fin, le lecteur connaît très bien la codebase (fichiers, types,
algorithmes, décisions, limites) et sait la modifier dans l'esprit du projet.

Chaque chapitre est écrit à partir d'un ou plusieurs **dossiers** de
`notes/NN-*.md` (matière vérifiée, extraits avec numéros de ligne) et du code
lui-même. Les consignes de forme sont dans `GUIDE-REDACTION.md`.

## Progression

Six parties, du concret au difficile :

| Partie | Contenu | Difficulté |
|---|---|---|
| I. Le problème et son contexte | bugs React, modèle d'exécution de React, rappels d'interprétation abstraite, tour d'architecture | 1–2 |
| II. Du source à l'IR | oxc et détection, IR, lowering, splice/inlining | 2–3 |
| III. Les domaines abstraits | treillis simples, produit StateValue et stabilité, transfert et stores | 2–4 |
| IV. Le moteur | analyse de CFG et dominance, point fixe d'un composant, relations, churn, inter-composants | 3–5 |
| V. Les règles | API typée, règles d'état, infinite-loop, règles effets, rendu/RSC, règles déclaratives | 3–5 |
| VI. L'outil et sa méthode | CLI/driver/projets, distribution, tests et corpus, histoire et doctrine, limites | 2–3 |
| Annexes | fiches ADR, glossaire, catalogue des règles, carte de la codebase | — |

## Lexique imposé (cohérence entre chapitres)

Règle : les **entités du code** gardent leur nom anglais (en `\code{}` quand
c'est un identifiant, en romain sinon) ; les **concepts mathématiques** sont en
français avec le symbole unifié de `macros.tex`.

| Concept | Terme à employer | À éviter |
|---|---|---|
| ⊑ | « ordre du treillis », `\lleq` | « inférieur » sans précision |
| ⊔ / ⊓ | « join » / « meet » (mots invariables), `\join` `\meet` | « union », « intersection » (réservés aux ensembles) |
| ∇ | « widening » (italique à la première occurrence), `\widen` | « élargissement » |
| ⊤ / ⊥ | « top » / « bottom », `\Top` `\Bot` ; « inconnu » pour ⊤ en prose libre | « indéfini » |
| γ | « concrétisation », `\gam` | |
| must / may | « must » / « may » en romain (« un fait must », « une arête may ») ; `\must` `\may` en math | « certain / possible » seuls (peuvent accompagner) |
| slot | « slot » (emplacement d'état d'un `useState`, indexé par label) | « case », « variable d'état » |
| site | « site » (une occurrence d'écriture / d'allocation dans le code) | « emplacement » |
| writer / SlotWriter | « écrivain » pour le concept, `\relation{slot_writers}` pour la relation, `\code{SlotWriter}` pour le type | |
| seed | « seed » (valeur qui amorce un slot) | « graine » |
| guard | « garde » | |
| churn | « churn » (une référence recréée à chaque rendu qui re-déclenche) | « barattage » |
| reviver | « reviver » (site qui ranime une écriture convergée) | |
| trigger / effect_triggers | « déclencheur », `\relation{effect_triggers}` | |
| render / commit / effects | « rendu », « commit », « effets » (phases React) ; `\Render` etc. en math | |
| handler | « handler » (callback événementiel) | « gestionnaire » |
| deps | « deps » (tableau de dépendances d'un hook) | « dépendances » sauf en prose explicative |
| Error / Warning / Info | `\Error` `\Warning` `\Info` | « erreur », « avertissement » comme niveaux |
| faux positif / faux négatif | « faux positif (FP) », « faux négatif (FN) » | |
| soundness | « soundness » (mot anglais, défini au chapitre 3 : l'analyse sur-approxime) | « correction » sans définition |
| fixpoint | « point fixe », `\lfp` | |
| lowering | « lowering » (abaissement AST → IR) | « traduction » |
| splice / inlining | « splice » (greffe d'un CFG dans un autre), « inlining » | |
| Versioned / PerRender / Stable | noms du code en `\code{}` | traductions |
| exact / must / may / ⊤ (polarité) | « polarité » d'une relation ou d'une colonne | |

Notation des programmes d'exemple : TSX, composants nommés en anglais
(`Counter`, `Timer`, `List`) ; sortie de l'analyseur telle qu'observée.

## Labels canoniques

Chaque chapitre définit `\label{chap:<slug>}`. Les renvois inter-chapitres
n'utilisent **que** ces labels (et `adr:NN`, `def:...` listés ci-dessous).

| Slug | Label |
|---|---|
| introduction | `chap:introduction` |
| react-modele | `chap:react-modele` |
| interpretation-abstraite | `chap:interpretation-abstraite` |
| tour-architecture | `chap:tour-architecture` |
| frontend-detection | `chap:frontend-detection` |
| ir | `chap:ir` |
| lowering-cfg | `chap:lowering-cfg` |
| splice-inlining | `chap:splice-inlining` |
| treillis-simples | `chap:treillis-simples` |
| statevalue-stabilite | `chap:statevalue-stabilite` |
| transfert-stores | `chap:transfert-stores` |
| cfg-analyzer-dominance | `chap:cfg-analyzer-dominance` |
| point-fixe-composant | `chap:point-fixe-composant` |
| relations | `chap:relations` |
| churn | `chap:churn` |
| inter-composants | `chap:inter-composants` |
| api-regles | `chap:api-regles` |
| regles-etat | `chap:regles-etat` |
| infinite-loop | `chap:infinite-loop` |
| regles-effets | `chap:regles-effets` |
| regles-rendu-rsc | `chap:regles-rendu-rsc` |
| regles-declaratives | `chap:regles-declaratives` |
| projet-cli-driver | `chap:projet-cli-driver` |
| distribution | `chap:distribution` |
| tests-corpus | `chap:tests-corpus` |
| histoire-doctrine | `chap:histoire-doctrine` |
| limites-perspectives | `chap:limites-perspectives` |
| annexe fiches ADR | `chap:fiches-adr`, et `adr:1` … `adr:42` |
| annexe glossaire | `chap:glossaire` |
| annexe catalogue des règles | `chap:catalogue-regles` |
| annexe carte de la codebase | `chap:carte-code` |

Définitions partagées (définies UNE fois, au chapitre indiqué, référencées
ailleurs par `\cref{def:...}`) :

| Label | Défini au chapitre | Objet |
|---|---|---|
| `def:treillis` | interpretation-abstraite | treillis, join, meet |
| `def:concretisation` | interpretation-abstraite | γ, correction d'une abstraction |
| `def:point-fixe` | interpretation-abstraite | post-point fixe, itération de Kleene |
| `def:widening` | interpretation-abstraite | widening, à seuils |
| `def:must-may` | interpretation-abstraite | fait must / fait may / ⊤ |
| `def:soundness` | interpretation-abstraite | soundness = sur-approximation ; FN interdits |
| `def:dominance` | interpretation-abstraite | dominateurs, sur tous les chemins |
| `def:slot` | ir | slot d'état, label de hook |
| `def:site` | ir | site d'allocation / d'écriture (`ExprId`) |
| `def:cfg` | ir | CFG, bloc, terminator, EdgeKind |
| `def:composant-hook-utilitaire` | frontend-detection | les trois classes détectées |
| `def:stabilite` | statevalue-stabilite | treillis Stability, Versioned/PerRender |
| `def:statevalue` | statevalue-stabilite | produit StateValue |
| `def:store` | transfert-stores | les stores (state, memo, heap, shared) |
| `def:relation` | relations | relation du moteur, polarité |
| `def:slot-writer` | relations | la ligne SlotWriter |
| `def:churn-graph` | churn | graphe de churn, arêtes must/may |
| `def:render-dep` | inter-composants | dépendance de rendu |
| `def:certified` | api-regles | Certified / MustResult / sceau de sévérité |
| `def:witness` | api-regles | chaîne de témoins |

## Les chapitres

Pour chaque chapitre : dossiers sources, fichiers à lire en priorité, ADR,
plan de sections, éléments obligatoires (exemples gradués, schémas, encadrés),
modèle recommandé pour le rédacteur et pages visées (PDF A4, 11 pt).

---

### 01 — Introduction : pourquoi analyser statiquement React ?
`chapters/01-introduction.tex` · difficulté 1 · 15–20 pages · rédacteur opus
- Dossiers : 16 (§1, §6 exemples), 10 et 11 (§ « bug React visé » de chaque règle), 13 (§6.1 invocation), 15 (§5.1 chronologie brève), `README.md`, `docs/usage.md`.
- Plan : (1) trois programmes React fautifs très simples (boucle infinie `useEffect(() => setN(n+1))`, deps manquantes, stale closure d'un `setInterval`) et ce qui se passe à l'exécution ; (2) ce qu'un linter syntaxique voit et ne voit pas (alias de setter, hook custom, setter passé en prop) ; (3) le pari : interpréter abstraitement le composant ; (4) la promesse : Error = défaut certain, Warning = possible, Info = limite ; soundness (FN interdits) — en mots, renvoi au chapitre 3 ; (5) première prise en main : `cargo run -- check`, lecture d'une sortie humaine et JSON (sorties réelles) ; (6) le projet en chiffres (lignes, fichiers, 22 règles, 42 ADR, corpus) ; (7) plan du manuscrit et conseils de lecture par profil.
- Obligatoire : encadré `react` sur le cycle render → commit → effets ; figure TikZ du pipeline (parse → lowering → IR → engine → relations → règles → driver) ; tableau des règles du catalogue (id, niveau max, une ligne) tiré de `reactant rules` ou `src/rules/docs.rs`.
- Références : `\cref{chap:react-modele}`, `\cref{chap:interpretation-abstraite}`, `\cref{chap:tour-architecture}`, `\cref{chap:catalogue-regles}`.

### 02 — React vu par l'analyste : le modèle d'exécution que l'on abstrait
`chapters/02-react-modele.tex` · difficulté 2 · 25–35 pages · rédacteur opus
- Dossiers : 16 (intégralement), ADR-001, ADR-009, ADR-017, ADR-025, ADR-035.
- Plan : (1) composant = fonction pure du rendu ; trigger → render → commit ; (2) `useState` : file d'updaters, batching, `Object.is` bail-out, updater fonctionnel ; (3) effets : passifs après commit, deps et `Object.is`, cleanup, `[]`/absent, `useLayoutEffect` ; (4) identité référentielle : littéraux frais, `useMemo`/`useCallback`/`useRef` ; (5) règles des hooks et ordre positionnel ; (6) StrictMode double rendu ; (7) Context et propagation ; (8) Server/Client Components et `"use client"` ; (9) React-tRace : sémantique formelle (syntaxe, tree memory, StepInit/StepEffect/StepCheck/StepEvent, théorèmes) — sources citées ; (10) la sémantique concrète du projet (ADR-001) : ce qu'on garde, ce qu'on abstrait (file d'updates, ordre des effets, bail-out) et pourquoi c'est sound ; table de correspondance React-tRace ↔ reactant (fichier:lignes) ; (11) les exemples canoniques du manuscrit (Counter, SelfCounter, Timer, Toggle, ObjChurn…) définis ici et réutilisés ensuite.
- Obligatoire : chronologie TikZ render → commit → effets → setState → render ; automate des modes React-tRace (`automata`) ; encadrés `react` nombreux ; `piege` sur « set = prochain rendu » et sur le double rendu StrictMode.
- Références : `chap:interpretation-abstraite`, `chap:point-fixe-composant`, `chap:statevalue-stabilite`.

### 03 — Rappels d'interprétation abstraite
`chapters/03-interpretation-abstraite.tex` · difficulté 2 · 20–30 pages · rédacteur opus
- Dossiers : 04 (§10.1 point 2, §3 intervalles pour l'exemple), 06 (§4 analyze_cfg pour l'exemple de worklist), 09 (must/may), `refs.bib` (Cousot & Cousot 77/79, Rival & Yi).
- Plan : (1) sémantique concrète = ensembles d'états, indécidabilité, approximation ; (2) ordres, treillis, ⊥/⊤, join/meet (`def:treillis`) ; (3) concrétisation γ et correction (`def:concretisation`) ; (4) exemple filé : le compteur en intervalles ; (5) fonctions de transfert monotones, équations de flot sur un CFG, itération de Kleene, post-point fixe (`def:point-fixe`) ; (6) hauteur infinie et widening, widening à seuils, pourquoi le narrowing peut être remplacé (`def:widening`, renvoi ADR-014) ; (7) must / may / ⊤ et la polarité d'un fait (`def:must-may`) ; (8) soundness : FN interdits, FP tolérés ; ce que ça impose à chaque niveau de diagnostic (`def:soundness`) ; (9) CFG, dominance, « sur tous les chemins » (`def:dominance`) ; (10) analyse intra vs inter-procédurale, inlining ; (11) mise à jour forte vs faible ; (12) ce que reactant fait et ne fait pas (pas de SSA, insensible au flot pour l'état, sensible pour l'env…).
- Obligatoire : diagrammes de Hasse TikZ (booléens plats, intervalles tronqués) ; `algorithm2e` de l'itération de Kleene et de la worklist ; exercices avec solutions (calculs de join/widening à la main).
- Références : `chap:treillis-simples`, `chap:cfg-analyzer-dominance`, `chap:point-fixe-composant`.

### 04 — Tour d'architecture : lire la codebase
`chapters/04-tour-architecture.tex` · difficulté 2 · 15–25 pages · rédacteur opus
- Dossiers : §1 de TOUS les dossiers (01 à 15), 13 (§4.10 `run_check` pas à pas), 15 (§ principes), `CLAUDE.md`, `Cargo.toml`, `src/lib.rs`, `src/main.rs`.
- Plan : (1) arborescence `src/` module par module (une phrase par fichier, tableau) et les crates (`reactant-wasm`, `npm/`) ; (2) le flot d'une invocation `reactant check counter.tsx` de bout en bout, avec les types qui passent d'une couche à l'autre (`ParserReturn` → `Candidate` → `ComponentIR` → `AnalysisResult` → `ProgramRelations` → `Diagnostic` → sortie) ; (3) les cinq couches et leurs frontières (jamais d'AST après le lowering ; les règles ne parcourent pas l'IR — ADR-042) ; (4) les conventions transversales : fail-closed (absence = non prouvé), must/may comme types, `FileId`/`ComponentId` internés, `SourceRange` ; (5) les trois principes de `CLAUDE.md` et comment ils se lisent dans le code ; (6) comment naviguer : tests, fixtures, `--trace`, `--verbose`, `explain` ; (7) feuille de route des chapitres suivants (quel fichier est traité où).
- Obligatoire : figure TikZ du pipeline détaillée (modules et types) ; tableau fichiers → chapitre (préfigure l'annexe carte de code).
- Références : tous les `chap:`.

---

### 05 — Le front-end : oxc et la détection des composants, hooks et utilitaires
`chapters/05-frontend-detection.tex` · difficulté 2–3 · 25–35 pages · rédacteur opus
- Dossier : 01 (intégralement). Fichiers : `src/lowering/mod.rs`, `detector.rs`, `component_detector.rs`, `hook_detector.rs`, `utility_detector.rs`, `hook_call_detect.rs`, `jsx_detect.rs`, `module_facts.rs`, `import_resolution.rs`, `utility_lowerer.rs`. ADR-003, 005, 013, 026, 040. Issues #63, #65, #51, #122, #5.
- Plan : suivre le §10.1 du dossier 01 : oxc (arène, `Program<'a>`, spans, `ParserReturn`, `panicked` vs diagnostics, `SourceType`) → contrat du front-end → marcheur commun et `Candidate` → les trois classes (`def:composant-hook-utilitaire`), partition non exhaustive → les deux FN corrigés (#5, #122) → provenance des hooks (`HookOrigin`, `ImportCtx`, `classify_callee`, shadowing) → faits de module (constantes, contextes, `ModuleFacts`, directives, graphe d'import, `reachable_from`) → identité des callees JSX (`CompOrigin`, `resolve_child`, ADR-040) → limites comme question de soundness.
- Obligatoire : arbre de décision TikZ de `is_component` et de `classify_callee` ; diagramme des trois classes ; exemples gradués du dossier (§6) avec sorties observées ; encadrés `decision` pour ADR-040 et les wontfix.
- Références : `chap:ir`, `chap:lowering-cfg`, `chap:inter-composants`, `chap:regles-rendu-rsc`, `chap:projet-cli-driver`.

### 06 — La représentation intermédiaire
`chapters/06-ir.tex` · difficulté 2–3 · 30–40 pages · rédacteur opus
- Dossier : 03 (§1–§4.4, §5, §6.1, §6.4, §6.7, §7–9 ; laisser splice/remap/α-renommage/expansion au chapitre 8). Fichiers : `src/ir/*.rs` sauf `splice.rs`, `remap.rs`. ADR-003, 004, 010, 011, 039, 040.
- Plan : pourquoi une IR en CFG (ADR-003, alternatives) → squelette : `CFG`, `BasicBlock`, `Terminator`, `EdgeKind`, `Stmt` (`def:cfg`) → `Expr` par familles (littéraux, allouants, accès, opérateurs, appels, JSX, valeurs de hooks), `ExprId` et sites (`def:site`), `for_each_child` → hooks : `HookEntry`, labels positionnels (`def:slot`), `HookMarker`/`MarkerVal`, `DepsArg`/`DepsList`/`Arity`, provenance → unités : `ComponentIR`/`HookIR`/`FunctionIR`, `ModuleConstInit`/`ContextId`, `ModuleFacts` → identités internées `FileId`/`ComponentId` (ADR-040) → positions : `SourceRange`, `SourceMap`, positions synthétiques (ADR-039) → analyses syntaxiques : variables libres, `AccessPath`, couverture par préfixe, certificats de liaison.
- Obligatoire : chaque enum/struct central avec `\rustsnippet` verbatim et description champ par champ ; CFG du compteur dessiné en TikZ (style `cfgbloc`) ; treillis `Arity` ; exercices (prédire `DepsArg`).
- Références : `chap:frontend-detection`, `chap:lowering-cfg`, `chap:splice-inlining`, `chap:regles-effets` (missing-deps utilise la couverture).

### 07 — Le lowering : de l'AST au CFG
`chapters/07-lowering-cfg.tex` · difficulté 3 · 35–45 pages · rédacteur opus
- Dossier : 02 (intégralement). Fichiers : `src/lowering/cfg_builder.rs`, `expr_lower.rs`, `hook_extractor.rs`. ADR-003, 004, 010, 025, 035, 039 ; ADR-020 §1, §11 ; issues #1, #2, #4, #5, #134.
- Plan : suivre le §10.2 du dossier 02 : la machine à blocs (`BlockBuilder`, sceller/ouvrir, `into_cfg`, fall-through ADR-025) → `if`/`else`, retour anticipé → expressions (« garder les lectures », `lower_for_effect`) → diamants (`? :`, `&&`, `||`, `??`) → boucles (`Back`, `break`/`continue`/labels) → `switch` et `try` (deux sous-approximations corrigées) → motifs, destructuring, temporaires → fonctions imbriquées, classes, sites d'allocation (ADR-010) → `await` et arêtes `Await` (ADR-035), positions synthétiques → extraction des hooks (provenance, marqueurs, destructuration d'état, corps de hooks) → handlers et subscriptions → hoist de terminateur (#4) et flèches concises (#5) → limites.
- Obligatoire : gabarits TikZ de CFG pour `if`, `while`, `for`, `do…while`, `for…of`, `switch`, `try`, `&&`, ternaire, `await` ; avant/après `extract_hooks` sur un exemple ; exemples gradués avec l'IR effectivement produit (méthode de dump indiquée dans le dossier).
- Références : `chap:ir`, `chap:cfg-analyzer-dominance`, `chap:regles-effets` (conditional-hook), `chap:relations` (phases et `Await`).

### 08 — Splice, remap et inlining
`chapters/08-splice-inlining.tex` · difficulté 3–4 · 20–30 pages · rédacteur opus
- Dossiers : 03 (§4.5–§4.11, §6.2, §6.3, §6.5, §6.6, §6.8, §8), 06 (§ expansion des hooks personnalisés et utilitaires, `SpliceIds`), 05 (§ B5/B6 inlining local). Fichiers : `src/ir/splice.rs`, `src/ir/remap.rs`, `src/ir/free_vars.rs` (partie substitution), `src/engine/fixpoint.rs` (expansion), `src/engine/hook_registry.rs`, `function_registry.rs`, `src/registry/summary.rs` (résumés de hooks de bibliothèque). ADR-005, 010, 012, 024, 025.
- Plan : pourquoi inliner (intra-procédural + registre, ADR-005) → remap des identifiants (`ExprId`, `BindingId`, blocs, `Offsets`) → splice d'un CFG dans un autre (`Return` → `Let`/`Jump`, `Unreachable`, garde-`throw`) → α-renommage et hygiène (#141), capture → expansion des hooks custom (`expand_custom_hooks`, régions, table des labels avant/après) → utilitaires (`FunctionRegistry`, B6) → résumés de bibliothèque (`SummaryRegistry`, « connu ≠ modélisé ») → attribution des findings aux hooks inlinés (ADR-024 : rendre l'origine, ne jamais dédupliquer) → cas limites et collisions d'`ExprId`.
- Obligatoire : schéma TikZ avant/après splice ; déroulé bloc par bloc de l'exemple §6.2 du dossier 03 ; encadré `decision` ADR-024.
- Références : `chap:ir`, `chap:point-fixe-composant`, `chap:inter-composants`, `chap:api-regles`.

---

### 09 — Les treillis simples : plats, ensembles bornés, intervalles
`chapters/09-treillis-simples.tex` · difficulté 2–3 · 25–35 pages · rédacteur opus
- Dossier : 04 (§1–§3.5, §4 intervalles, §6.1, §6.2). Fichiers : `src/domains/mod.rs`, `impls/mod.rs`, `bool_val.rs`, `setter_val.rs`, `str_const.rs`, `interval.rs`. ADR-002, ADR-014 ; commit 548f922.
- Plan : le trait `AbstractDomain` (`PartialOrd` = ordre, join, widen) → `flat_lattice!` (`BoolVal`, `SetterVal`), γ exact → `StrConst` (k-ensembles, hauteur finie) → `Interval` : ordre, hull, arithmétique (`+ - * / %` `**`, `NaN`, ±∞), `is_int`, comparaisons, bitwise → widening simple puis à seuils (`widen_to`, seuils = littéraux du composant, ADR-014, pourquoi pas de narrowing) → raffinement sur gardes (`narrow_*`) → soundness de chaque opération (preuves courtes) → limites (`0 × ±∞`, `narrow_truthy`).
- Obligatoire : diagrammes de Hasse TikZ ; chronologie du compteur `[0,0] → [0,1] → [0,2] → saut au seuil` ; exercices calculatoires avec solutions.
- Références : `chap:interpretation-abstraite`, `chap:statevalue-stabilite`, `chap:cfg-analyzer-dominance`.

### 10 — Le produit StateValue et la stabilité référentielle
`chapters/10-statevalue-stabilite.tex` · difficulté 4 · 30–40 pages · rédacteur **fable**
- Dossier : 04 (§3.6–§3.9, §4 StateValue et Stability, §6.3–§6.6, §7, §8). Fichiers : `src/domains/impls/state_value.rs`, `stability.rs`, `src/domains/context.rs`. ADR-002, 008, 015, 017 ; F5 et `ObjChurn`.
- Plan : pourquoi un enum plat perd `null ∪ number` (ADR-008) → sortes disjointes ⇒ produit (ADR-015), `KindMask`, `as_arith`, `ToNumber(null)=0`, join/meet/widen du produit, γ (`def:statevalue`) → la stabilité : treillis naïf d'ADR-002, le FP F5 et le FN `ObjChurn` qui le rendent intenable → traces de changement, bornes may/must, treillis d'ADR-017 (`Stable`, `Versioned(set)`, `PerRender`, ⊤/⊥) (`def:stabilite`) → conversion côté lecture (double vue store/lecture) → `useMemo`/`useCallback`/objets littéraux → projections vers les règles (`to_stability`, `is_unbounded`, `is_unstable_reference_only`, `StabilityVerdict`) → `AnalysisCtx`/`QueryContext`/`InterCtx` comme passerelle vers le moteur → limites (`Toggle`, `MemoOverObjState`).
- Obligatoire : Hasse de `Stability` (reprendre le dessin ASCII de `stability.rs`) ; carré may/must d'ADR-017 avec exemples React ; diagramme « double vue » ; preuves de soundness des joins.
- Références : `chap:treillis-simples`, `chap:transfert-stores`, `chap:regles-effets` (always-unstable-deps), `chap:infinite-loop`.

### 11 — Fonctions de transfert, interpréteur d'expressions et stores
`chapters/11-transfert-stores.tex` · difficulté 3–4 · 35–45 pages · rédacteur **fable**
- Dossier : 05 (intégralement). Fichiers : `src/domains/transfer/state_value.rs`, `interp/*.rs`, `stores/*.rs`. ADR-002, 009, 010, 012, 015, 017.
- Plan : suivre le §10.2 du dossier 05 : le problème (`setN(n+1)`) → `AbstractEnv::lookup` (⊤ par défaut, deux tables `stabs`/`locs`), évaluateur d'expressions opération par opération (tableau exhaustif) → ce que le lowering a déjà fait → les stores (`StateStore` ⊥/join/amorçage, `MemoStore` ⊤/écrasement, `Heap`, `SharedStateStore`) et leur tableau comparatif (`def:store`) → l'instruction : `bind_rhs`, `exec_setter_call`, mise à jour faible, updater fonctionnel → conversion côté lecture (ADR-017) → callbacks : `TriggerClass`, pré-passe, `exec_body_impl` (ADR-009) → le tas par site d'allocation, `EnvVal::Loc`, `alloc_fn`, captures, `resolve_locs`, B5/B6, membres (#88) → `recompute_memo` → inter-composants : `eval_comp_app`, props → tas enfant, havoc, cache → soundness en pratique : tableau §4.7 puis défauts D1–D8 comme étude de cas.
- Obligatoire : flot d'appel TikZ `analyze_cfg → exec_stmt → … → eval_expr` ; carte des stores (cinq boîtes) ; schéma du tas ; exercices de calcul à la main avec solutions.
- Références : `chap:statevalue-stabilite`, `chap:point-fixe-composant`, `chap:inter-composants`, `chap:relations`.

---

### 12 — Interpréter un CFG : worklist, arcs arrière, dominance
`chapters/12-cfg-analyzer-dominance.tex` · difficulté 3 · 20–30 pages · rédacteur opus
- Dossier : 06 (§ `analyze_cfg`, `narrow_env_for_branch`, `dominance.rs`, tests `cfg_analyzer.rs`, §6.4). Fichiers : `src/engine/cfg_analyzer.rs`, `src/engine/dominance.rs`, `src/engine/eval.rs`. ADR-014, ADR-025.
- Plan : `analyze_cfg` sur un bloc, un diamant (join), une boucle (arc `Back`, compteur par en-tête, `widen_to`) ; état par bloc et `state_out` insensible au flot ; raffinement de branche (tableau motif × branche → opérateur) ; effets de bord des `Return` ; `compute_dominators` (itératif en RPO), `on_all_paths`, sorties atteignables ; `eval_in_stores`/`ConvergedEval` (sondes post-convergence) ; terminaison et complexité.
- Obligatoire : `algorithm2e` de la worklist ; CFG TikZ annoté des environnements ; tableau d'itérations pour `loop_counter_*` ; exercice : dominateurs d'un losange.
- Références : `chap:interpretation-abstraite`, `chap:lowering-cfg`, `chap:point-fixe-composant`, `chap:api-regles` (must_* via dominance).

### 13 — Le point fixe d'un composant
`chapters/13-point-fixe-composant.tex` · difficulté 4 · 35–45 pages · rédacteur **fable**
- Dossier : 06 (intégralement, sauf ce qui est au chapitre 12). Fichiers : `src/engine/fixpoint.rs` (analyze_component_impl et tout ce qu'il appelle), `triggers.rs`, `hook_registry.rs`. ADR-001, 005, 009, 014, 017 ; commit e67b10a pour effect_setter_writes si concerné.
- Plan : le problème (calculer un ensemble d'états, pas simuler) → préparation (constantes de module, inlining, seuils, amorçage des `useState`) → un tour : render (`analyze_cfg`), mémos (valeur = stabilité), tous les effets à chaque tour depuis `env_exit` (abstraction du batching), handlers (dans le point fixe, hors `widen_trace`), join avec la tranche du `SharedStateStore`, test `new ⊑ state` → widening externe (`widen_threshold = 3`), cap à 100 → post-convergence : rafraîchissement du render, ré-exécution des effets depuis ⊥ (`effect_setter_writes`), dérivation des relations (`slot_writers`, `slot_seeds`, `registrations`, `effect_triggers` exact = must / versioned = may) → correspondance avec React-tRace (ADR-001) et argument de soundness (join, mise à jour faible, tous les effets) → déroulés à la main : compteur, effet de montage, `useEffect(() => setX(x+1))`, compteur gardé → ce que le moteur garantit et ne garantit pas (§8 du dossier : updater inter, cleanups, `useReducer`, ⊤ par join sans widening, `WidenEvent.writers`).
- Obligatoire : diagramme de flot TikZ de `analyze_component_impl` ; chronogramme React aligné sur les tours abstraits ; tableaux d'itérations ; `algorithm2e` de la boucle externe ; `piege` pour chaque trou de soundness observé (présenté honnêtement comme constat au commit e67b10a).
- Références : `chap:react-modele`, `chap:cfg-analyzer-dominance`, `chap:transfert-stores`, `chap:relations`, `chap:inter-composants`, `chap:infinite-loop`.

### 14 — Les relations du moteur
`chapters/14-relations.tex` · difficulté 4 · 35–45 pages · rédacteur **fable**
- Dossier : 07 (§1–§8, §6 exemples 1–4, §9). Fichiers : `src/engine/setters.rs`, `written.rs`, `seeds.rs`, `guards.rs`, `registrations.rs`, `triggers.rs`, `program_relations.rs`, `docs/relations.md`. ADR-027, 028, 030, 031, 032, 033, 034, 035, 036, 037, 038, 039, 042.
- Plan : pourquoi des relations (ADR-042, dérive entre deux walkers), polarité exact/must/may/⊤ (`def:relation`) → la marche des setters : région, `WalkClass`, phase, appels directs, B6, IIFE, `SYNC_HOF_METHODS`, `await` → table des registrars (contrat vs devinette) → la ligne `SlotWriter` colonne par colonne (`def:slot-writer`), une ligne par site, `same_tick`, `Updater`, provenance, `must_direct_write`, lignes étrangères owner-qualified (ADR-030) → `written` : `Freshness`, `SiteEnvs` → `effect_triggers` → `slot_seeds` (ADR-031), `registrations` et appariement (ADR-034), `slot_reads`, `call` (ADR-036/037), `context_consumers` (ADR-032) → preuve mono-site : `guard_chain`, `site_guards`, `expand_guard` (ADR-038) → `ProgramRelations` et la frontière de certification.
- Obligatoire : schéma des trois couches ; table `WalkClass → WriterPhase` ; CFG annoté Sync/Deferred ; tableau des colonnes de chaque relation (comme `docs/relations.md`) ; déroulés sur les exemples 1–4 du dossier avec la sonde.
- Références : `chap:point-fixe-composant`, `chap:churn`, `chap:api-regles`, `chap:regles-etat`, `chap:regles-effets`.

### 15 — Le graphe de churn et la preuve multi-site
`chapters/15-churn.tex` · difficulté 5 · 25–35 pages · rédacteur **fable**
- Dossiers : 07 (§ churn, §6 exemples 5–10, §4.3.4), 10 (§ infinite-loop bras 3 et 4), 15 (fiches ADR-018, 029, 042 §6). Fichiers : `src/engine/churn.rs`, `src/rules/helpers/cycles.rs`, `src/engine/written.rs` (revivers), tests `tests/effect_cycles.rs`. ADR-017, 018, 029, 042 ; commit e67b10a.
- Plan : le problème (deux effets qui se re-déclenchent via des références fraîches) → le graphe : slots qualifiés, arêtes Must/May, `self_slot`, `no_deps`, `on_all_paths` (`def:churn-graph`) → SCC de Tarjan en deux passes (sous-graphe Must d'abord) → historique du kill : condition « un seul écrivain » (ADR-018) → sites multiples → « chaque écriture qui s'exécute est un site », revivers (commit e67b10a) → convergence « sous toutes les écritures », plus petit point fixe des sites convergents, pourquoi un plus grand point fixe serait insound → cross-component (lignes étrangères, `props_hold`, `stays_mounted`, plafond Warning) → `must_effect_cycle` et re-dérivation → limites (#157, mirror state).
- Obligatoire : graphes TikZ (slots, SCC coloriées, arêtes must pleines / may pointillées) ; tableau des itérations du point fixe des sites ; preuves (lemmes courts) de la soundness du kill.
- Références : `chap:relations`, `chap:statevalue-stabilite`, `chap:infinite-loop`, `chap:inter-composants`.

### 16 — L'analyse inter-composants et inter-fichiers
`chapters/16-inter-composants.tex` · difficulté 4 · 30–40 pages · rédacteur opus (effort max)
- Dossier : 08 (intégralement), 05 (§ `eval_comp_app`), 06 (§ `analyze_program`, phases). Fichiers : `src/engine/render_deps.rs`, `symbol_graph.rs`, `root_detector.rs`, `component_cache.rs`, `component_registry.rs`, `analysis_result.rs`, `program_result.rs`, `src/engine/fixpoint.rs` (analyze_program). ADR-012, 013, 040, 041, 042 ; `docs/campaign/rerender-cascade-plan.md` ; commits e257bcc, e67b10a.
- Plan : le problème (props = ⊤, fils qui appelle le setter du parent) → identité `ComponentId` (ADR-040), `CompOrigin`, `resolve_child` → racines (`--all-roots`, `--entry`) → inlining top-down (`eval_comp_app`, `InterCtx`, pile de récursion, props dans le tas) → flux ascendant : `ComponentSetter`, store partagé monotone, havoc → cache (égalité de treillis, éviction) et la subtilité de soundness §8.2 → deux phases, `phase1_reached` → `AnalysisResult`/`ProgramAnalysisResult`/`RuleCtx` → graphe de symboles (ADR-013 prévu vs réalisé) → la dépendance de rendu (ADR-041) : motivation, treillis `Deps` = P(Source) ∪ {⊤}, `gated`, transfert et dépendance de contrôle, handlers hôtes et landings, contextes, bindings de module (`writes`), `RenderIndex`, `uses`, `home_of`, `wasted_siblings` (`def:render-dep`).
- Obligatoire : diagramme de séquence TikZ de `eval_comp_app` ; schéma phase 1 / phase 2 ; treillis `Deps` ; arbre d'éléments de `drill.tsx` annoté ; exemples §6 du dossier.
- Références : `chap:point-fixe-composant`, `chap:transfert-stores`, `chap:regles-etat` (state-lifted-too-high), `chap:regles-rendu-rsc`.

---

### 17 — La couche règles : requêtes typées, sévérité certifiée, témoins
`chapters/17-api-regles.tex` · difficulté 3 · 30–40 pages · rédacteur opus
- Dossier : 09 (intégralement). Fichiers : `src/rules/mod.rs`, `registry.rs`, `docs.rs`, `api/*.rs`, `helpers/*.rs`. ADR-006, 007, 019, 021, 024, 042 ; tests `docs_drift.rs`, `catalogue.rs`.
- Plan : suivre le §10.2 du dossier 09 : le FN historique `is_unstable` (ADR-021) → trait `Rule` et exemple minimal (`conditional-hook`) → `Diagnostic`/`Severity` et le sceau par visibilité Rust → typestate `Certified`/`MustResult`/`May` (`def:certified`), « essayez de forger une Error » → primitives `must_*` (dominance, `must_setter_on_all_paths` = plus grand point fixe, ∀-stable, `classify_motion` → `must_frozen_seed`, `must_effect_cycle`, `must_stale_capture`) → témoins : `Step`, `FileId`, producteurs partagés, `located`, rendu humain/JSON (`def:witness`) → registre : passe par composant, clamp `pin ⊓ polarité`, off/allow, suspension, tri, docs générées → cache (`ProgramCache`, `OnceLock`) → helpers programme (`render_tree`, `mount`, `context_flow`, `jsx`, `providers`, `cycles`, `purity`) → frontière walk-free (ADR-042, cliquet `layer_boundary`) → Tier A et ouverture.
- Obligatoire : graphe de types du typestate (flèches interdites en rouge) ; pipeline du clamp ; séquence `check → safe_check → located → clamp → …` ; tableau d'itérations de `must_out`.
- Références : `chap:relations`, `chap:regles-etat`, `chap:regles-effets`, `chap:regles-declaratives`, `chap:projet-cli-driver`.

### 18 — Les règles sur l'état
`chapters/18-regles-etat.tex` · difficulté 3–4 · 35–45 pages · rédacteur opus
- Dossier : 10 (tout sauf infinite-loop et state-lifted-too-high). Fichiers : `src/rules/impls/lazy_init.rs`, `unstable_context_value.rs`, `redundant_set_state.rs`, `setter_in_render.rs`, `derived_state.rs`, `state_mutation.rs`, `frozen_initial_state.rs`. ADR-021, 027, 028, 031, 038.
- Plan : ordre du §10.2 du dossier 10 (1 → 7). Pour chaque règle, une section avec la même grille : bug React visé (encadré `react`), conditions exactes, niveau et preuve requise (quelles primitives must), relations lues, exemples gradués déclenchant / non déclenchant avec sortie observée, FP assumés et FN interdits, `piege`, fichiers et lignes.
- Obligatoire : CFG TikZ de `Conditional` vs `Unconditional` avec dominance (setter-in-render) ; diagramme de décision des strates de `frozen-initial-state` ; tableau récapitulatif des 7 règles.
- Références : `chap:api-regles`, `chap:relations`, `chap:statevalue-stabilite`, `chap:infinite-loop`.

### 19 — Étude de cas : infinite-loop et state-lifted-too-high
`chapters/19-infinite-loop.tex` · difficulté 5 · 25–35 pages · rédacteur **fable**
- Dossiers : 10 (§ infinite-loop intégral, § state-lifted-too-high), 07 (churn), 06 (widen_trace, effect_setter_writes), 08 (render_deps). Fichiers : `src/rules/impls/infinite_loop.rs`, `state_lifted_too_high.rs`, `widening_info.rs`. ADR-008, 014, 015, 017, 018, 029, 041 ; issues #144, #90, #157.
- Plan : la règle la plus riche, bras par bras : (1) point fixe : widening comme signal, `widen_trace`, `effect_setter_writes`, les deux portes, plafond Warning (#144) ; (2) cross-component ; (3) graphe de churn multi-effets, Error seulement pour un cycle tout-Must mono-composant ; (4) self-churn ADR-017 (triple must, Info hors deps) ; garde ∀-stable sur les deps ; re-déclenchement sensible aux champs (#90) ; témoins produits ; FP/FN observés (état dérivé, alias). Puis `state-lifted-too-high` : dépendance de rendu, `home_of`, « l'absence d'usage vaut preuve ».
- Obligatoire : frise des itérations pour `Counter` et `Bounded` ; graphes de churn des exemples `TwoEffects`, `MultiWriter`, `Dag` ; arbre d'éléments de `drill.tsx` ; tableau des bras (condition, niveau, preuve).
- Références : `chap:point-fixe-composant`, `chap:churn`, `chap:inter-composants`, `chap:regles-etat`.

### 20 — Les règles sur les effets
`chapters/20-regles-effets.tex` · difficulté 3–4 · 35–45 pages · rédacteur opus
- Dossier : 11 (tout sauf server-component-hook et wasted-subtree-render). Fichiers : `src/rules/impls/widening_info.rs`, `analysis_limit_info.rs`, `conditional_hook.rs`, `always_unstable_deps.rs`, `missing_deps.rs`, `unnecessary_rerender.rs`, `missing_cleanup.rs`, `stale_closure.rs`. ADR-017, 021, 034, 038 ; issues #40, #42, #142.
- Plan : ordre du dossier 11 (1 → 7) : règles Info (contrat « FN possible » annoncé, suspension) → `conditional-hook` (dominance, seule Error « pure structure ») → `always-unstable-deps` (`Object.is`, `PerRender` comme borne must, pourquoi `Versioned` se tait) → `missing-deps` (chemins d'accès, couverture par préfixe, polarité fire/stop, trois kills ; wontfix #40) → `unnecessary-rerender` (montage vs convergé, idiome SSR) → `missing-cleanup` (relation d'enregistrement, cleanup trois-valué) → `stale-closure` (conjonction de cinq must, #142 ; wontfix #42). Même grille par règle qu'au chapitre 18.
- Obligatoire : CFG en losange avec dominateurs ; arbre des préfixes de `bag.r.current` annoté ; chronologie TikZ de la capture figée de `Timer` ; matrice `REGISTRARS` firing × timing ; arbre de la conjonction des cinq must de stale-closure avec le test qui réfute chaque feuille.
- Références : `chap:api-regles`, `chap:relations`, `chap:statevalue-stabilite`, `chap:ir` (couverture).

### 21 — Rendu et Server Components : wasted-subtree-render, server-component-hook
`chapters/21-regles-rendu-rsc.tex` · difficulté 4 · 15–25 pages · rédacteur opus
- Dossiers : 11 (§ ces deux règles), 08 (render_deps, `RenderIndex`), 01 (`ModuleFacts`, `reachable_from`), 13 (Next.js). Fichiers : `src/rules/impls/wasted_subtree_render.rs`, `server_component_hook.rs`, `src/rules/helpers/render_tree.rs`, `src/project/nextjs.rs`. ADR-026, 041.
- Plan : `wasted-subtree-render` : analyse de dépendance séparée, arbre d'éléments, « tout inconnu est un usage », options de règle, fixtures `typing.tsx`, `children.tsx` ; `server-component-hook` : graphe de modules Next, atteignabilité avec frontière `"use client"`, pourquoi Warning et pas Error (preuve hors domaine, ADR-026 §4).
- Obligatoire : arbre d'éléments TikZ avec propagation de `Relevance` ; graphe de modules Next avec frontière en pointillés.
- Références : `chap:inter-composants`, `chap:frontend-detection`, `chap:projet-cli-driver`.

### 22 — Les règles déclaratives et les packs
`chapters/22-regles-declaratives.tex` · difficulté 3 · 30–40 pages · rédacteur opus
- Dossier : 12 (§1–§9 ; laisser la distribution WASM/npm au chapitre 24). Fichiers : `src/rules/declarative/*.rs`, `packs/guardrails.json`, `packs/community/*.json`, `docs/custom-rules.md`, `docs/schemas/pack.schema.json`. ADR-022, 023, 027 §2, 030 ; issues #143, #68, #67, #101.
- Plan : suivre le §10.2 du dossier 12 (1 → 9) : pourquoi pas ESLint → anatomie d'une règle et grammaire (EBNF) → système de sortes (matrice ancre/arête/garde/champ) → validation en trois étages, catalogue E1–E25 / W1–W4 → exécution (`EntityCtx`, énumération, conjonction court-circuitée, gabarit) → sévérité `pin ⊓ polarité` → quantificateurs `any_of`/`every`/`none` → croissance du vocabulaire Tier-A (3/21 → 21/22) → limites et brèche #143.
- Obligatoire : diagramme de flot `JSON → Value → PackFile → ResolvedRule → TierARule → Diagnostic` ; diagramme entité-relation des sortes ; au moins 5 règles déclaratives complètes commentées (listings JSON) avec sortie observée.
- Références : `chap:api-regles`, `chap:relations`, `chap:regles-etat`, `chap:distribution`.

---

### 23 — Projets, configuration, driver et CLI
`chapters/23-projet-cli-driver.tex` · difficulté 2–3 · 30–40 pages · rédacteur opus
- Dossier : 13 (intégralement). Fichiers : `src/main.rs`, `src/lib.rs`, `src/config.rs`, `src/cli/*.rs`, `src/driver/*.rs`, `src/project/*.rs`, `src/resolver/*.rs`, `src/registry/*.rs`. ADR-011, 016, 026 ; `docs/usage.md`, `docs/schemas/reactant-config.schema.json`.
- Plan : suivre le §10.1 du dossier 13 (1 → 10) : invocation vue de l'utilisateur → configuration (JSONC, précédence, `pin ⊓ polarity`, off/allow) → `run_check` comme composition → découverte des fichiers (`.gitignore`, exclusions) → projets et résolution d'imports (tsconfig `extends`/`references`/`paths`, Vite, monorepo) → parse/lowering/canal d'erreurs → blind spots et `--follow-imports` (« pas de clean bill pour du code non lu ») → Next.js et graphe serveur → rendu humain et JSON (schéma v2, deux comptages, `LocationIndex`) → `SummaryRegistry`.
- Obligatoire : diagramme de flot de `run_check` avec les sorties `EXIT_USAGE` ; arbre de décision de `build_context` ; graphe tsconfig ; tableau de précédence flags/config ; sorties réelles.
- Références : `chap:frontend-detection`, `chap:api-regles`, `chap:regles-rendu-rsc`, `chap:distribution`.

### 24 — Distribution : WASM, npm, GitHub Action, CI
`chapters/24-distribution.tex` · difficulté 2 · 12–18 pages · rédacteur opus
- Dossiers : 12 (§ distribution), 14 (§ CI et livraison). Fichiers : `crates/reactant-wasm/src/lib.rs`, `npm/` (package.json, lib, bin, scripts, build.sh), `action.yml`, `scripts/gh-action.mjs`, `.github/` si présent, `Cargo.toml` (features).
- Plan : features Cargo (`cli`, `schema-gen`) et build WASM sans CLI ; l'hôte non fiable et `MemFileSystem` ; le paquet npm (`bin`, `lib`, `.d.ts` généré, `packs build`) ; parité native/WASM (tests) ; l'Action GitHub (entrées, sorties, piège de l'épingle) ; les jobs CI ; versions et stabilité (ADR-017 « versioned stability » ne concerne pas ceci — attention à l'homonymie ; ADR-016 pour la CLI).
- Obligatoire : séquence de chargement native vs WASM (deux colonnes convergeant sur `load_pack`/`run_check`).
- Références : `chap:regles-declaratives`, `chap:projet-cli-driver`, `chap:tests-corpus`.

### 25 — Tests, corpus et campagnes : la méthode de mesure
`chapters/25-tests-corpus.tex` · difficulté 2–3 · 25–35 pages · rédacteur opus
- Dossier : 14 (intégralement). Fichiers : `tests/*.rs` (structure), `src/test_support.rs`, `scripts/corpus-baseline.py`, `corpus-diff.py`, `setup-test-repo.sh`, `rerender-bench/`, `docs/corpus-baseline.json`, `docs/precision-log.md`, `docs/limitations.md`, `docs/campaign/*.md`, `skills/reactant-triage/`.
- Plan : suivre le §10 du dossier 14 (1 → 12, la livraison étant au chapitre 24) : pourquoi mesurer → la sévérité comme contrat → pyramide des tests → discipline des paires fires/silent → blind spots → le corpus (14 dépôts épinglés) → la métrique (rangées vs localisations, digest) → l'erreur de comptage du 2026-09-03 comme étude de cas → cycle de vie d'un chiffre → campagnes (FP classique, wish-list à l'aveugle) → oracle runtime jsdom → classer ce qu'on ne corrige pas (labels, wontfix, réouverture de #64).
- Obligatoire : pyramide TikZ ; flot `setup-test-repo.sh → reactant → JSON → corpus-baseline.py` ; entonnoir de campagne ; diagramme de séquence avec les runs réels.
- Références : `chap:api-regles`, `chap:histoire-doctrine`, `chap:limites-perspectives`.

### 26 — Histoire et doctrine du projet
`chapters/26-histoire-doctrine.tex` · difficulté 3 · 20–30 pages · rédacteur opus
- Dossier : 15 (§1–§4, §5.1, §5.3–§5.4, §6–§8 ; les fiches ADR §5.2 sont en annexe A). Fichiers : `CLAUDE.md`, `docs/adr/README.md`, `docs/adr/ADR-020`, `git log`.
- Plan : suivre le §10.2 du dossier 15 (1 → 9) : le problème de départ (ADR-001/003/004) → premières semaines → tournant du corpus (ADR-015/016/017) → codification (CLAUDE.md, ADR-020 : les 11 non-changements) → polarité comme type (ADR-021/022/023) → relations (ADR-027 → 039, 042) → identité et attribution (ADR-019/024/039/040) → cascades de rendu (ADR-041) → méthode (décision, mesure, journal, wontfix, cliquets) → les fils rouges.
- Obligatoire : frise chronologique TikZ avec phases et histogramme des commits ; graphe des ADR (supersedes/amends/extends) ; diagramme « migration des faits » (rules/helpers → engine) ; encadrés `decision` et `piege` du §10.5 du dossier.
- Références : `chap:fiches-adr`, tous les chapitres techniques.

### 27 — Limites connues, défauts observés et perspectives
`chapters/27-limites-perspectives.tex` · difficulté 3 · 15–25 pages · rédacteur opus (effort max)
- Dossiers : la section 8 (« Subtilités, pièges, limites ») et la section « Vérification » de TOUS les dossiers 01–16 ; `docs/limitations.md` ; `docs/TODO.md` ; `gh issue list --state open` (labels soundness-bug / precision-fn / precision-fp) ; wontfix fermés.
- Plan : (1) limites documentées et assumées (`docs/limitations.md`, wontfix) classées par couche ; (2) défauts observés pendant la rédaction du manuscrit au commit e67b10a, rassemblés et classés : FN vérifiés (updater fonctionnel en inter, cleanups hors store, `useReducer`, ⊤ par join sans widening, alias de setter en rendu, `<Ctx value>` React 19, lacunes de détection : `switch`, `.map`, hook en argument, classes…), FP vérifiés (redundant-set-state sur deux constantes-références, Error de state-mutation sur branches exclusives, bras point fixe sur état dérivé), écarts ADR ↔ code (ADR-003 règle 4, ADR-013 symbol_extractor, ADR-018, ADR-042 §4/§5, `docs/semantics.md` absent, doc-comments en retard) — chacun avec un programme minimal, la sortie observée, la cause dans le code (fichier:lignes) et la piste de correction « à la racine » ; (3) dette et pistes (TODO, issues ouvertes) ; (4) perspectives de recherche (sémantique formelle des extensions, useReducer, cleanup, précision relationnelle…). Ton factuel : ce sont des constats au commit e67b10a, pas des jugements.
- Obligatoire : tableau récapitulatif (défaut, type FN/FP/écart, couche, sévérité, fichier, issue si existante) ; renvois aux chapitres où le mécanisme est expliqué.
- Références : tous.

---

### Annexe A — Fiches ADR
`chapters/A-fiches-adr.tex` · rédacteur opus · 30–45 pages
- Dossier : 15 (§5.2 : une fiche par ADR), `docs/adr/*.md`.
- `\chapter{Fiches des décisions d'architecture (ADR)}\label{chap:fiches-adr}` puis, pour chaque ADR 1 → 42, une `\section{ADR-NN — Titre}` avec `\label{adr:NN}` (obligatoire, ces labels sont référencés partout par `\adr{NN}`), et : contexte/problème, décision, alternatives refusées, conséquences, statut (Accepted / Superseded by / Implemented), ADR liés, fichiers du code qui l'incarnent (chemins), chapitre du manuscrit qui l'explique (`\cref{chap:...}`). 8–20 lignes par fiche, plus pour les ADR structurants (001, 002, 003, 009, 010, 012, 014, 015, 017, 018, 021, 022, 027, 041, 042).

### Annexe B — Glossaire
`chapters/B-glossaire.tex` · rédacteur opus · 10–15 pages
- Dossiers : la section 9 (« Glossaire ») de TOUS les dossiers 01–16 + le lexique de ce plan.
- `\chapter{Glossaire}\label{chap:glossaire}` ; entrées triées alphabétiquement (`description` list), une à trois phrases chacune, terme du code en `\code{}`, où il est défini (`\srcline{}`), chapitre (`\cref{chap:...}`). Fusionner les doublons entre dossiers, harmoniser avec le lexique imposé. Viser 150–250 entrées.

### Annexe C — Catalogue des règles
`chapters/C-catalogue-regles.tex` · rédacteur opus · 8–12 pages
- Dossiers : 10, 11 (tableaux récapitulatifs), 12 (packs livrés), `src/rules/docs.rs`, `cargo run -q -- rules` et `explain <rule>`.
- `\chapter{Catalogue des règles}\label{chap:catalogue-regles}` : une fiche compacte par règle native (id, niveau max et conditions par niveau, relations lues, primitives must, fichier, tests, chapitre) puis les règles des packs livrés (`guardrails`, `community/*`), avec la sortie de `reactant rules` en listing.

### Annexe D — Carte de la codebase
`chapters/D-carte-code.tex` · rédacteur opus · 10–15 pages
- Dossiers : la section 2 (« Inventaire des fichiers ») de TOUS les dossiers 01–16 ; `find src crates tests -name '*.rs' | xargs wc -l`.
- `\chapter{Carte de la codebase}\label{chap:carte-code}` : pour chaque fichier Rust de `src/`, `crates/`, et les tests d'intégration : chemin, lignes, rôle en une ou deux phrases, types publics principaux, chapitre du manuscrit (`\cref`). Grouper par module, `longtable`. Terminer par un graphe TikZ des dépendances entre modules de `src/`.

### Avant-propos
`chapters/00-avant-propos.tex` · rédigé en dernier par l'orchestrateur : objet du manuscrit, comment le lire (parcours par profil), conventions (encadrés, `\rustsnippet`, numéros de ligne réels, commit photographié), remerciements aux sources (React docs, React-tRace, Cousot).

## Ordre d'assemblage (`chapter-list.tex`)

```
\part{Le problème et son contexte}
01 02 03 04
\part{Du source à la représentation intermédiaire}
05 06 07 08
\part{Les domaines abstraits}
09 10 11
\part{Le moteur}
12 13 14 15 16
\part{Les règles}
17 18 19 20 21 22
\part{L'outil et sa méthode}
23 24 25 26 27
\appendix
A B C D
```
