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

### Handlers JSX — limitations restantes *(ADR-009 migration, suite)*

JSX `onX={fn}` handlers sont maintenant des points d'entrée de première classe (`HookEntry::Handler`, dans le fixpoint loop — ADR-009 §5). Reste :

1. **`addEventListener` dans les effects** — lowering depuis `body_cfg` d'un effect vers `HookEntry::Handler` avec env au site d'appel (stale-closure-in-handler débloqué).
2. **Politique `Subscription`** — flip `classify_callee::Subscription` → `analyze-as-entry-point` pour les callbacks passés à `addEventListener` inline dans un effect.

### Diagnostic notes pour autres règles *(ADR-011)*

`Diagnostic.notes` et `HookEntry.span` sont implémentés. Reste :
- Propager `effect.span` dans `missing-deps`, `unnecessary-rerender`, etc.
- Handler span : extraire le range de la prop JSX `onX` depuis le lowering (pas de Stmt correspondant dans l'IR actuel).

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
