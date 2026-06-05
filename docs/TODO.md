# TODO — limites d'analyse restantes

## Faux négatifs connus (FN)

- **Callees inconnus sans `Loc`** — `myHelper(() => setX())` → FN si le helper est importé depuis un autre module (pas de `Loc` dans l'env). Fix nécessite analyse interprocédurale. *(ADR-010)*

- **`cross-component-infinite-loop` FN si parent analysé intra seulement** — si le composant parent n'est pas atteint par l'analyse top-down (Phase 2 fallback, props = ⊤), le `SharedStateStore` n'est pas peuplé → règle ne fire pas. Sous-cas de la limite imports hors scope. *(ADR-012)*

- **`useState(null)` sans annotation TypeScript** — init `Null` sans type hint → `join(Null, Number) = Top` → convergence immédiate → FN possible sur boucles. Atténué : `useState<number>(null)` détecté via le hint TS. *(ADR-008)*

- **Valeurs loop-carried dans callback** — `exec_body` ne widen pas sur back-edges → `setX(arr[i])` enregistre valeur partielle. FN mineur sur la *valeur*, jamais de FP. *(ADR-009)*

## Faux positifs connus (FP)

- **`missing-deps` FP sur variables fonction stables** — `const cb = () => setData({loaded: true})` → `Reference(Unstable)` → `missing-deps` fire même si `cb` ne capture aucune valeur mutable. Conservatif acceptable (cf. ESLint rules-of-hooks).

- **`useState({...})` retourne `Reference(Unstable)`** — l'analyseur ne distingue pas la première création de l'objet (mount) de sa réutilisation cross-render. Conséquence : `[obj]` dans un deps array peut déclencher `always-unstable-deps`. Conservatif acceptable.

## Périmètre hors scope (futur)

- **Résolution d'imports** — `import { Button } from './Button'` non tracé ; composant absent du registry → `⊤` + Info `analysis-limit` émis. *(ADR-012)*
- **Composants dynamiques** — `const C = cond ? A : B; <C />` → `CompApp` non généré, non analysé.
- **Plugin système** — Next.js (`pages/`), TanStack (route components) → future extension de `RootDetector`.
