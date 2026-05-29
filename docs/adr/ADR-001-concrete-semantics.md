# ADR-001 : React-tRace comme sémantique concrète de référence

- **Statut** : Accepté
- **Date** : 2026-05-29

## Contexte

Un analyseur basé sur l'interprétation abstraite nécessite une sémantique concrète C dont on dérive la sémantique abstraite C#. Sans C explicite, la correction (soundness) de l'analyseur ne peut pas être établie formellement, et les fonctions de transfert sont écrites "au jugé".

Le papier React-tRace (Lee, Ahn, Yi — OOPSLA 2025) fournit une sémantique opérationnelle formelle des hooks React (`useState`, `useEffect`), prouvée conforme au comportement de React sur une suite de tests empiriques.

## Décision

React-tRace est adopté comme sémantique concrète C de référence. Les fonctions de transfert abstraites sont dérivées des règles de React-tRace. Les extensions nécessaires (dependency arrays, `useMemo`, `useCallback`, `useRef`, objets) sont spécifiées comme extensions de React-tRace dans `docs/semantics.md`.

## Justification

- React-tRace est la seule formalisation React publiquement disponible avec preuve de conformance.
- Le modèle Tree Memory + render loop (StepInit → StepEffect → StepCheck) correspond directement à l'itération de fixpoint de notre interpréteur abstrait.
- Les règles clés (SttReBind, CheckEffect, CheckNoEffect) définissent exactement les conditions de re-render détectables par notre analyse.
- L'interpréteur React-tRace (OCaml, dépôt `react-trace/`) sert d'oracle de tests.

## Limites acceptées

- React-tRace couvre uniquement `useState` et `useEffect` sans dependency arrays.
- Leur langage minimal ≠ JS/TS complet — on travaille sur un sous-ensemble maîtrisé.
- Les extensions hors scope React-tRace sont spécifiées localement sans garantie formelle équivalente.

## Conséquences

- `docs/semantics.md` spécifie les extensions de React-tRace.
- Les fonctions de transfert dans `src/domains/` citent la règle React-tRace correspondante.
- Les tests de régression vérifient que l'analyseur abstrait sur-approxime les traces de l'interpréteur React-tRace sur les exemples du papier.
