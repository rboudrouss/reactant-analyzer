# TODO — améliorations futures

Fonctionnalités utiles non encore implémentées. Classées par thème.

## Précision d'analyse

### Traverser les callbacks `.then()` et Promise chains

Pattern non détecté :
```js
useEffect(() => {
  fetch("/api/user").then((u) => setUser(u))
}, [])
```

`setUser` est dans un `FnLit` passé comme argument — `exec_stmt` ne descend pas dans les corps de FnLit. Fix : lors du CFG analysis, extraire les setters appelés dans les closures immédiates (un niveau de profondeur suffit pour les cas courants).

### Functional updaters `(n) => n + 1` ne déclenchent pas de widening

```js
useEffect(() => {
  setCount(c => c + 1)  // FnLit → Reference(Unstable) → join Top → converge sans widening
}, [])
```

`FnLit` → `Reference(Unstable)` → cross-type join avec `Number([init])` → `Top`. Converge en 2 iter sans déclencher `widened_labels`. Il faut un signal spécifique pour les functional updaters : si l'argument est un `FnLit` dont le corps appelle le setter de façon non-identity → potential infinite loop.

## Infrastructure

### Analyse inter-composants

Analyse intra-procédurale uniquement. État propagé d'un parent vers un enfant via props non tracé. Nécessiterait un graphe de composants + analyse inter-procédurale (scope futur, complexité O(n²)).

### Narrowing sur les branches pour le fixpoint SCC

`if (count < 10) setCount(count + 1)` génère un faux négatif : l'analyse widen mais `unconditionally_calls_setter` retourne false (setter dans une branche). Fix : implémenter narrowing dans `cfg_analyzer` sur les terminators `Branch { cond }` pour restreindre les intervals après les conditions.

### Block-states pour les effect bodies

`AnalysisResult::block_states` ne contient que les états du render CFG. Les règles qui veulent analyser l'intérieur des effect bodies re-calculent manuellement. Ajouter `effect_block_states: HashMap<HookLabel, HashMap<BlockId, AbstractEnv<D>>>`.

---

## Nouvelles règles

### `missing-deps` pour `useCallback` et `useMemo`

Même logique que la règle `missing-deps` existante (stale closure), mais appliquée aux hooks `useCallback` et `useMemo`. Actuellement `effect_info` ne collecte que les `HookEntry::Effect`. Étendre `collect_effect_info` (ou créer `collect_closure_info`) pour couvrir `HookEntry::Callback` et `HookEntry::Memo`.

### `always-unstable-deps`

```js
useEffect(() => { doX() }, [{}])        // {} = Reference(Unstable) chaque render
useEffect(() => { doX() }, [someObj])   // someObj = Reference(Unstable)
```

Deps array présent mais toutes les valeurs évaluées à `is_unstable()` → effect tourne chaque render, équivalent à `deps: None` avec overhead. Détection : évaluer chaque dep expression avec `StateValueTransfer.eval_expr`, vérifier si toutes `is_unstable()`.

### `useState-lazy-init`

```js
const [data, setData] = useState(expensiveCompute())  // recalculé chaque render
// fix : useState(() => expensiveCompute())
```

Si `HookEntry::State { init: Expr::Call { .. } }` → warn. Règle structurelle pure, pas besoin du fixpoint. Faux positifs sur les calls cheap acceptables (sound).

### `derived-state` *(bloqué — dépend du support fonctions)*

```js
const [n, setN] = useState(0);
const [doubled, setDoubled] = useState(0);
useEffect(() => { setDoubled(n * 2) }, [n])  // doubled = f(n) → devrait être useMemo
```

**Définition retenue** : effect dont les deps sont `[stateA, ...]`, qui appelle `setB(expr)` inconditionnellement où `expr` est **call-free** (pas de `Expr::Call` dans les sous-termes) et ne lit que des `StateVal` des labels sources, et où `setB` n'est appelé **nulle part ailleurs** dans le composant (render + autres effects + handlers).

Multi-source supporté (`setSum(a + b)` avec deps `[a, b]`).

**Bloqué par** :

1. **Pas de support fonctions** — `Expr::Call` est opaque. `setFormatted(format(n))` où `format` est pure serait un faux négatif. Pour l'éviter sans exploser les faux positifs (ex. `setUser(fetchUser(id))`), il faudrait soit distinguer sync/async dans l'IR, soit un registre de fonctions pures connues (`Math.abs`, etc.), soit une analyse de corps de fonction.

2. **Functional updaters** — `setB(prev => ...)` est lui aussi un `FnLit` non analysé (voir item ci-dessus). Même prérequis.

Dépend aussi de `effect_block_states` pour inspecter les corps d'effect proprement.

---

## Précision d'analyse (suite)

### `InfiniteLoopRender` émet un seul diagnostic

`check` retourne au premier setter trouvé (`return vec![...]`). Si `setA` et `setB` sont tous les deux appelés dans le render body, un seul est reporté. Fix : collecter tous les setters appelés avant de retourner.

### `missing-deps` manque les stale closures dans les mount-only effects (`deps: Some([])`)

**Corrigé** : `EffectInfo::has_deps_array` distingue maintenant `deps: None` (skip) de `deps: Some([])` (check).
