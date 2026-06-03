# TODO — améliorations futures

## Précision d'analyse

### Callbacks in-cycle — limites connues *(ADR-009 + ADR-010)*

- **Back-edge dans un corps de callback** → `exec_body` bail conservateur (`Reference(Unstable)`) → setter dans une boucle `for`/`while` interne non propagé (FN). Fix : traversée *side-effect-only* même en présence de back-edge.
- **Callees inconnus sans `Loc`** (`myHelper(() => setX())`) → FN sur helpers externes/wrappers qui exécutent le callback en synchrone.

### Functional updaters `(n) => n + 1` ne déclenchent pas de widening

```js
useEffect(() => {
  setCount(c => c + 1)  // FnLit → Reference(Unstable) → join Top → converge sans widening
}, [])
```

`FnLit` évalué à `Reference(Unstable)` → cross-type join avec `Number([init])` → `Top`. Converge en 2 itérations sans déclencher `widened_labels`. Fix : signal spécifique pour les functional updaters dont le corps appelle le setter de façon non-identité.

### `InfiniteLoopRender` émet un seul diagnostic

`check` retourne au premier setter trouvé. Si `setA` et `setB` appelés dans le render body, un seul est reporté. Fix : collecter tous les setters avant de retourner.

---

## Infrastructure

### Analyse des event handlers comme points d'entrée *(ADR-009)*

Handlers (`onClick={}`, `addEventListener`) actuellement skippés (`Subscription`/`Unknown`) pour éviter FP `infinite-loop`. Migration actée dans ADR-009 :

1. **Lowering** — lifter les handlers JSX `onX` + `addEventListener` en racines de première classe avec leur env de binding.
2. **Engine** — analyser via `analyze_cfg` ; weak-join des effets taggés provenance `event`, **exclus de `widened_labels`**.
3. **Politique** — flip `Subscription`/`Unknown` → `analyze-as-entry-point` dans `classify_callee`.
4. **Multiplicité** — handler tourne 0..N fois → fixpoint sur ces racines aussi.

Débloque : **stale-closure-in-handler**, **missing-cleanup** (`addEventListener` sans `removeEventListener`).

### Analyse inter-composants

Intra-procédural uniquement. Props parent→enfant non tracées. Nécessite graphe de composants + analyse inter-procédurale (complexité O(n²)).

### Block-states pour les effect bodies

`AnalysisResult::block_states` ne contient que les états du render CFG. Ajouter `effect_block_states: HashMap<HookLabel, HashMap<BlockId, AbstractEnv<D>>>`.

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

### `derived-state` *(bloqué)*

Effect dont les deps sont `[stateA]`, appelle `setB(expr)` inconditionnellement où `expr` est call-free et ne lit que des `StateVal` sources, et `setB` n'est appelé nulle part ailleurs.

Bloqué par : `Expr::Call` opaque (pas de support fonctions pures), functional updaters non analysés, dépend de `effect_block_states`.
