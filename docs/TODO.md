# TODO — améliorations futures

## Limites d'analyse connues

- **Valeurs loop-carried dans callback** — `exec_body` traverse le corps même avec une back-edge mais ne widen pas → `setX(arr[i])` enregistre valeur partielle. FN mineur sur la *valeur*, jamais de FP. *(ADR-009)*
- **Callees inconnus sans `Loc`** — `myHelper(() => setX())` → FN sur helpers externes qui exécutent le callback en synchrone. *(ADR-010)*
- **`missing-deps` FP sur variables fonction stables** — `const cb = () => setData({loaded: true})` → `Reference(Unstable)` → `missing-deps` fire même si `cb` ne capture aucune valeur mutable. Conservatif acceptable (cf. ESLint rules-of-hooks).
- **`derived-state` corps conditionnels** — détection linéaire uniquement (≤2 blocs). Effet avec branches conditionnelles → FN conservatif.
- **Analyse inter-composants** — implémentée (ADR-012). Limites acceptées : résolution d'imports hors scope, composants dynamiques (`const C = cond ? A : B`) non tracés, plugin système (Next.js/TanStack) futur.
- **`useState(null)` sans annotation TypeScript** — init Null sans type hint → `StateType::Unknown` → `join(Null, Number) = Top` → convergence immédiate → FN possible sur boucles. Atténué : `useState<number>(null)` détecté via le hint TS (voir ADR-008). Pattern non annoté reste un FN accepté.
- **`useState({...})` retourne `Reference(Unstable)`** — l'analyseur ne distingue pas la première création de l'objet (mount) de sa réutilisation cross-render (cached). Conséquence : `[obj]` dans un deps array passe pour entièrement instable et peut déclencher `always-unstable-deps`. Conservatif acceptable.
- **`lazy-init` top-level uniquement** — détecte `useState(call())` mais pas `useState(1 + call())` (call imbriqué). Évite des FP sur `useState(a + 1)` si un futur `+` devenait un call. FN accepté.
