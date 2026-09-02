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

Re-mesure des 60 scénarios : **8 EXPRESSIBLE (contre 1), 20 INEXPRESSIBLE
(contre 27)** — `docs/campaign/triage-2026-09-02-wave2.md`, avec
`packs/community/wave2.json` et ses paires de fixtures comme preuve exécutable.

Prochaines marches, par valeur décroissante : les valeurs d'arguments (#67) ;
une requête de dominance ; les résumés d'écosystème (#94), seule façon de
réduire la classe ⊤ qu'ADR-038 §2 laisse en Warning ; les lignes sans span
(#131) ; et la moitié atteignabilité de `reads` (#132).
