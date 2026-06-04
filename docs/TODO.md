# TODO — améliorations futures

## Nouvelles règles

### `missing-deps` pour `useCallback` et `useMemo`

Même logique que `missing-deps` existant, appliquée aux `HookEntry::Callback` et `HookEntry::Memo`. Étendre `collect_effect_info`.

### `always-unstable-deps`

```js
useEffect(() => { doX() }, [{}])       // {} = Reference(Unstable) chaque render
useEffect(() => { doX() }, [someObj])  // someObj = Reference(Unstable)
```

Toutes les deps évaluées à `is_unstable()` → effect tourne chaque render. Détection : `eval_expr` sur chaque dep, vérifier `is_unstable()`.

### `useState-lazy-init`

```js
const [data, setData] = useState(expensiveCompute())  // recalculé chaque render
```

`HookEntry::State { init: Expr::Call { .. } }` → warn. Règle structurelle pure, pas de fixpoint.

---

## Limites d'analyse connues

- **Valeurs loop-carried dans callback** — `exec_body` traverse le corps même avec une back-edge mais ne widen pas → `setX(arr[i])` enregistre valeur partielle. FN mineur sur la *valeur*, jamais de FP. *(ADR-009)*
- **Callees inconnus sans `Loc`** — `myHelper(() => setX())` → FN sur helpers externes qui exécutent le callback en synchrone. *(ADR-010)*
- **`missing-deps` FP sur variables fonction stables** — `const cb = () => setData({loaded: true})` → `Reference(Unstable)` → `missing-deps` fire même si `cb` ne capture aucune valeur mutable. Conservatif acceptable (cf. ESLint rules-of-hooks).
- **`derived-state` corps conditionnels** — détection linéaire uniquement (≤2 blocs). Effet avec branches conditionnelles → FN conservatif.
- **Analyse inter-composants** — intra-procédural uniquement. Props parent→enfant non tracées. Nécessite graphe de composants + analyse inter-procédurale (O(n²)).
