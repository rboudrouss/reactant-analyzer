# ADR-010 : Heap model — allocation-site abstraction pour la résolution des callbacks par variable

- **Statut** : Accepté
- **Date** : 2026-06-03
- **Contexte** : [ADR-009](ADR-009-callback-traversal.md) (traversée callbacks), [ADR-003](ADR-003-ir-design.md) (IR / FnLit), [ADR-005](ADR-005-analysis-scope.md) (scope intra-procédural)

## Contexte

ADR-009 implémente la descente dans les callbacks inline (`FnLit` passé directement à `.then(cb)`). Deux patterns courants restaient aveugles :

**B5 — callback par variable :**
```js
const cb = () => setN(n + 1);
setTimeout(cb, 1000);  // arg = Identifier, pas FnLit → skippé
```

**B6 — appel direct à une fonction locale :**
```js
async function load() { setUser(data); }
load();  // Call{ fn_: Var("load") } → callee Unknown → skippé
```

Dans les deux cas, le corps de la fonction est inaccessible parce que l'IR ne lie pas les noms de variables aux corps de leurs `FnLit` au moment de l'analyse.

## Décision

### 1. ExprId — identifiant d'allocation

Chaque nœud « allouant » (`FnLit`, `ObjectLit`, `ArrayLit`) reçoit un `id: ExprId` (newtype `struct ExprId(pub usize)`) assigné par un compteur dans `BlockBuilder` au moment du lowering. C'est le *allocation-site* : même nœud syntaxique → même ExprId à travers toutes les itérations du fixpoint.

```rust
// ir/types.rs
pub struct ExprId(pub usize);

// ir/expr.rs
FnLit { id: ExprId, params: Vec<Var>, body_cfg: Arc<CFG> },
ObjectLit { id: ExprId, fields: Vec<(Symbol, Expr)> },
ArrayLit { id: ExprId, elems: Vec<Expr> },
```

`Arc<CFG>` remplace `Box<CFG>` dans `FnLit` pour que le heap stocke un clone bon marché.

### 2. Heap — store par site d'allocation

```rust
// domains/stores/heap.rs
pub enum HeapValue {
    Fn { params: Vec<Var>, body_cfg: Arc<CFG> },
    Obj(HashMap<Symbol, StateValue>),  // réservé — domaine objet futur
    Arr(Vec<StateValue>),              // réservé — domaine tableau futur
}

pub struct Heap(HashMap<ExprId, HeapValue>);
```

Le heap est monotone (insert-only). `join` = union (même site → même corps, valeurs scalaires joinées pour les futurs domaines objet/tableau).

### 3. AbstractEnv — deux maps séparées

`AbstractEnv<D>` maintient désormais :

- `stabs: HashMap<Var, D>` — valeurs abstraites (inchangé sémantiquement)
- `locs: HashMap<Var, HashSet<ExprId>>` — sites d'allocation pour les variables liées à un `FnLit`/`ObjectLit`/`ArrayLit`

Les deux coexistent pour la même variable. `extend(var, val)` touche `stabs`, `extend_loc(var, id)` touche `locs`. `lookup_env_val(var)` renvoie `Some(EnvVal::Loc(ids))` si `locs` contient var, sinon `Some(EnvVal::Val(...))` depuis `stabs`.

**Pourquoi deux maps séparées** : une seule map `EnvVal = Val | Loc` était tentante mais `env.extend(var, val)` écrasait le `Loc` précédemment posé par `env.extend_loc(var, id)`. Les deux maps évitent ce conflict.

### 4. Population du heap

Dans `exec_state_value`, au traitement d'un `Stmt::Let { var, rhs: FnLit{id, params, body_cfg} }` :

```rust
env.extend_loc(var, *id);
heap.insert(*id, HeapValue::Fn { params: params.clone(), body_cfg: Arc::clone(body_cfg) });
// + eval normal → env.extend(var, Reference(Unstable))
```

Le heap est ainsi peuplé dès la première rencontre du `let cb = () => ...` dans l'analyse.

### 5. Transfer trait — heap en paramètre

`heap: &mut Heap` ajouté à `exec_stmt` et `eval_expr` dans le trait `Transfer`. `analyze_cfg` maintient un `heap_out: Heap` local accumulé sur tous les blocs. La fonction retourne `(exit_envs, state_out, heap_out)`.

### 6. B5 — résolution callback par variable

Dans `exec_callbacks_depth`, pour un arg `Expr::Var(name)` quand `class == InCycle` :

```rust
Expr::Var(name) if class == TriggerClass::InCycle => {
    exec_var_callback(name, env, state, memo, heap, depth);
}
```

`exec_var_callback` : `lookup_env_val(name)` → `EnvVal::Loc(ids)` → pour chaque `id` → `heap.get(id)` → `HeapValue::Fn{params, body_cfg}` → `exec_body_depth(body_cfg, sub_env, ..., depth+1)`.

Si `name` n'a pas de `Loc` (variable externe/importée) → skip silencieux → **pas de FP**.

### 7. B6 — inlining des appels locaux directs

Même `exec_var_callback`, déclenchée depuis le traitement d'un `Call` dont le callee est `Unknown` :

```rust
if class == TriggerClass::Unknown {
    if let Expr::Var(name) = fn_.as_ref() {
        exec_var_callback(name, env, state, memo, heap, depth);
    }
}
```

`Unknown + Loc` → inliné. `Unknown + pas de Loc` → skip → conservatif. Distingue naturellement les fonctions locales (Loc dans env) des callees externes (pas de Loc).

### 8. Garde de profondeur

`MAX_INLINE_DEPTH = 3`. La profondeur est propagée à travers `exec_callbacks_depth → exec_var_callback → exec_body_depth → exec_state_value_depth → exec_callbacks_depth`. Si `depth >= MAX_INLINE_DEPTH` → bail immédiat.

**FN connu** : fonctions mutuellement récursives ou callstack plus profonde de 3 niveaux → non descendues.

## Limites connues

- **Back-edge dans un corps de callback** → FN (documenté ADR-009, inchangé).
- **Heap non persisté entre les passes fixpoint** : `analyze_cfg` crée un `Heap::new()` à chaque appel. Si le `let cb = ...` est dans le `render_cfg` et `setTimeout(cb)` dans un `effect_cfg`, le heap du render n'est pas visible dans l'effect. Fix futur : passer le heap accumulé du render dans les effect passes dans `fixpoint.rs`.
- **Domaines objet/tableau** (`HeapValue::Obj`/`Arr`) réservés — non utilisés jusqu'à l'implémentation d'un domaine de champs.
- **Multi-site join** : `locs` peut contenir plusieurs ExprIds pour une même variable (branches ternaires). Tous les corps sont exécutés et leurs effets joints — correct par over-approximation.

## Conséquences

- `src/ir/types.rs` — `ExprId` newtype.
- `src/ir/expr.rs` — `FnLit`/`ObjectLit`/`ArrayLit` struct variants avec `id`.
- `src/lowering/cfg_builder.rs` — `expr_counter` + `next_expr_id()`.
- `src/lowering/expr_lower.rs` — assignation des ids aux 3 sites d'allocation.
- `src/domains/stores/abstract_env.rs` — `locs: HashMap<Var, HashSet<ExprId>>`, `extend_loc`, `lookup_env_val`, join/widen/leq des locs.
- `src/domains/stores/heap.rs` — **nouveau** fichier.
- `src/domains/mod.rs` — `Transfer` étendu avec `heap: &mut Heap`.
- `src/engine/cfg_analyzer.rs` — `heap_out` accumulé, retourné.
- `src/domains/impls/state_value.rs` — `exec_var_callback`, `exec_callbacks_depth`, `exec_body_depth`, `exec_state_value_depth`.
- Blast radius IR : tous les match sur `ObjectLit`/`ArrayLit`/`FnLit` mis à jour (wildcard `{ .. }` ou nommage des champs).
