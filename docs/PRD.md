# PRD — reactant-analyzer

Analyseur statique React basé sur l'interprétation abstraite.

## Problème

Les hooks React ont une sémantique de render opaque. Les développeurs commettent régulièrement des erreurs qui causent des re-renders inutiles (dégradation perf silencieuse) ou des comportements incorrects (boucles infinies, stale closures). Les outils existants (`eslint-plugin-react-hooks`) travaillent au niveau syntaxique et ratent les bugs qui nécessitent un raisonnement sur le dataflow et la stabilité des valeurs.

## Objectif

Construire un analyseur statique React en Rust qui détecte les bugs sémantiques liés aux hooks via **interprétation abstraite** — avec un taux de faux-positifs bas et une fondation formellement justifiée.

Référence de correction : React-tRace (Lee, Ahn, Yi — OOPSLA 2025).

## Non-objectifs

- Analyse de JS/TS complet (sous-ensemble maîtrisé uniquement).
- Analyse inter-composant dans la phase initiale.
- Remplacement de TypeScript (les types sont des hints, pas la source de vérité).
- Vitesse d'exécution sub-secondaire (priorité à la justesse).

## Bugs cibles

### Phase 1 — bugs natifs (hooks primitifs)

| ID | Nom | Description | Complexité d'analyse |
|---|---|---|---|
| B1 | Hook conditionnel | Hook appelé dans une branche conditionnelle | CFG + dominance |
| B2 | Deps manquantes | Variable utilisée dans useEffect absente du dep array | Stability + free vars |
| B3 | setState redondant | setState(v) où v ≡ état courant | Stability + constant prop |
| B4 | Boucle infinie render | useEffect déclenche setState sans condition d'arrêt | Fixpoint divergence |

### Phase 2 — hooks de librairie

Extension via HookRegistry aux hooks TanStack Query, React Router, Zustand, etc.

### Phase 3 — inter-composant

Instabilité de props entre parent et enfant causant des re-renders en cascade.

## Architecture technique

Voir ADRs dans `docs/adr/` et `docs/ir.md`.

| Composant | Rôle |
|---|---|
| `src/lowering/` | AST Oxc → IR CFG-based |
| `src/ir/` | Types IR (CFG, BasicBlock, Expr, HookEntry) |
| `src/domains/` | Stability lattice, StateStore, MemoStore, RefStore |
| `src/engine/` | Worklist fixpoint, cycle render/effect |
| `src/registry/` | Hook models (built-in + librairies) |
| `src/rules/` | Règles post-pass sur AnalysisResult |
| `src/diagnostics/` | Formatage des warnings |

## Roadmap

### Étape 0 — Décisions de design ✅ (cette session)
ADRs + IR spec + PRD. Fondation formelle établie.

### Étape 1 — Nettoyage du codebase
- Supprimer `src/impl_/` (CFG/worklist factices, jamais appelés).
- Conserver `src/core/aval.rs` + `src/core/abs_env.rs` comme base évolutive.
- Conserver les 7 règles actuelles uniquement comme **oracles de régression** end-to-end — elles ne constituent plus l'implémentation principale.

### Étape 2 — IR (`src/ir/`)
Implémenter les types IR selon `docs/ir.md`. Pas encore de lowering, pas encore de domaines.  
Critère de succès : on peut construire un `ComponentIR` à la main et le sérialiser.

### Étape 3 — Lowering (`src/lowering/`)
AST Oxc → IR. Désucrage JSX, hooks, destructuring, early returns → CFG.  
Critère de succès : les 8 exemples de `examples/bugs.tsx` produisent un IR correct.

### Étape 4 — Domaines abstraits (`src/domains/`)
Stability lattice + 3 stores. Trait `AbstractDomain`. Fonctions de transfert par nœud IR.  
Critère de succès : les fonctions de transfert passent des tests unitaires sur IR construit à la main.

### Étape 5 — Engine worklist (`src/engine/`)
Fixpoint générique paramétré par `AbstractDomain`. Cycle render → effects → check.  
Widening configurable. Métadonnée de widening dans `AnalysisResult`.  
Critère de succès : le fixpoint converge sur tous les exemples, avec et sans boucle infinie.

### Étape 6 — Sémantique formelle React (`docs/semantics.md`)
Spécification des extensions de React-tRace : dependency arrays, useMemo, useCallback, useRef.  
Critère de succès : chaque extension est justifiée par rapport aux règles React-tRace de base.

### Étape 7 — Règles (`src/rules/`)
Réécrire B1–B4 comme requêtes post-pass sur `AnalysisResult`.  
Critère de succès : détection des 8 bugs de `examples/bugs.tsx`, zéro faux-positif sur 20 composants clean.

### Étape 8 — Hook registry librairies (`src/registry/`)
Specs TanStack Query, React Router. Config utilisateur `reactant.toml`.  
Critère de succès : détection de bugs dans des projets TanStack réels.

### Étape 9 — Inter-procédural hooks customs
Inlining call-string-1 pour fonctions `use*`.  
Critère de succès : bugs dans custom hooks détectés depuis les composants appelants.

### Étape 10 — Perf + fuzzing
Interning des symboles, parallélisme rayon, fuzzing sur DefinitelyTyped + repos React publics.

## Métriques de succès

| Métrique | Cible phase 1 | Cible phase 2 |
|---|---|---|
| Bugs B1–B4 détectés sur exemples | 100% | 100% |
| Faux positifs sur composants clean | < 5% | < 3% |
| Faux négatifs sur suite de régression | 0 | 0 |
| Temps d'analyse (fichier 1000 LOC) | < 5s | < 2s |

## Qualité du code

- Fonctions de transfert citent la règle React-tRace correspondante en commentaire.
- Chaque règle testable indépendamment avec un `AnalysisResult` fabriqué.
- Tests de régression : l'interpréteur React-tRace sert d'oracle.
- Pas de `unwrap()` dans le chemin d'analyse (uniquement dans les tests).
