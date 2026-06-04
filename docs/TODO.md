# TODO — améliorations futures

## Précision d'analyse

### Callbacks in-cycle — limites connues *(ADR-009 + ADR-010)*

- **Valeurs loop-carried dans un corps de callback** → vues à leur valeur de **première itération**. `exec_body` traverse le corps pour ses effets de bord même avec une back-edge (les setters dans une boucle fire), mais ne widen pas les back-edges → `setX(arr[i])` enregistre une valeur partielle. FN mineur sur la *valeur*, jamais de FP.
- **Callees inconnus sans `Loc`** (`myHelper(() => setX())`) → FN sur helpers externes/wrappers qui exécutent le callback en synchrone.

---

## Infrastructure

### Analyse inter-composants

Intra-procédural uniquement. Props parent→enfant non tracées. Nécessite graphe de composants + analyse inter-procédurale (complexité O(n²)).

---

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

### `derived-state` *(implémenté — 2026-06-04)*

Détecte les effets dont les deps sont `[stateA]`, qui appellent `setB(expr)` inconditionnellement où `expr` est call-free, et `setB` n'est appelé nulle part ailleurs.

`Expr::is_call_free()` ajouté dans `src/ir/expr.rs`. Règle dans `src/rules/derived_state.rs`.

**Limite connue** : détection linéaire uniquement (corps d'effet à ≤2 blocs). Corps avec branches conditionnelles → FN conservatif.
