# Next steps — feuille de route recommandée

> Complément stratégique du tracker : les issues listent les limites
> d'analyse ouvertes une par une ([docs/limitations.md](limitations.md) en
> donne le résumé) ; ce fichier ordonne le travail par levier d'adoption.
> Principe directeur : **sortir l'outil et le vocabulaire devant des
> utilisateurs avant de creuser la précision.**
>
> Positionnement (double, les deux volets se renforcent) :
> 1. **Vérificateur sound pour le code que l'IA écrit** — les LLMs produisent
>    précisément les bugs que le niveau 3 attrape (boucles d'effets, deps
>    manquantes, état dérivé, closures périmées) ; sortie JSON déterministe +
>    chaînes de témoins = oracle exploitable dans une boucle d'agent.
> 2. **Langage de contraintes sémantiques pour lui dire quoi écrire** — les
>    packs Tier A transforment les conventions d'équipe (prose CLAUDE.md,
>    page Notion) en spec exécutable, gardée sur des faits prouvés du domaine,
>    donc robuste au refactoring et aux indirections — contrairement à une
>    règle eslint custom (pattern AST contournable). Le React Compiler
>    n'attaquera jamais ce volet.

## Phase 1 — Rendre l'outil installable (1-2 semaines)

1. **Publier le paquet npm** (`reactant-analyzer`) — le build wasm et le
   wrapper existent (`npm/`, smoke test 9/9 byte-identical vs natif). Reste
   de la finition : nom définitif, README npm, `npx reactant check src/` qui
   marche du premier coup.
2. **GitHub Action** — un `action.yml` autour du CLI (`--format json`,
   `--fail-on error`, exit codes déjà propres). Point d'entrée CI, donc point
   d'entrée « gate pour code généré par IA ».
3. **README orienté adoption** — section « reactant vs React Compiler vs
   eslint-plugin-react-hooks » : ce que chacun corrige, masque, ou prouve.
   Le compiler auto-mémoïse (il érode l'urgence de la classe stabilité
   référentielle) mais ne corrige ni `infinite-loop`, ni `derived-state`,
   ni `stale-closure`, ni les cycles cross-composants — c'est la ligne de
   différenciation à écrire noir sur blanc.

## Phase 2 — Le chemin critique du vocabulaire (le gros du trimestre)

**✅ FAIT (2026-08-26)** — les quatre items sont livrés, voir l'historique git
et les issues `enhancement` + `area/tier-a` du tracker :

> **Mise à jour 2026-09-02 — la courbe est arrivée au bout.** Le catalogue est à
> **21/22** (#116) ; la seule entrée encore bloquée, `nullable-return-unguarded`,
> est exclue par décision (#101). Les chiffres « 3/21 → 5/21 » ci-dessous sont
> ceux de l'époque et restent tels quels : l'historique complet de la courbe est
> dans [limitations.md](limitations.md).
>
> La question suivante n'est plus « combien du catalogue » mais « combien de ce
> qu'on nous demanderait » : 60 scénarios écrits à l'aveugle donnent **1
> exprimable, 16 déjà natifs, 16 partiels, 27 hors de portée**
> ([docs/campaign/](campaign/README.md), #128). Les deux relations qui débloquent
> le plus sont #126 (les appels non-hook d'un corps) et #127 (les *lectures*
> d'un slot).

4. ~~**Étape 1 ADR-023 : provenance des hooks**~~ — `HookOrigin` fail-closed,
   littéral `"react"` avant le resolver, spécificateur brut retenu ;
   `hook_provenance` (`label → origin hook, source, direct|inlined`) survit à
   `expand_custom_hooks` jusqu'à `AnalysisResult`, et le guard Tier-A
   `origin` exprime « jamais `useLayoutEffect` direct » wrapper-aware.
   Bonus mesurés : FN d'alias fermé, +4 TPs corpus (re-exports barrel).
5. ~~**Étape 2 ADR-023 : entités en position d'expression**~~ —
   `custom_arg_returns` calculé pendant le fixpoint (params ⊤, module consts
   seuls en scope), `ReturnsVerdict` (question d'*identité*, pas de
   stabilité) + reader ⊤-total dans `api/query.rs`, edge `args` + guard
   `returns` en Tier A ; `stability` refusé sur un argument (erreur de point
   de programme, ADR-023 §2). La règle zustand-selector est exprimable.
6. ~~**Re-mesure automatisée**~~ — `tests/catalogue.rs` matérialise le
   catalogue 21 règles ; chaque entrée exprimable est *prouvée* (le pack
   charge, fire sur le fixture buggé, silencieux sur le conforme).
   **Courbe : 3/21 → 5/21.** `cargo test --test catalogue -- --nocapture`
   imprime le rapport des bloqueurs.
7. ~~**Authoring JS/TS des packs**~~ — `reactant packs build <pack.js>` dans
   le wrapper npm : module évalué à l'authoring, validé par le `load_pack`
   du core (export wasm `validatePack`), **JSON généré = artefact committé** ;
   `npm/lib/pack.d.ts` généré de `pack.schema.json` (mêmes types schemars),
   anti-drift testé (`npm/test/packs.sh`).

## Phase 3 — Se faire connaître (en parallèle de la phase 2)

8. **L'article de comparaison** — les 8 corpus, le différentiel vs
   eslint-plugin-react-hooks, les cas où les auteurs eux-mêmes ont posé des
   `eslint-disable` que reactant tranche sémantiquement. L'envoyer aux
   auteurs de React-tRace (Lee, Ahn, Yi — OOPSLA 2025) : cette implémentation
   est probablement la seule industrielle de leur sémantique.
9. **Contacter l'équipe Oxc** — reactant est déjà sur leur parser ; oxlint
   cherche de l'analyse React profonde et se greffer sur un linter que les
   gens ont déjà bat toute installation standalone. Même une issue « voilà ce
   que je fais avec oxc » peut ouvrir le canal.
10. **Un mode agent** — serveur MCP, ou simplement `--format json` documenté
    comme interface agent, avec les chaînes de témoins (ADR-019) exposées
    comme explication causale exploitable pour l'autofix. Petit effort, gros
    effet de positionnement.

## Phase 4 — Précision (après avoir des utilisateurs)

11. **Trancher la question `useContext`** — post-pass sur les résultats vs
    unification des deux phases (« decide that first »). Plus
    gros gain de précision disponible (363 sites ⊤ sur les corpus), mais il
    sert les règles natives, pas le marché — il attend du feedback réel.
12. ~~**Étapes 3-4 ADR-023** (`writers` edge, anchor `context_providers`
    Tier A) au fil de l'eau, guidées par ce que les premiers packs
    utilisateurs n'arrivent pas à exprimer.~~ — **Remplacé par
    [ADR-027](adr/ADR-027-writer-relation-setter-provenance.md) §8**
    (2026-09-01) : la demande attendue est arrivée (famille
    « wrapper-enforcement », règles type `putState`) ; la séquence est
    tirée en avant : #6 → writers (#70) → résumés de phase → provenance
    setters → `context_providers` (#71).

## Anti-priorités

- Entamer la phase 4 avant la phase 1.
- Ajouter de nouvelles règles natives (issues `rule-proposal`) avant que le
  vocabulaire ne les rende exprimables en pack.
- Vendre « écrivez vos règles sémantiques » tant que l'expressivité mesurée
  reste à 3/21 — les étapes 4-6 d'abord.
- Toute re-mesure corpus non automatisée.

## 2026-09-02 (soir) — la campagne à l'envers : construire ce qui manquait

Après l'audit et le triage à l'aveugle, trois chantiers, tous vérifiés corpus
`+0 -0` sur les quatorze dépôts :

- **#129** — le rapport humain groupe par localisation source. chakra-ui 122 → 22,
  mantine 3165 → 359. Le JSON garde une ligne par composant.
- **#126 / #125** — la relation `calls` (ADR-036) : ce qu'un corps *fait*, au-delà
  de ses setters, comme second canal de la marche des setters. Plus `render_calls`,
  le quantificateur `none` (l'existentielle niée), les éléments hôtes dans
  `jsx_props` et la garde `prop`.
- **#127** — la relation `reads` (ADR-037) : l'image miroir de `writers`, région
  et phase, sur la même marche.
- **#130** — la grille tombe (ADR-038) : la marche atteint un `Call` dans
  *toute* position d'expression, pas seulement en position d'instruction, donc
  `wrap(setN(1))` est une écriture comme `setN(1)`. Corpus : **rien de retiré,
  27 lignes ajoutées, toutes en Warning**, et deux défauts de précision
  préexistants corrigés au passage — `setter-in-render` lit enfin la phase que
  la marche calcule (une écriture prouvée différée se tait, une écriture ⊤ ne
  prétend plus être un appel direct), et une composante ne se lit plus comme
  son propre parent quand son nom est salé.

- **#131** — une liaison synthétique est synthétique, sa position ne l'est pas
  (ADR-039) : les neuf sites où l'abaissement et le greffon fabriquaient une
  instruction sans span, une marche qui cesse de jeter la position sur laquelle
  elle se tient, et un constat qui prend la première position que nomme sa
  chaîne de témoins. **82 lignes sans position sur 7 146 → 0** (9 sur 6 340 → 0
  sans pack), jeu de résultats inchangé. Les six `JSON.stringify` de `commerce` nomment enfin le
  fichier qui les contient, et #129 les regroupe en une ligne.

Re-mesure des 60 scénarios : **8 EXPRESSIBLE (contre 1), 20 INEXPRESSIBLE
(contre 27)** — `docs/campaign/triage-2026-09-02-wave2.md`, avec
`packs/community/wave2.json` et ses paires de fixtures comme preuve exécutable.

- **ADR-040** — une lecture n'est périmée que si *toutes* les poignées de son
  chemin peuvent changer. `missing-deps` interrogeait la racine et le chemin
  entier, jamais l'entre-deux : `bag.ref.current` où `bag` est reconstruit à
  chaque rendu mais `bag.ref` est un `useRef` lit la valeur courante. Corpus :
  **6 340 → 5 654, 686 retirées, aucune ajoutée** — 11 % de la sortie totale,
  d'un seul tenant.

- **ADR-041 / ADR-042** — trois des quatre formes de #89, chacune corrigée là où
  l'information se perdait, jamais dans la règle. Un index calculé cache ce qui
  est *sous* lui, pas la chaîne *au-dessus* (`theme.snackBar[v].color` lit tout
  `theme.snackBar`, que le tableau de deps nomme). La stabilité
  comportementale ne savait pas résoudre un `useCallback` : l'extraction de
  hooks réécrit `useCallback(fn, deps)` en `CallbackVal(label)`, donc toute
  liaison de ce genre était supposée périmable quoi qu'elle capture. Et une
  sous-expression nommée **verbatim** dans les deps épingle les lectures
  qu'elle contient — `[searchParams.get("sort")]` couvre le corps qui évalue la
  même expression, tandis qu'un substitut *avec perte* (`[JSON.stringify(o)]`)
  n'épingle rien, sous peine de faux négatif. Corpus : **1 423 → 1 402
  emplacements distincts (5 654 → 5 511 attributions), 21 retirées, aucune
  ajoutée**, toutes vérifiées faux positifs.

- **ADR-043** — la quatrième forme de #89 : la chasse aux liaisons prend un
  *chemin*, pas un nom. Un nom nu est le cas de base ; chaque segment descend
  dans le champ du seul `ObjectLit` auquel le préfixe est lié, en suivant les
  alias (`{ bump }` enregistre le membre comme `Var("bump")`, exactement la
  propagation que fait l'interpréteur). Un conteneur est la façon dont un hook
  personnalisé rend un rappel : `useFormErrors()` de mantine renvoie cinq
  `useCallback`, et son appelant lit `$errors.clearFieldError` treize fois. Les
  deux lecteurs `fn_binding_in` / `callback_binding_in` n'en font plus qu'un,
  `closure_binding_of`, qui répond aussi *laquelle* des deux orthographes.
  Corpus : **1 402 → 1 394 emplacements distincts (5 511 → 5 119 attributions),
  8 retirées, aucune ajoutée** — les 24 annoncées étaient un comptage sur une
  base plus étroite.

  Reste de #89 : l'alias (`const c = x` enregistre tout `x` au lieu des
  membres que le corps touche) — ~5 emplacements. La chasse suit les alias, la
  marche des variables libres ne réécrit pas les chemins qu'elle enregistre.

Prochaines marches, par valeur décroissante : les valeurs d'arguments (#67) ;
une requête de dominance ; les résumés d'écosystème (#94), seule façon de
réduire la classe ⊤ qu'ADR-038 §2 laisse en Warning ; et la moitié
atteignabilité de `reads` (#132).
