# Dossier 05 — Fonctions de transfert, interpréteur abstrait des expressions, callbacks, stores

> Matière première pour le manuscrit. État du dépôt : `main` au commit
> `e67b10a` (2026-09-27). Tous les extraits sont verbatim et référencés
> `chemin:Ldébut-Lfin`. Les sorties citées ont été obtenues le 2026-09-28 avec
> le binaire `reactant` (`./target/debug/reactant check …`, construit par
> `cargo build`) et avec une **sonde temporaire** (§6.0) qui affiche les
> valeurs abstraites convergées. Ce qui n'a pas pu être vérifié est marqué
> **« à vérifier »**. Plusieurs **défauts de soundness** ont été observés en
> rédigeant ce dossier (§8.1) : ils sont reproduits, pas corrigés.

Périmètre lu intégralement :

| Fichier | Lignes |
|---|---|
| `src/domains/transfer/mod.rs` | 3 |
| `src/domains/transfer/state_value.rs` | 2749 (production L1-L1041, tests L1042-L2749) |
| `src/domains/interp/mod.rs` | 9 |
| `src/domains/interp/interpreter.rs` | 583 |
| `src/domains/interp/callbacks.rs` | 131 |
| `src/domains/interp/cfg.rs` | 26 |
| `src/domains/stores/mod.rs` | 35 |
| `src/domains/stores/abstract_env.rs` | 366 |
| `src/domains/stores/state_store.rs` | 173 |
| `src/domains/stores/shared_state_store.rs` | 204 |
| `src/domains/stores/heap.rs` | 165 |
| `src/domains/stores/memo_store.rs` | 70 |

ADR lus : ADR-002, ADR-009, ADR-010, ADR-012, ADR-015 (plus ADR-001, ADR-017,
ADR-020 pour le contexte). Suivis dans les modules voisins (lus partiellement,
cités quand utile) : `src/domains/mod.rs` (traits `AbstractDomain`,
`Transfer`), `src/domains/context.rs` (`AnalysisCtx`, `InterCtx`,
`QueryContext`, `FixpointCtx`), `src/domains/impls/state_value.rs` (le
produit `StateValue`, décrit en détail au dossier 04), `src/engine/fixpoint.rs`
(boucle de point fixe, `analyze_program`), `src/engine/cfg_analyzer.rs`
(`analyze_cfg`), `src/engine/eval.rs`, `src/engine/written.rs`,
`src/engine/component_cache.rs`, `src/lowering/expr_lower.rs` (comment les
constructions JS arrivent jusqu'à l'évaluateur), `src/lowering/hook_extractor.rs`
(`extract_handlers`), `src/ir/{expr,stmt,hooks,types,cfg}.rs`,
`src/rules/impls/analysis_limit_info.rs`.

Tests exercés (tous verts au commit `e67b10a`) :

- `cargo test -q --lib domains::transfer` → `44 passed` ;
- `cargo test -q --lib domains::interp` → `4 passed` ;
- `cargo test -q --lib domains::stores` → `30 passed` ;
- tests d'intégration `tests/inter_component.rs` (26 tests),
  `tests/functional_updater.rs` (5), `tests/memo_recompute.rs`,
  `tests/concise_arrow_bodies.rs`, `tests/inline_capture.rs`,
  `tests/allocation_site_identity.rs`, `tests/subscriptions.rs`,
  `tests/lazy_init.rs`, `tests/body_calls.rs` : tous `ok`.

---

## 1. Rôle et position dans le pipeline

### 1.1 Ce que fait le sous-système, en une phrase

Ce sous-système est **l'interpréteur abstrait proprement dit** : étant donné
une expression ou une instruction de l'IR et l'état abstrait courant, il
calcule la valeur abstraite (`StateValue`) de l'expression
(`eval_state_value`) ou fait évoluer l'état (`exec_stmt`) — l'environnement
local (`AbstractEnv`), le store d'état React (`StateStore`), le tas abstrait
par site d'allocation (`Heap`), le store inter-composants
(`SharedStateStore`) — en **descendant dans les callbacks** qui s'exécutent
« dans le cycle » (ADR-009) et en **inlinant les composants enfants**
rencontrés en JSX (ADR-012). Il ne décide pas quand itérer : c'est le moteur
(`src/engine/fixpoint.rs`, `cfg_analyzer.rs`, dossier 06) qui l'appelle bloc
par bloc et itère jusqu'au point fixe.

Trois sous-répertoires, trois rôles :

- `src/domains/transfer/` : la **sémantique des expressions** pour le domaine
  `StateValue` (littéraux, opérateurs, appels, membres, hooks, JSX), plus le
  calcul des mémos (`recompute_memo`) et l'inlining inter-composants
  (`eval_comp_app`).
- `src/domains/interp/` : la **sémantique des instructions** générique en
  `T: Transfer` — liaison `let`/affectation, écriture de champ, appel de
  setter (y compris *functional updater*), pré-passe de callbacks,
  exécution d'un corps de fonction (`exec_body`).
- `src/domains/stores/` : les **structures d'état** (environnement, stores,
  tas) et leurs opérations de treillis.

### 1.2 Place dans la chaîne parse → lowering → IR → engine → rules → driver

```
parse (oxc) ─► lowering (src/lowering/) ─► IR : ComponentIR { render_cfg, hooks, … }
                   │  expr_lower.rs : templates → `+`, ternaires/&&/||/?? → diamants de blocs,
                   │  a?.b → FieldAccess, {...o} → clé synthétique "...N", new → Expr::New
                   ▼
driver ─► resolver ─► engine::analyze_program                     src/engine/fixpoint.rs:704-819
                        │  SharedStateStore::new() (un seul pour tout le programme)   :714
                        │  InterCtx { registry, cache, shared_state, call_graph, … }  :729-741
                        ▼
                      analyze_component_impl(…, inter)                           :130-698
                        ├─ module consts : transfer.eval_expr(Lit)                :164-190
                        ├─ custom_arg_returns : interp::exec_body(…)              :240-277
                        ├─ graine useState : exec_body (lazy init) | eval_expr    :317-353
                        └─ loop {                                                 :355-530
                             analyze_cfg(render_cfg)   ── transfer.exec_stmt ──┐  cfg_analyzer.rs:34-124
                             transfer.recompute_memo(…) → MemoStore::set          │  :388-413
                             analyze_cfg(effets), analyze_cfg(handlers)           │  :416-483
                             new_state = render ⊔ effets ⊔ handlers
                                         ⊔ shared_state.slice(comp)               │  :486-493
                             StateStore::leq / widen_to / changed_labels          │  :495-529
                           }                                                      │
                                                                                  ▼
          ┌──────────────────── ce dossier ───────────────────────────────────────────┐
          │ StateValueTransfer::exec_stmt → interp::exec_stmt_with_callbacks          │
          │    exec_full_stmt : pré-passe exec_callbacks_depth + exec_stmt_core       │
          │    exec_expr_effects : pré-passe + exec_setter_call (+ CompApp)           │
          │    exec_var_callback / exec_body_depth (B5/B6, profondeur ≤ 3)            │
          │ StateValueTransfer::eval_expr → eval_state_value                          │
          │    eval_binop / eval_unary / eval_field_access / eval_index_access        │
          │    eval_comp_app → inter.analyze_child(…) (récursion sur l'enfant)        │
          │ stores : AbstractEnv, StateStore, MemoStore, SharedStateStore, Heap       │
          └────────────────────────────────────────────────────────────────────────────┘
                                                                                  │
AnalysisResult { state_store, memo_store, block_states, heap, … } ────────────────┘
      │
      ├─ relations post-point-fixe (engine/written.rs, triggers, seeds…) :
      │     StateValueTransfer.eval_expr / exec_stmt rejoués sur les stores convergés
      │     (engine/eval.rs:31-47 `eval_in_stores`, engine/written.rs:233-274)
      ▼
rules (src/rules/) : lisent StateValue, MemoStore, SharedStateStore (infinite_loop.rs:198),
                     AnalysisStats (analysis_limit_info.rs:53-88)
```

### 1.3 Qui appelle qui — points d'entrée exacts

Le seul `Transfer` concret est `StateValueTransfer`
(`src/domains/transfer/state_value.rs:24-102`). Le moteur est générique en
`T: Transfer<Domain = StateValue>` (`src/engine/fixpoint.rs:90`, `:130`) et
`analyze_cfg` en `T: Transfer` (`src/engine/cfg_analyzer.rs:34`).

| Appelant | Appelé (ce dossier) | Où |
|---|---|---|
| `analyze_cfg` (chaque instruction d'un bloc) | `transfer.exec_stmt(stmt, &mut env_out, &mut ac)` | `src/engine/cfg_analyzer.rs:77-79` |
| `analyze_cfg` (terminateur `Return(e)`) | `transfer.exec_expr_effects(return_expr, …)` | `src/engine/cfg_analyzer.rs:84-86` |
| amorçage des constantes de module | `transfer.eval_expr(&Expr::Lit(p.clone()), …)` | `src/engine/fixpoint.rs:173-175` |
| verdicts de retour des arguments `FnLit` de hooks custom | `crate::domains::interp::exec_body(…)` | `src/engine/fixpoint.rs:272` |
| graine `useState(() => e)` (initialiseur paresseux) | `crate::domains::interp::exec_body(…)` | `src/engine/fixpoint.rs:342-346` |
| graine `useState(e)` | `transfer.eval_expr(init, &init_env, &mut ac)` | `src/engine/fixpoint.rs:347` |
| rafraîchissement des mémos après chaque passe render | `transfer.recompute_memo(comp_id, deps, &env_exit, &mut memo_ctx)` | `src/engine/fixpoint.rs:399-410` |
| convergence | `StateStore::join`, `leq`, `widen`, `widen_to`, `changed_labels`, `SharedStateStore::slice` | `src/engine/fixpoint.rs:486-529` |
| sondes post-point-fixe | `StateValueTransfer.eval_expr(…)` | `src/engine/eval.rs:41`, `src/engine/written.rs:245` |
| rejeu du préfixe d'un bloc | `StateValueTransfer.exec_stmt(s, &mut env, ac)` | `src/engine/written.rs:270` |
| `eval_comp_app` (inlining d'un enfant) | `inter.analyze_child` = `analyze_component_inter` | `state_value.rs:579-580`, `fixpoint.rs:65-81`, `:739` |

Ré-exports publics (`src/domains/mod.rs:7-10`, `src/domains/stores/mod.rs:9-13`,
`src/domains/interp/mod.rs:1-9`, `src/domains/transfer/mod.rs:1-3`) :
`StateValueTransfer`, `AbstractEnv`, `EnvVal`, `Heap`, `HeapValue`,
`resolve_locs`, `MemoStore`, `SharedStateStore`, `StateStore`, `TriggerClass`,
`classify_callee`, `MAX_INLINE_DEPTH` ; en `pub(crate)` : `exec_body`,
`exec_expr_effects`, `exec_stmt_with_callbacks`.

### 1.4 Ce qui entre, ce qui sort

- **Entrée** : des `Expr` et `Stmt` de l'IR (`src/ir/expr.rs:175-300`,
  `src/ir/stmt.rs:8-29`), un `AbstractEnv<StateValue>` et un
  `AnalysisCtx<StateValue>` qui regroupe `state`, `memo`, `heap`, `query` et
  l'`inter` optionnel (`src/domains/context.rs:112-125`).
- **Sortie** : une `StateValue` (pour `eval_expr`, `recompute_memo`,
  `exec_body`) ou des **mutations en place** : `env` (liaisons), `ctx.state`
  (mises à jour faibles par `update` = join), `ctx.heap` (insertions),
  `inter.shared_state` (mises à jour faibles), `inter.stats`,
  `inter.cache`, `inter.results`, `inter.call_graph`.

Propriété de conception centrale (ADR-009 §4) : **`eval_state_value` est
« pure » vis-à-vis de l'état React** — elle ne met jamais à jour
`ctx.state` elle-même, sauf deux exceptions assumées : `havoc_setter_props`
(join de ⊤ dans les slots dont le setter s'échappe vers un enfant inconnu) et
`eval_comp_app` (qui lance l'analyse d'un enfant, lequel écrit dans le
`SharedStateStore`). Les effets de bord ordinaires (appel de setter,
callbacks) sont tirés par l'interpréteur (`interp/interpreter.rs`), dans des
positions syntaxiques précises (§4.3.2).

---

## 2. Inventaire des fichiers du périmètre

### 2.1 `src/domains/transfer/mod.rs` (3 lignes)

```rust
mod state_value;

pub use state_value::StateValueTransfer;
```
(`src/domains/transfer/mod.rs:1-3`)

Rôle : façade. Seul `StateValueTransfer` est public ; toutes les fonctions
d'évaluation (`eval_state_value`, `eval_binop`, …) sont privées au module.

### 2.2 `src/domains/transfer/state_value.rs` (2749 lignes)

Rôle : fonctions de transfert du domaine `StateValue`. Production L1-L1041,
tests L1042-L2749 (44 tests). Types publics : `pub struct StateValueTransfer;`
(`:24`). Tout le reste est privé.

| Fonction | Lignes | Rôle |
|---|---|---|
| `impl Transfer for StateValueTransfer` | 26-102 | `eval_expr` → `eval_state_value` ; `exec_stmt` → `exec_stmt_with_callbacks` ; `exec_expr_effects` ; `recompute_memo` |
| `eval_state_value` | 106-190 | dispatch par variante d'`Expr` |
| `summary_value` | 192-222 | table `SummaryValue` → `StateValue` |
| `returns_fresh_reference` | 224-259 | appels dont le résultat est une allocation fraîche |
| `havoc_setter_props` | 261-297 | ⊤ dans les slots dont le setter part vers un enfant inanalysable |
| `collect_escaping_setters` | 299-367 | setters atteignables depuis une valeur de prop |
| `setter_calls_in_cfg` / `setter_calls_in_expr` / `callee_setter` | 369-467 | parcours des corps de fonctions à la recherche d'appels de setter |
| `eval_comp_app` | 469-594 | inlining d'un composant enfant (ADR-012) |
| `eval_field_access` | 596-628 | lecture de membre via le tas |
| `eval_index_access` | 630-651 | index constant = membre (`"0"`, `"1"` …) |
| `eval_props_map` | 653-689 | props → `HashMap<Symbol, EnvVal>` |
| `record_call_site` | 691-706 | arête du graphe d'appels |
| `as_arith` / `as_str_only` | 708-750 | vues numérique (`ToNumber`) / chaîne d'un opérande |
| `eval_binop` | 752-811 | opérateurs binaires |
| constantes et helpers bitwise | 813-844 | `I32_MIN`, `int32_range`, `uint32_range`, `const_of`, `shift_amount` |
| `eval_bitwise` | 846-929 | `&`, `|`, `^`, `<<`, `>>`, `>>>` |
| `all_ones_above` | 931-939 | plus petit `2^n − 1 ≥ m` |
| `eval_unary` | 941-1004 | `-`, `!`, `typeof`, `~`, `+`, inconnu |
| `coerce_to_number` | 1006-1040 | `ToNumber` des booléens et chaînes connues |

Dépendances internes (`:5-20`) : `domains::{AbstractDomain, AnalysisCtx,
Transfer}`, `domains::impls::{BoolVal, Interval, SetterVal, Stability,
StateValue, StrConst}`, `domains::interp::{exec_expr_effects,
exec_stmt_with_callbacks}`, `domains::stores::{AbstractEnv, EnvVal, Heap,
HeapValue, resolve_locs}`, `engine::component_registry::ChildLookup`,
`ir::{ComponentId, expr::{BinOp, CompOrigin, Expr, MarkerVal, Prim, UnaryOp},
hooks::{Arity, DepsArg}, stmt::Stmt, types::{ExprId, Symbol}}`. Noter la
dépendance **montante** vers `engine::component_registry` et, via
`InterCtx`, vers `engine::fixpoint` : la boucle est cassée par le pointeur de
fonction `AnalyzeChildFn` (`src/domains/context.rs:17-25`).

### 2.3 `src/domains/interp/mod.rs` (9 lignes)

```rust
mod callbacks;
mod cfg;
mod interpreter;

pub use callbacks::{TriggerClass, classify_callee};
pub use interpreter::MAX_INLINE_DEPTH;
pub(crate) use interpreter::exec_body;
pub(crate) use interpreter::exec_expr_effects;
pub(crate) use interpreter::exec_stmt_with_callbacks;
```
(`src/domains/interp/mod.rs:1-9`)

### 2.4 `src/domains/interp/interpreter.rs` (583 lignes, aucun test local)

Rôle : sémantique des instructions, générique en `T: Transfer`. Pas de bloc
`#[cfg(test)]` : il est testé à travers `StateValueTransfer` dans
`transfer/state_value.rs` (tests L1449-L2748).

| Élément | Lignes | Visibilité |
|---|---|---|
| `pub const MAX_INLINE_DEPTH: usize = 3;` | 21 | `pub` |
| `exec_stmt_with_callbacks` | 29-36 | `pub(crate)` |
| `exec_body` | 42-49 | `pub(crate)` |
| `exec_body_depth` | 53-61 | `pub(crate)` (non ré-exporté hors `interp`) |
| `exec_full_stmt` | 69-99 | privé |
| `exec_expr_effects` | 114-126 | `pub(crate)` |
| `exec_stmt_core` | 136-206 | privé |
| `bind_rhs` | 214-292 | privé |
| `obj_members` | 301-333 | privé |
| `exec_setter_call` | 341-392 | privé |
| `exec_body_impl` | 398-469 | privé |
| `exec_callbacks_depth` | 477-540 | privé |
| `exec_var_callback` | 544-583 | privé |

Dépendances (`:4-19`) : `domains::{AbstractDomain, AnalysisCtx, Transfer,
impls::StateValue, stores::{AbstractEnv, EnvVal, HeapValue, resolve_locs}}`,
`ir::{cfg::{CFG, EdgeKind, Terminator}, expr::{Expr, MarkerVal,
SummaryValue}, stmt::{MemberKey, Stmt}, types::{BlockId, Symbol}}`,
`super::callbacks`, `super::cfg::topo_sort`.

### 2.5 `src/domains/interp/callbacks.rs` (131 lignes, 4 tests)

Rôle : classification d'un *callee* (`TriggerClass`, `classify_callee`),
cœur de la politique d'ADR-009. Types publics : `enum TriggerClass`
(`:7-23`), `fn classify_callee` (`:28-56`). Tests `:58-131`.

### 2.6 `src/domains/interp/cfg.rs` (26 lignes)

Rôle : `topo_sort` (`:5-11`) et `dfs_post` (`:13-26`), un tri topologique par
post-ordre inversé d'un DFS depuis `cfg.entry`, `pub(super)`. Utilisé par
`exec_body_impl` uniquement.

```rust
pub(super) fn topo_sort(cfg: &CFG) -> Vec<BlockId> {
    let mut visited: HashSet<BlockId> = HashSet::new();
    let mut order: Vec<BlockId> = Vec::new();
    dfs_post(cfg.entry, cfg, &mut visited, &mut order);
    order.reverse();
    order
}
```
(`src/domains/interp/cfg.rs:5-11`)

`dfs_post` (`:13-26`) marque le bloc visité (`visited.insert`), descend
récursivement dans `cfg.successors(bid)` puis empile le bloc (post-ordre).
La récursion est native (pile Rust) : profondeur bornée par la longueur du
plus long chemin simple du CFG d'un corps de fonction.

Remarque : sur un CFG avec arcs retour, le post-ordre inversé place l'en-tête
de boucle avant le corps (les arcs retour sont ignorés), ce qui est
l'hypothèse d'ADR-009 (« `topo_sort` emitting the header before its back-edge
source »). Les blocs inatteignables depuis l'entrée ne sont jamais visités.

### 2.7 `src/domains/stores/mod.rs` (35 lignes)

Déclare les cinq sous-modules et deux helpers partagés :

```rust
/// `a ⊑ b` in a (possibly partial) lattice order: `Less`/`Equal` hold,
/// incomparable/`Greater` do not. Centralises the pointwise ≤ check the store
/// `leq`s share, spelled with `partial_cmp` (the form clippy prefers over a
/// negated comparison operator).
pub(crate) fn leq_pointwise<D: AbstractDomain>(a: &D, b: &D) -> bool {
    matches!(a.partial_cmp(b), Some(Ordering::Less | Ordering::Equal))
}

/// `m[k]` cloned, or `default()` when the key is absent — the get-or-default
/// lattice read every store performs (`⊥` or `⊤` for an unseen key).
pub(crate) fn map_get_or<K: Eq + Hash, D: Clone>(
    m: &HashMap<K, D>,
    k: &K,
    default: impl Fn() -> D,
) -> D {
    m.get(k).cloned().unwrap_or_else(default)
}
```
(`src/domains/stores/mod.rs:19-35`)

### 2.8 `src/domains/stores/abstract_env.rs` (366 lignes, 13 tests)

Types publics : `enum EnvVal<D>` (`:15-27`), `struct AbstractEnv<D>`
(`:51-60`). Méthodes : `as_val`, `locs` (`EnvVal`) ; `new`, `lookup`,
`lookup_env_val`, `contains`, `extend`, `extend_loc`, `bind_setter`,
`setter_label`, `bind_callback`, `callback_label`, `join`, `widen_to`,
`bottom`, `leq` (`AbstractEnv`). Tests `:244-366`.

### 2.9 `src/domains/stores/state_store.rs` (173 lignes, 8 tests)

Type public : `struct StateStore<D>(HashMap<HookLabel, D>)` (`:10-11`).
Méthodes : `new`, `get` (⊥ par défaut), `update` (join), `join`, `widen`,
`widen_to`, `bottom`, `leq`, `labels` (triés), `changed_labels`.

### 2.10 `src/domains/stores/shared_state_store.rs` (204 lignes, 6 tests)

Type public : `struct SharedStateStore { entries: HashMap<(ComponentId,
HookLabel), StateValue> }` (`:14-17`) — **non générique**, fixé à
`StateValue`. Méthodes : `new`, `get`, `update`, `join`, `leq`, `slice`.

### 2.11 `src/domains/stores/heap.rs` (165 lignes, aucun test local)

Types publics : `enum HeapValue { Fn { … }, Obj(…) }` (`:17-29`),
`struct Heap(HashMap<ExprId, HeapValue>)` (`:31-34`), `fn resolve_locs`
(`:134-165`). Méthodes : `new` (`:37-39`), `insert` (`:41-43`), `alloc_fn`
(`:45-70`), `get` (`:72-74`), `get_mut` (`:76-78`), `join` (`:80-119`),
`widen` (`:121-125`), `leq` (`:127-131`). Testé indirectement (`tests/inter_component.rs:381-462`,
`tests/allocation_site_identity.rs`).

### 2.12 `src/domains/stores/memo_store.rs` (70 lignes, 3 tests)

Type public : `struct MemoStore<D>(HashMap<HookLabel, D>)` (`:13-14`).
Méthodes : `new`, `get` (**⊤** par défaut), `set` (écrasement).

### 2.13 Tableau de synthèse des dépendances

```
transfer/state_value.rs ──► interp::{exec_expr_effects, exec_stmt_with_callbacks}
        │                ──► stores::{AbstractEnv, EnvVal, Heap, HeapValue, resolve_locs}
        │                ──► impls::{StateValue, Stability, Interval, StrConst, BoolVal, SetterVal}
        │                ──► engine::component_registry::ChildLookup   (dépendance montante)
        │                ──► context::InterCtx (analyze_child, cache, results, shared_state…)
interp/interpreter.rs ──► Transfer (trait, générique) ; stores ; impls::StateValue
interp/callbacks.rs   ──► stores::AbstractEnv (setter_label)
stores/heap.rs        ──► ir::free_vars::compute_free_vars ; impls::StateValue
stores/shared_state_store.rs ──► stores::StateStore ; impls::StateValue ; ir::ComponentId
```

---

## 3. Types et structures centraux

### 3.1 Le trait `Transfer` (rappel, défini hors périmètre)

```rust
pub trait Transfer {
    type Domain: AbstractDomain;

    /// Abstract evaluation of an expression in the current abstract state.
    fn eval_expr(
        &self,
        expr: &Expr,
        env: &AbstractEnv<Self::Domain>,
        ctx: &mut context::AnalysisCtx<Self::Domain>,
    ) -> Self::Domain;

    /// Execute a statement, updating `env`, `state`, `memo`, and `heap` in place.
    fn exec_stmt(
        &self,
        stmt: &Stmt,
        env: &mut AbstractEnv<Self::Domain>,
        ctx: &mut context::AnalysisCtx<Self::Domain>,
    );

    /// Fire the side effects of `expr` evaluated in *effect position* — a bare
    /// `expr;` statement or a concise-arrow `Return` body: the callback
    /// pre-pass, the setter-call weak-update, and the inter-component eval a
    /// component application performs. This is the single definition of
    /// "run an expression for its effects"; the fixpoint engine's `Return`
    /// handling calls it instead of fabricating a throwaway `Stmt::ExprStmt`.
    fn exec_expr_effects(
        &self,
        expr: &Expr,
        env: &mut AbstractEnv<Self::Domain>,
        ctx: &mut context::AnalysisCtx<Self::Domain>,
    );
```
(`src/domains/mod.rs:114-144`)

Suivi de `recompute_memo` (`src/domains/mod.rs:146-161`), dont la doc exige :
« A deps argument the engine could not read bounds nothing: it must not share
an answer with a written `[]`, which pins the memo forever. »

Deux points d'architecture :

1. `env` est passé **à part** de l'`AnalysisCtx` parce que sa mutabilité
   diffère entre `eval_expr` (`&`) et `exec_stmt` (`&mut`)
   (`src/domains/context.rs:107-111`).
2. `AbstractDomain` porte deux ponts `as_state_value` / `from_state_value`
   (`src/domains/mod.rs:40-53`) : le tas (`Heap`) et le `SharedStateStore`
   stockent des `StateValue` concrètes, quel que soit `T::Domain`. Pour tout
   domaine autre que `StateValue`, `from_state_value` rend ⊥ et
   `as_state_value` rend `None` — c'est-à-dire que l'interpréteur générique
   **n'est réellement complet que pour `StateValue`** (les captures de
   fermeture et les écritures de champs sont silencieusement perdues sinon :
   `interpreter.rs:168-170`, `heap.rs:58-61`).

### 3.2 `StateValueTransfer`

```rust
pub struct StateValueTransfer;

impl Transfer for StateValueTransfer {
    type Domain = StateValue;

    fn eval_expr(
        &self,
        expr: &Expr,
        env: &AbstractEnv<StateValue>,
        ctx: &mut AnalysisCtx<StateValue>,
    ) -> StateValue {
        eval_state_value(expr, env, ctx)
    }

    fn exec_stmt(
        &self,
        stmt: &Stmt,
        env: &mut AbstractEnv<StateValue>,
        ctx: &mut AnalysisCtx<StateValue>,
    ) {
        exec_stmt_with_callbacks(self, stmt, env, ctx);
    }

    fn exec_expr_effects(
        &self,
        expr: &Expr,
        env: &mut AbstractEnv<StateValue>,
        ctx: &mut AnalysisCtx<StateValue>,
    ) {
        exec_expr_effects(self, expr, env, ctx, 0);
    }
```
(`src/domains/transfer/state_value.rs:24-54`)

Structure unitaire sans état : toute la variation vient de l'`AnalysisCtx`.
L'exécution des instructions est **entièrement déléguée** à l'interpréteur
générique ; seule l'évaluation des expressions est spécifique au domaine.

### 3.3 `AnalysisCtx`, `InterCtx`, `QueryContext` (rappel, `src/domains/context.rs`)

```rust
pub struct AnalysisCtx<'a, D: AbstractDomain> {
    /// The component under analysis. An analysis is always the analysis of
    /// SOME component — intra or inter — so this is not an `Option`:
    /// state-slot provenance (`Versioned` labels, `SetterVal`) always carries
    /// the real owner. A hand-built IR analysed on its own carries
    /// [`ComponentId::SYNTHETIC`].
    pub component: ComponentId,
    pub state: &'a mut StateStore<D>,
    pub memo: &'a mut MemoStore<D>,
    pub heap: &'a mut Heap,
    pub query: &'a dyn QueryContext,
    /// Optional inter-component context. `None` = intra-component analysis only.
    pub inter: Option<&'a InterCtx<'a>>,
}
```
(`src/domains/context.rs:112-125`)

- `component` : estampille les `Versioned({(component, label)})` produits à
  la lecture d'un état et les `SetterVal::One(component, label)` produits par
  `StateSetter`.
- `state` : store d'état **de la passe courante** (dans `analyze_cfg`, c'est
  `state_out`, une copie accumulant les écritures de la passe :
  `cfg_analyzer.rs:49`, `:69-76`).
- `memo` : dans `analyze_cfg`, une **copie locale jetée** à chaque bloc
  (`let mut memo_local = memo.clone();` avec le commentaire « Memo is fixed
  for this pass; local mutations are discarded », `cfg_analyzer.rs:65-66`).
- `heap` : le tas unique du composant, partagé entre toutes les passes
  (render, effets, handlers) et toutes les itérations (`fixpoint.rs:301`).
- `query` : seule requête utilisée ici, `callback_body(label)` (corps d'un
  `useCallback`), servie par `FixpointCtx` (`context.rs:154-171`) ;
  `NullCtx` répond `None`.
- `inter` : présent pour toute analyse lancée par `analyze_program` en phase 1
  (racines et enfants inlinés), absent pour `analyze_component` (tests) et
  pour la phase 2 (`fixpoint.rs:782-790`) et le rafraîchissement
  post-convergence (`fixpoint.rs:542-563`, `inter` = `None`).

`InterCtx` (`context.rs:59-77`) regroupe le `registry`, le `cache`
(`RefCell<ComponentCache>`), le `shared_state` (`RefCell<SharedStateStore>`),
le `call_graph`, les `stats`, la table `results`, la pile `call_stack`, le
`component` courant, la `config`, le pointeur `analyze_child` et le
`hook_registry`. `child()` (`:82-98`) clone la pile et y pousse le parent ;
`is_recursive(id)` (`:100-102`) teste la pile **et** le composant courant.

### 3.4 `EnvVal<D>` — valeur ou valeur + sites

```rust
/// An entry in the abstract environment.
///
/// Variables bound to locally-defined function/object/array literals carry
/// a `Loc` with the set of allocation-site `ExprId`s they may point to.
/// All other variables carry a `Val` with the standard domain value.
#[derive(Debug, Clone, PartialEq)]
pub enum EnvVal<D> {
    Val(D),
    /// A reference the analysis can chase: the allocation sites it may point
    /// to *and* the value the domain computed for it. A location and a value
    /// are not alternatives — answering ⊤ for the value of a `Loc` is what
    /// made a function or object prop read as "unknown" in the child instead
    /// of the per-render reference the parent had already proved it to be.
    Loc {
        ids: HashSet<ExprId>,
        val: D,
    },
}
```
(`src/domains/stores/abstract_env.rs:10-27`)

- `Val(d)` : une valeur abstraite, sans rien à « poursuivre » dans le tas.
- `Loc { ids, val }` : l'ensemble des **sites d'allocation** (`ExprId`) que la
  variable peut désigner *et* sa valeur abstraite. Invariant documenté : un
  `Loc` n'est pas une alternative à la valeur (historiquement, un `Loc`
  rendait ⊤ comme valeur ; le commentaire en garde la trace).
- `as_val()` (`:31-35`) rend la valeur quelle que soit la variante ;
  `locs()` (`:38-43`) rend `Some(ids)` seulement pour `Loc`.

`EnvVal` n'est **pas** le stockage interne de l'environnement (qui a deux
tables séparées, §3.5) : c'est la **vue** rendue par `lookup_env_val`, et le
type des champs d'un objet du tas (`HeapValue::Obj`).

### 3.5 `AbstractEnv<D>` — environnement local

```rust
/// Per-variable abstract environment.
///
/// `lookup` returns `D::top()` for unbound vars. `setter_bindings` is a
/// React-specific side-channel for setState. `locs` tracks heap allocation-site
/// `ExprId`s for FnLit/ObjectLit/ArrayLit vars; independent from `stabs`.
#[derive(Debug, Clone, PartialEq)]
pub struct AbstractEnv<D: AbstractDomain> {
    stabs: HashMap<Var, D>,
    locs: HashMap<Var, HashSet<ExprId>>,
    setter_bindings: HashMap<Var, HookLabel>,
    /// Side-channel for `useCallback` bindings: the body lives in the hook
    /// entry (`QueryContext::callback_body`), not the heap, so calls through
    /// the variable can still be executed for side effects.
    callback_bindings: HashMap<Var, HookLabel>,
}
```
(`src/domains/stores/abstract_env.rs:46-60`)

Rôle des quatre tables (toutes privées) :

| Champ | Contenu | Écrit par | Lu par |
|---|---|---|---|
| `stabs` | valeur abstraite par variable (le nom est un vestige du domaine `Stability` d'ADR-002) | `extend` | `lookup`, `lookup_env_val`, `contains` |
| `locs` | sites d'allocation possibles | `extend_loc` (union) | `lookup_env_val` |
| `setter_bindings` | variable → label du `useState` dont elle est le setter | `bind_setter` | `setter_label` (→ `classify_callee`, `exec_setter_call`) |
| `callback_bindings` | variable → label du `useCallback` | `bind_callback` | `callback_label` (→ `exec_var_callback`) |

Pourquoi deux tables `stabs`/`locs` plutôt qu'une `HashMap<Var, EnvVal>` ?
ADR-010 §3 : « a single map `EnvVal = Val | Loc` was tempting but
`env.extend(var, val)` overwrote the `Loc` previously placed by
`env.extend_loc(var, id)`. The two maps avoid this conflict. »

Lecture :

```rust
    /// Conservative lookup: `D::top()` for any variable not in the map.
    pub fn lookup(&self, var: &str) -> D {
        self.stabs.get(var).cloned().unwrap_or_else(D::top)
    }

    /// Returns heap location set for `var` if it was bound to an allocating expr.
    /// Returns `None` when the variable has no Loc (external/imported function).
    pub fn lookup_env_val(&self, var: &str) -> Option<EnvVal<D>> {
        if let Some(ids) = self.locs.get(var) {
            return Some(EnvVal::Loc {
                ids: ids.clone(),
                val: self.lookup(var),
            });
        }
        self.stabs.get(var).map(|v| EnvVal::Val(v.clone()))
    }
```
(`src/domains/stores/abstract_env.rs:78-93`)

**Invariant de soundness n° 1** : une variable inconnue vaut ⊤ (`lookup`). Les
paramètres de fonctions, les imports, les globales (`fetch`, `window`) et
toute liaison perdue lisent ⊤. Conséquence importante (et source de
défauts, §8.1) : perdre une liaison de *valeur* est sound (⊤), perdre une
liaison de *setter*, de *callback* ou de *site* ne l'est pas, parce que ces
tables-là ne se lisent pas « ⊤ par défaut » mais « rien par défaut ».

Écriture : `extend` écrase la valeur mais **préserve** les `locs` ;
`extend_loc` **ajoute** un site (`.or_default().insert(id)`, `:106-108`) —
les sites ne sont jamais retirés, même quand la variable est réaffectée à
autre chose (voir §8.2).

Join (`:130-173`) :

```rust
    fn join_stabs(a: &HashMap<Var, D>, b: &HashMap<Var, D>) -> HashMap<Var, D> {
        let mut out = HashMap::new();
        for (k, v) in a {
            let w = map_get_or(b, k, D::top);
            out.insert(k.clone(), v.join(&w));
        }
        for k in b.keys() {
            if !a.contains_key(k) {
                out.insert(k.clone(), D::top());
            }
        }
        out
    }
```
(`src/domains/stores/abstract_env.rs:130-142`)

- variable présente des deux côtés : join ponctuel ;
- variable présente d'un seul côté : **⊤** (cohérent avec « absente = ⊤ ») ;
- `locs` : union ensembliste par variable (`join_locs`, `:144-153`) ;
- `setter_bindings`, `callback_bindings` : union avec **priorité à gauche**
  (`entry(k).or_insert(v)`, `:159-166`). Deux labels différents pour une même
  variable (`s = c ? setA : setB`) ne sont pas représentables : le second
  est ignoré. Impact vérifié à la relecture : `const s = c ? setA : setB;
  s(1)` n'écrit que dans le slot de `setA` (D10, §8.2 et §6.9 (j)).

Subtilité (relecture) : `AbstractEnv::bottom()` (`:211-213`) est
l'environnement **vide**, qui n'est *pas* l'élément neutre du join. Comme une
clé présente d'un seul côté donne ⊤, `bottom().join(&e)` rend un
environnement où **toutes** les variables de `e` valent ⊤ (les `locs` et les
liaisons, elles, sont conservées par union). `bottom()` est bien le plus
petit élément pour `leq` (`leq_bottom_leq_anything`), mais `join` n'est donc
pas la borne supérieure de cet ordre : l'environnement vide se comporte en
lecture comme « tout ⊤ ». Les appelants évitent ce piège en ne joignant que
des environnements effectivement calculés (`exec_body_impl` filtre les
prédécesseurs sans entrée, `interpreter.rs:420-425` ; `analyze_cfg` insère
le premier environnement reçu sans join, `cfg_analyzer.rs:97-98`) ; un bloc
sans aucun prédécesseur calculé part de `bottom()`, où tout se lit ⊤ (sound).

Widening (`:175-208`) : `widen_to` ponctuel sur `stabs` avec les seuils
(ADR-014), join pour le reste. Appelé par `analyze_cfg` sur arc retour
(`cfg_analyzer.rs:100-107`).

Ordre (`:215-239`) : `leq` compare `stabs` avec **⊥** comme valeur par défaut
côté droit (`map_get_or(&other.stabs, k, D::bottom)`) et vérifie l'inclusion
des `locs`. Il y a donc une **asymétrie** avec `lookup`/`join` (absent = ⊤) ;
elle est sans conséquence aujourd'hui car `AbstractEnv::leq` n'est appelé
qu'en tests (`abstract_env.rs:303,310,317`) : `analyze_cfg` compare les
environnements par `PartialEq` (`cfg_analyzer.rs:114`).

Tests (`:251-365`) : `lookup_missing_returns_top`, `join_shared_keys_pointwise`
(`Stable ⊔ PerRender = Unknown`), `join_single_side_key_is_top`,
`join_merges_setter_bindings`, `leq_*`, `extend_loc_*`, `join_locs_unions_sets`,
`loc_lookup_returns_top_val`.

### 3.6 `StateStore<D>` — l'état React, sujet du point fixe

```rust
/// Maps each `useState` / `useReducer` hook label to the current abstract value
/// of its state.  Starts at `D::bottom()` and is refined by detected setter
/// calls during the worklist analysis.
#[derive(Debug, Clone, PartialEq)]
pub struct StateStore<D: AbstractDomain>(HashMap<HookLabel, D>);
```
(`src/domains/stores/state_store.rs:7-11`)

```rust
    /// Returns `D::bottom()` for labels not yet updated by any setter call.
    pub fn get(&self, label: HookLabel) -> D {
        map_get_or(&self.0, &label, D::bottom)
    }

    /// Monotone update: `self[label] = self[label] ⊔ val`.
    pub fn update(&mut self, label: HookLabel, val: D) {
        let current = self.get(label);
        self.0.insert(label, current.join(&val));
    }

    /// Pointwise merge over the union of keys, absent keys read as `D::bottom`.
    /// The per-key combinator `f` is the only thing `join`/`widen`/`widen_to`
    /// differ by.
    fn merge_with(&self, other: &Self, f: impl Fn(&D, &D) -> D) -> Self {
        let mut out = self.0.clone();
        for (&k, v) in &other.0 {
            let cur = map_get_or(&out, &k, D::bottom);
            out.insert(k, f(&cur, v));
        }
        StateStore(out)
    }
```
(`src/domains/stores/state_store.rs:24-45`)

- **⊥ par défaut** (un label jamais écrit est « impossible ») — à l'inverse
  de l'environnement et du `MemoStore`. C'est correct parce que tout label
  d'état est **amorcé** avec la valeur de son initialiseur avant la boucle
  (`fixpoint.rs:317-353`, `state.update(*label, init_val)`).
- `update` est une **mise à jour faible** (*weak update*, join) : un appel de
  setter n'écrase jamais, il ajoute un comportement possible. C'est la
  sémantique *may* : l'appel **peut** avoir lieu (ADR-009 « Weak-update:
  internal setters call `state.update`, already a monotone join → correct
  "may run" semantics »). Conséquence : le store contient le **join de toutes
  les valeurs écrites, initialiseur compris** — la « vue événement »
  d'ADR-017 (§4.1.2).
- `labels()` (`:80-84`) trie : l'ordre des labels est déterministe.
- `changed_labels` (`:87-95`) : labels dont la valeur diffère — la source du
  signal `widen_trace` (`fixpoint.rs:518`).

Tests (`:107-172`, 8 tests) : `get_unknown_label_is_bottom`,
`update_from_bottom_sets_value`, `update_is_monotone_join`,
`join_two_stores`, `leq_bottom_is_least`, `leq_self_true`,
`changed_labels_detects_differences`, `widen_equals_join` (vrai pour
`Stability`, faux pour les intervalles).

Remarques (relecture) : `leq` (`:69-77`) ne parcourt que les clés de `self`
(une clé absente de `self` vaut ⊥, toujours ⊑) et lit `other` avec ⊥ par
défaut ; il est donc cohérent avec `get`. `changed_labels` (`:86-95`)
compare par `!=` (égalité structurelle de `D`), pas par l'ordre : un label
qui a seulement *décru* serait aussi listé (cas impossible dans la boucle,
où `new_state` contient toujours l'ancien état puisque `state_out` part de
`state.clone()`, `cfg_analyzer.rs:49`).

### 3.7 `MemoStore<D>` — valeurs dérivées, sans point fixe propre

```rust
/// Maps each `useMemo` / `useCallback` hook label to its current abstract value.
///
/// Unlike `StateStore`, this store is NOT a fixpoint subject it is fully
/// recomputed via `Transfer::recompute_memo` after each render-pass analysis.
/// The `set` method is the only mutation; the recomputation logic lives in the
/// `Transfer` implementation so each domain can define its own semantics.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoStore<D: AbstractDomain>(HashMap<HookLabel, D>);
```
(`src/domains/stores/memo_store.rs:7-14`)

```rust
    /// Returns `D::top()` for labels not yet computed (conservative).
    pub fn get(&self, label: HookLabel) -> D {
        map_get_or(&self.0, &label, D::top)
    }

    /// Store a precomputed domain value for a label.
    pub fn set(&mut self, label: HookLabel, val: D) {
        self.0.insert(label, val);
    }
```
(`src/domains/stores/memo_store.rs:27-35`)

- **⊤ par défaut** : un mémo non encore calculé peut valoir n'importe quoi.
- `set` **écrase** (mise à jour forte) : le mémo est recalculé en entier
  après chaque passe render, à partir de l'environnement de sortie
  (`fixpoint.rs:388-413`). Le point fixe du `StateStore` fait converger les
  chaînes mémo → mémo « across iterations » (`fixpoint.rs:385-387`).
- La valeur stockée n'est **pas** la valeur calculée par le corps du mémo mais
  une *référence* dont la stabilité est le join des stabilités des deps
  (§4.4) : `useMemo(() => n * 2, [n])` vaut `ref(…)`, jamais un intervalle.
- Les deux hooks `useMemo` **et** `useCallback` y vivent (`Expr::MemoVal(l)`
  et `Expr::CallbackVal(l)` lisent tous deux `ctx.memo.get(l)`,
  `state_value.rs:136`).

### 3.8 `SharedStateStore` — le store inter-composants (ADR-012 §8)

```rust
/// Cross-component state store: maps `(component, hook_label)` → abstract value.
///
/// Written when a `ComponentSetter` call is detected in a child component's analysis.
/// Read by each component's fixpoint loop to import mutations made by its children.
#[derive(Debug, Clone, Default)]
pub struct SharedStateStore {
    entries: HashMap<(ComponentId, HookLabel), StateValue>,
}
```
(`src/domains/stores/shared_state_store.rs:10-17`)

```rust
    /// Extract all entries for `comp` as a `StateStore` (for import into that component's fixpoint).
    pub fn slice(&self, comp: ComponentId) -> StateStore<StateValue> {
        let mut store = StateStore::new();
        for ((c, label), val) in &self.entries {
            if *c == comp {
                store.update(*label, val.clone());
            }
        }
        store
    }
```
(`src/domains/stores/shared_state_store.rs:57-66`)

- Clé `(ComponentId, HookLabel)` : le composant **propriétaire** du slot
  (`ComponentId` est un identifiant interné, ADR-040 ; ADR-012 le décrivait
  encore comme un `Symbol`).
- `get` → ⊥ par défaut ; `update` → join (faible), exactement comme
  `StateStore`.
- **Une seule instance par programme**, créée dans `analyze_program`
  (`fixpoint.rs:714`), partagée par référence `RefCell` via `InterCtx`, donc
  vue par toutes les analyses (racines, enfants inlinés à toute profondeur,
  itérations successives). Elle n'est jamais remise à zéro : c'est un
  accumulateur monotone global.
- Écrite à deux endroits : `exec_setter_call` (appel d'une valeur dont le
  slot `setter` est un `One(c, l)` quand `ctx.inter` est présent,
  `interpreter.rs:368-391`) et `havoc_setter_props` (setter d'un ancêtre
  transmis à un enfant inanalysable, `state_value.rs:287-296`).
- Lue par la boucle de chaque composant à la vérification de convergence
  (`fixpoint.rs:488-493`) : `new_state = … ⊔ shared.slice(comp_id)` ; et par
  les règles (`result.shared_state.get(parent_comp, parent_label)`,
  `src/rules/impls/infinite_loop.rs:198`).
- `join` et `leq` existent (`:37-55`) mais ne sont appelés qu'en tests.

### 3.9 `Heap` et `HeapValue` — le tas par site d'allocation (ADR-010)

```rust
/// Value stored at a heap location (indexed by `ExprId` allocation site).
#[derive(Debug, Clone)]
pub enum HeapValue {
    /// A function literal: its params, body CFG, and captured environment at creation site.
    Fn {
        params: Vec<Var>,
        body_cfg: Arc<CFG>,
        /// Free variables captured from the enclosing scope when this function was created.
        captured: HashMap<Symbol, StateValue>,
    },
    /// An abstract object: fields may be plain values or heap locations (for FnLit props).
    Obj(HashMap<Symbol, EnvVal<StateValue>>),
}

/// Abstract heap: maps allocation-site `ExprId`s to `HeapValue`s.
/// Populated by `eval_expr` for `FnLit`/`ObjectLit`/`ArrayLit` nodes.
#[derive(Debug, Clone, Default)]
pub struct Heap(HashMap<ExprId, HeapValue>);
```
(`src/domains/stores/heap.rs:17-34`)

- **Site d'allocation** = `ExprId`, attribué par le lowering à chaque
  `ObjectLit`, `ArrayLit`, `FnLit`, `New` (`src/ir/expr.rs:180-244`), unique
  par composant depuis #134 (`tests/allocation_site_identity.rs:1-12`) ; les
  sites synthétiques (objet de props d'un enfant) viennent de
  `ExprId::fresh()`, compteur atomique démarrant à `1_000_000_000`
  (`src/ir/types.rs:17-25`).
- `HeapValue::Fn` : paramètres, CFG du corps (partagé par `Arc`) et
  **environnement capturé** (valeurs des variables libres au moment de
  l'allocation) — ADR-012 §6.
- `HeapValue::Obj` : carte **par membre** (`EnvVal`, donc valeur et
  éventuellement sites) — c'est ce qui permet `o.f` → valeur propre du membre
  (#88). ADR-010 prévoyait aussi `HeapValue::Arr(Vec<StateValue>)`
  (« reserved — future array domain ») : **il n'existe pas** dans le code.
  La doc de `Heap` (« Populated by `eval_expr` for
  `FnLit`/`ObjectLit`/`ArrayLit` nodes ») est inexacte sur deux points : ce
  n'est pas `eval_expr` qui peuple le tas mais l'interpréteur (`bind_rhs`,
  `obj_members`, `MemberWrite`) et `eval_props_map` ; et aucun `ArrayLit`
  n'y est jamais inséré.

Allocation d'une fermeture, **point unique** :

```rust
    /// Allocate a `HeapValue::Fn` at site `id`, snapshotting the closure's
    /// captured `StateValue`s (the body's free vars resolved in `env`).
    ///
    /// The single place a FnLit becomes a heap closure — shared by every
    /// statement arm (`Let`/`Assign`/`MemberWrite`) and by prop evaluation.
    pub fn alloc_fn<D: AbstractDomain>(
        &mut self,
        id: ExprId,
        params: &[Var],
        body_cfg: &Arc<CFG>,
        env: &AbstractEnv<D>,
    ) {
        let free = compute_free_vars(body_cfg);
        let captured = free
            .iter()
            .filter_map(|v| env.lookup(v).as_state_value().map(|sv| (v.clone(), sv)))
            .collect();
        self.insert(
            id,
            HeapValue::Fn {
                params: params.to_vec(),
                body_cfg: Arc::clone(body_cfg),
                captured,
            },
        );
    }
```
(`src/domains/stores/heap.rs:45-70`)

Remarques :

- `insert` **écrase** (`self.0.insert(id, val)`, `:41-43`) : une
  ré-allocation du même site (itération suivante, autre passe) remplace la
  fermeture — y compris ses captures. Le tas n'est donc *pas* monotone en
  valeur, seulement en clés ; comme le tas ne participe pas au test de
  convergence (seul `StateStore::leq` est testé, `fixpoint.rs:495`), cette
  forte mise à jour n'empêche pas la terminaison.
- Les captures ne contiennent que ce que `env.lookup` rend — une variable
  libre non liée donne ⊤ (et est capturée comme ⊤). Les **sites** et les
  **liaisons de setter** des variables capturées ne sont pas capturés (seule
  la valeur l'est) ; le setter reste retrouvable parce que sa valeur est un
  `SetterVal::One(comp, label)` (§4.3.2).

Opérations de treillis (`:80-131`) : `join` fait l'union des clés, joint les
captures des `Fn` de même site, **garde le côté gauche** pour les `Obj`
(« structural ») ; `widen` = `join` ; `leq` = inclusion des clés. Aucune de
ces trois méthodes n'est appelée en production (grep : aucun appel hors du
fichier) : le moteur manipule **un seul tas mutable** par composant.

Résolution des chaînes de membres, **walker unique** :

```rust
/// The allocation sites `expr` may denote, chasing member chains through the
/// heap: `o.a.f` resolves as far as the objects on the way record their
/// members. `None` for anything the heap cannot place (a call, a primitive, an
/// unbound variable) — the caller then falls back to the expression's value.
///
/// One walker so every consumer — the field evaluator, the `let` binder, the
/// object-literal member map — agrees on what a chain points to; the old
/// `Expr::Var` special cases each stopped at the first segment.
pub fn resolve_locs<D: AbstractDomain>(
    expr: &Expr,
    env: &AbstractEnv<D>,
    heap: &Heap,
) -> Option<HashSet<ExprId>> {
    match expr {
        Expr::Var(v) => env.lookup_env_val(v).and_then(|ev| ev.locs().cloned()),
        Expr::TSAnnotated(inner) => resolve_locs(inner, env, heap),
        Expr::FieldAccess { obj, field } => {
            let ids: HashSet<ExprId> = resolve_locs(obj, env, heap)?
                .iter()
                .filter_map(|id| match heap.get(*id) {
                    Some(HeapValue::Obj(fields)) => fields.get(field),
                    _ => None,
                })
                .filter_map(|ev| ev.locs())
                .flatten()
                .copied()
                .collect();
            (!ids.is_empty()).then_some(ids)
        }
        _ => None,
    }
}
```
(`src/domains/stores/heap.rs:134-165`)

`resolve_locs` ne connaît que `Var`, `TSAnnotated` et `FieldAccess` : un
`IndexAccess` (`xs[0]`), un appel ou un littéral d'objet imbriqué ne sont pas
résolus (conséquences en §8.1, D6).

### 3.10 `TriggerClass` — la classe de déclenchement (ADR-009 §2)

```rust
/// How a call's closure arguments should be treated by the side-effect pre-pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerClass {
    /// Callee is a bound state setter handled by the core exec path (functional
    /// updaters), so the pre-pass must NOT descend its closure.
    Setter,
    /// Runs as a consequence of the current render/effect: synchronous HOFs
    /// (`map`, `forEach`, …) and scheduled async (`.then`/`.catch`/`.finally`,
    /// `setTimeout`/`setInterval`, `queueMicrotask`, `requestAnimationFrame`).
    /// Its closure arguments ARE descended into.
    InCycle,
    /// Event subscription (`addEventListener`/`removeEventListener`) triggered
    /// externally, NOT part of the render→effect→render cycle. Not descended.
    Subscription,
    /// Unrecognized callee (custom helper/hook). Conservatively NOT descended
    /// (FP-averse: avoids flagging custom subscription wrappers).
    Unknown,
}
```
(`src/domains/interp/callbacks.rs:6-23`)

Écart avec ADR-009 : l'ADR distinguait `InCycleSync` et `InCycleDeferred` ;
le code les fusionne en `InCycle` (même politique : descendre) et ajoute
`Setter` (le *functional updater* est exécuté par `exec_setter_call`, pas par
la pré-passe, pour ne pas l'exécuter deux fois).

### 3.11 IR consommé (rappel)

- `Expr` (`src/ir/expr.rs:175-300`) : `Lit(Prim)`, `ObjectLit{id, fields}`,
  `ArrayLit{id, elems, arity, spread_at}`, `FnLit{id, params, body_cfg}`,
  `Var`, `FieldAccess{obj, field}`, `IndexAccess{arr, idx}`,
  `BinOp{op, lhs, rhs}`, `UnaryOp{op, arg}`, `Call{fn_, args}`,
  `New{id, fn_, args}`, `CompApp{name, props, span, origin}`,
  `NativeElem{tag, props, children, span, prop_spans}`, `TSAnnotated`,
  `StateVal(l)`, `StateSetter(l)`, `MemoVal(l)`, `CallbackVal(l)`,
  `HookMarker(l, MarkerVal)`, `SummaryVal(SummaryValue)`.
- `Stmt` (`src/ir/stmt.rs:8-29`) : `Let{var, rhs, span}`,
  `Assign{var, rhs, span}`, `MemberWrite{obj, key, rhs, span}`,
  `ExprStmt(expr, span)`. **Pas** d'instruction de retour : le retour est un
  terminateur de bloc (`Terminator::Return(expr)`).
- `MarkerVal` (`src/ir/expr.rs:312-331`) : `Undefined` (hook React sans valeur,
  p. ex. `useEffect`), `StableRef` (`useRef`), `Unknown` (hook custom ni
  inliné ni résumé), `Summary(SummaryValue)` (hook de bibliothèque résumé).
- `SummaryValue` (`src/ir/expr.rs:336-…`) : `Top`, `StableRef`, `UnstableRef`,
  `Wrapper{stable}`, `Shape{id, members}`, `Held`, `Navigator{stable}`.

### 3.12 Les stores, comparés (ADR-002, état actuel)

| Store | Clé | Défaut | Mise à jour | Sujet du point fixe ? | Portée | Durée de vie |
|---|---|---|---|---|---|---|
| `AbstractEnv` | `Var` | ⊤ (valeur) ; rien (setter/callback/site) | `extend` (forte) ; `extend_loc` (union) | oui (par bloc, dans `analyze_cfg`) | un point de programme | une passe |
| `StateStore` | `HookLabel` | ⊥ | `update` = join (faible) | **oui** (boucle externe) | un composant | toute l'analyse du composant |
| `MemoStore` | `HookLabel` | ⊤ | `set` (forte) | non (recalculé après chaque passe render) | un composant | idem |
| `SharedStateStore` | `(ComponentId, HookLabel)` | ⊥ | `update` = join | indirectement (importé par `slice`) | **programme** | tout `analyze_program` |
| `Heap` | `ExprId` | absent | `insert` (forte) ; champs d'`Obj` joints par `MemberWrite` | non | un composant (+ copie donnée aux enfants) | toute l'analyse du composant |

ADR-002 prévoyait un troisième store `RefStore { HookLabel → () }` : il n'a
jamais été implémenté séparément ; `useRef` est modélisé par
`HookMarker(_, MarkerVal::StableRef)` → `reference(Stable)`
(`state_value.rs:144-147`). Le « troisième » store du code est le tas, et le
quatrième le `SharedStateStore`.

### 3.13 Inventaire exhaustif des items publics du périmètre (relecture)

Obtenu par `grep -n "pub fn\|pub struct\|pub enum\|pub trait\|pub type\|pub
const\|pub(crate) fn\|pub(super) fn"` sur les douze fichiers, puis par un
`grep` des appelants hors périmètre (`src/`, tests exclus). Aucun `pub trait`
ni `pub type` dans le périmètre (le trait `Transfer` et le type
`AnalyzeChildFn` vivent dans `src/domains/{mod,context}.rs`).

| Item | Fichier:ligne | Visibilité | Utilisé (production) par |
|---|---|---|---|
| `struct StateValueTransfer` (+ `impl Transfer`) | `transfer/state_value.rs:24`, `:26-102` | `pub` | `engine::fixpoint`, `engine::eval` (`eval_in_stores`), `engine::written`, règles via ces deux derniers |
| `const MAX_INLINE_DEPTH: usize = 3` | `interp/interpreter.rs:21` | `pub` | `exec_callbacks_depth` ; texte de l'Info (`analysis_limit_info.rs:85`) |
| `fn exec_stmt_with_callbacks` | `interp/interpreter.rs:29-36` | `pub(crate)` | `StateValueTransfer::exec_stmt` seulement |
| `fn exec_body` | `interp/interpreter.rs:42-49` | `pub(crate)` | `fixpoint.rs:272` (retours d'arguments `FnLit` de hooks custom), `fixpoint.rs:344` (initialiseur paresseux) |
| `fn exec_body_depth` | `interp/interpreter.rs:53-61` | `pub(crate)`, non ré-exporté | interne à `interpreter.rs` (updater, pré-passe, B5/B6) |
| `fn exec_expr_effects` | `interp/interpreter.rs:114-126` | `pub(crate)` | `StateValueTransfer::exec_expr_effects` (appelé par `cfg_analyzer.rs:85`) ; `exec_full_stmt` |
| `enum TriggerClass` | `interp/callbacks.rs:7-23` | `pub` | `exec_callbacks_depth` seulement |
| `fn classify_callee` | `interp/callbacks.rs:28-56` | `pub` | `exec_callbacks_depth` seulement. **Homonymes à ne pas confondre** : `lazy_init.rs:246` (`classify_callee` privé de la règle `lazy-init`), `hook_extractor.rs:572` (méthode `classify_callee` de résolution des imports de hooks), `witness.rs:324` (`classify_callee_name`) |
| `fn topo_sort`, `fn dfs_post` | `interp/cfg.rs:5-11`, `:13-26` | `pub(super)` | `exec_body_impl` |
| `fn leq_pointwise` | `stores/mod.rs:23-25` | `pub(crate)` | `leq` d'`AbstractEnv`, `StateStore`, `SharedStateStore` |
| `fn map_get_or` | `stores/mod.rs:29-35` | `pub(crate)` | `get`/`join`/`leq` des stores, `Heap::join` |
| `enum EnvVal<D>` : `as_val`, `locs` | `stores/abstract_env.rs:15-44` | `pub` | interpréteur, `eval_field_access`, `eval_comp_app`, `engine/setters.rs:317` |
| `struct AbstractEnv<D>` + `Default` | `stores/abstract_env.rs:51-71` | `pub` (champs privés) | partout (moteur, règles) |
| `AbstractEnv::new`, `bottom` | `:74-76`, `:211-213` | `pub` | `Self::default()` tous deux : l'environnement vide est ⊥ **et** « tout ⊤ » à la lecture (voir §8.2, défauts inverses) |
| `AbstractEnv::lookup`, `lookup_env_val`, `contains` | `:79-98` | `pub` | `contains` n'est utilisé que par la règle `missing-deps` (`missing_deps.rs:79`, `:185`) pour savoir si une racine de chemin est liée localement |
| `AbstractEnv::extend`, `extend_loc`, `bind_setter`, `setter_label`, `bind_callback`, `callback_label` | `:101-128` | `pub` | interpréteur ; `setter_label` aussi par `redundant_set_state.rs:205`, `:256` |
| `AbstractEnv::join`, `widen_to`, `leq` | `:156-173`, `:178-180`, `:216-239` | `pub` | `join`/`widen_to` : `cfg_analyzer.rs:104-109`, `exec_body_impl` ; `leq` : tests seulement |
| (privés) `join_stabs`, `join_locs`, `widen_with` | `:130-142`, `:144-153`, `:182-208` | privés | `widen_with` est le corps de `widen_to` : `widen_to` ponctuel sur `stabs` (clé d'un seul côté → ⊤), `join_locs` pour les sites, union à priorité gauche pour les deux tables de liaisons |
| `enum HeapValue` | `stores/heap.rs:17-29` | `pub` | interpréteur, `state_value.rs` |
| `struct Heap` : `new`, `insert`, `alloc_fn`, `get`, `get_mut`, `join`, `widen`, `leq` | `stores/heap.rs:31-132` | `pub` | `join`/`widen`/`leq` : aucun appel (ni production ni test) |
| `fn resolve_locs` | `stores/heap.rs:134-165` | `pub` | `eval_field_access`, `eval_props_map`, `bind_rhs`, `obj_members` |
| `struct MemoStore<D>` + `Default` : `new`, `get`, `set` | `stores/memo_store.rs:7-36` | `pub` | `fixpoint.rs:300`, `:412` (`set`), `state_value.rs:136` (`get`) |
| `struct SharedStateStore` : `new`, `get`, `update`, `join`, `leq`, `slice` | `stores/shared_state_store.rs:10-67` | `pub` | `new` : `fixpoint.rs:714` ; `update` : `exec_setter_call`, `havoc_setter_props` ; `slice` : `fixpoint.rs:489` ; `get` : `infinite_loop.rs:198` ; `join`/`leq` : tests ; exposé comme champ `ProgramAnalysisResult::shared_state` (`program_result.rs:31`) |
| `struct StateStore<D>` + `Default` : `new`, `get`, `update`, `join`, `widen`, `widen_to`, `bottom`, `leq`, `labels`, `changed_labels` | `stores/state_store.rs:7-96` | `pub` | voir ci-dessous |
| (privé) `StateStore::merge_with` | `stores/state_store.rs:35-45` | privé | union des clés, absent = ⊥, combinateur `f` (join/widen/widen_to) |

Usage exact des méthodes de `StateStore` par la boucle de point fixe
(`fixpoint.rs:299-530`) :

- `bottom()` : état initial (`:299`), accumulateurs des effets (`:416`) et
  des handlers (`:454`), tranche vide en intra (`:490`) ;
- `update` : amorçage par les initialiseurs (`:350`) ; en cours de passe,
  toutes les écritures (`exec_setter_call`, havoc) ;
- `join` : `render ⊔ effets` (`:486`), puis `⊔ handlers ⊔ tranche`
  (`:491-493`) ;
- `leq` : test d'arrêt `new_state ⊑ state` (`:495`) ;
- `widen` (sans seuils) : **uniquement** au garde-fou des 100 itérations
  (`if iteration >= 100 { … state = state.widen(&new_state); break; }`,
  `:500-512`), qui élargit tous les labels et les inscrit dans
  `widen_trace` ;
- `widen_to` (avec seuils, ADR-014) : dès `iteration >= config.widen_threshold`
  (`:514-526`) ;
- `changed_labels` : labels élargis inscrits dans `widen_trace`, calculés sur
  `new_state_incycle` (render ⊔ effets, **sans** les handlers : « handler
  widening is not a bug », `:515`) ;
- `labels` : écrivains de chaque slot par effet (`slot_writers`, `:444`) et
  liste des labels du garde-fou (`:502`).

`AnalysisCtx::null(component, state, memo, heap)` (`context.rs:129-144`)
construit un contexte avec `NullCtx` et `inter: None` : c'est ce qu'utilisent
l'amorçage des `useState` (`fixpoint.rs:327-332`, avec un **tas neuf** et un
`StateStore` vide — l'initialiseur ne voit ni l'état ni le tas du
composant), `eval_in_stores`, les rejeux de `written.rs` et tous les tests
unitaires. Conséquence : un `CompApp` rencontré dans ces évaluations passe
par la branche intra d'`eval_comp_app` (havoc, `state_value.rs:477-483`),
dont les écritures tombent dans le store jetable.

---

## 4. Algorithmes clefs

### 4.1 `eval_state_value` — l'évaluateur d'expressions

```rust
fn eval_state_value(
    expr: &Expr,
    env: &AbstractEnv<StateValue>,
    ctx: &mut AnalysisCtx<StateValue>,
) -> StateValue {
    match expr {
        Expr::Lit(Prim::Int(n)) => StateValue::number(Interval::point(*n as f64)),
        Expr::Lit(Prim::Float(f)) => StateValue::number(Interval::point(*f)),
        Expr::Lit(Prim::Bool(b)) => {
            StateValue::boolean(if *b { BoolVal::True } else { BoolVal::False })
        }
        Expr::Lit(Prim::String(s)) => StateValue::str_singleton(s.to_string()),
        Expr::Lit(Prim::Null) => StateValue::null(),
        Expr::Lit(Prim::Unit) => StateValue::undefined(),

        Expr::Var(v) => env.lookup(v),
        Expr::StateVal(label) => {
            let mut val = ctx.state.get(*label);
            // ADR-017 read-side conversion: the store holds the join of
            // *written* values (event view); what a render *reads* can only
            // change at setter events of this slot (cross-render view). The
            // written value's allocation freshness (`PerRender`) must not
            // leak into reads. Assumes sets happen outside render — the
            // violation has its own diagnostic (`setter-in-render`).
            if val.reference != Stability::Bottom {
                val.reference = Stability::versioned_by(ctx.component, *label);
            }
            val
        }
        Expr::StateSetter(label) => StateValue::component_setter(ctx.component, *label),
        Expr::MemoVal(label) | Expr::CallbackVal(label) => ctx.memo.get(*label),
```
(`src/domains/transfer/state_value.rs:106-136`)

C'est une fonction récursive structurelle sur l'arbre d'expression :
**ordre d'évaluation** gauche → droite (`lhs` puis `rhs`, `:166-167`),
chaque sous-expression évaluée une fois. Elle prend `ctx` en `&mut` pour
deux raisons seulement : `havoc_setter_props`/`eval_comp_app` (qui écrivent)
et l'allocation paresseuse de fermetures de props (`eval_props_map`,
`:674`). Complexité : linéaire en la taille de l'expression, sauf pour
`CompApp` (une analyse complète de l'enfant, bornée par le cache) et pour
`+` sur deux ensembles de chaînes (produit cartésien, ≤ 4 × 4 après seuil de
`StrConst`).

#### 4.1.1 Littéraux

| IR | `StateValue` | Remarque |
|---|---|---|
| `Lit(Int(n))` | `number([n, n])` | le lowering produit `Int` quand la valeur est entière et `< i32::MAX` en valeur absolue (`expr_lower.rs:148-154`) |
| `Lit(Float(f))` | `number([f, f])` | |
| `Lit(Bool(b))` | `boolean(True|False)` | |
| `Lit(String(s))` | `str_singleton(s)` | ensemble `{s}` |
| `Lit(Null)` | `null()` | seul le slot `null` est vrai |
| `Lit(Unit)` | `undefined()` | `undefined` s'écrit `Lit(Unit)` : l'identifiant `undefined` est abaissé ainsi (`expr_lower.rs:187-193`) |

Tests : `eval_int_literal` (`:1077-1089`), `eval_bool_literal` (`:1091-1103`),
`eval_string_literal_gives_singleton` (`:1392-1402`).

#### 4.1.2 Variables et hooks

- `Var(v)` → `env.lookup(v)` : ⊤ si non lié.
- `StateVal(l)` : **conversion côté lecture** (ADR-017 §2). Le store contient
  le join des valeurs *écrites* (vue événement) ; ce qu'un render *lit* ne
  change qu'aux écritures de ce slot (vue inter-renders). Si l'emplacement
  `reference` de la valeur stockée est non-⊥, il est **remplacé** par
  `Versioned({(component, l)})`. Les autres emplacements (nombre, chaîne…)
  restent intacts. Hypothèse explicite : pas d'écriture pendant le render
  (sinon `setter-in-render` le signale à part). C'est le seul point de
  conversion du projet (« The conversion happens in **one place** », ADR-017).
- `StateSetter(l)` → `component_setter(ctx.component, l)` : **le setter
  porte toujours son propriétaire**, même en intra. C'est ce qui permet de
  suivre un setter passé en prop jusqu'à l'enfant (`SetterVal::One`), et
  c'est aussi la racine du défaut D3 (§8.1).
- `MemoVal(l)` / `CallbackVal(l)` → `ctx.memo.get(l)` (⊤ tant que non calculé).
- `HookMarker` et `SummaryVal` :

```rust
        // Call-site marker. A React hook with no tracked result really does
        // return `undefined`; an unresolved custom hook returns ⊤. Reading the
        // latter as `undefined` made it *provably stable* (`to_stability`
        // joins `Stable` for `undef`) and silenced every stability-gated rule
        // on it — a false negative.
        Expr::HookMarker(_, MarkerVal::Undefined) => StateValue::undefined(),
        Expr::HookMarker(_, MarkerVal::Unknown) => StateValue::top(),
        // `useRef` hands back the same container every render — a reference,
        // and a stable one. Both halves matter: `undefined` was stable too, but
        // it was not a reference, so the identity was invisible.
        Expr::HookMarker(_, MarkerVal::StableRef) => StateValue::reference(Stability::Stable),
        // A summarized library hook reads exactly as its summary; the marker
        // is kept (rather than replaced by a bare `SummaryVal`) so the label
        // stays anchored at the call site.
        Expr::HookMarker(_, MarkerVal::Summary(sv)) => summary_value(sv),
```
(`src/domains/transfer/state_value.rs:137-151`)

Table des résumés (`summary_value`, `:194-222`) : `Top` → ⊤ ; `StableRef` →
`ref(Stable)` ; `UnstableRef` → `ref(PerRender)` ; `Wrapper{stable}` →
`ref(Stable)` ou `ref(PerRender)` ; `Shape{..}` → **⊤** (le conteneur ne
promet rien, seuls les membres le font — lus par le tas, §4.3.1) ; `Held` →
⊤ ; `Navigator{stable}` → `ref(Stable)` si stable, sinon **⊤** (et non
`PerRender` : on ne prouve pas la fraîcheur d'un navigateur). « One table, so
the marker and the standalone `SummaryVal` can never disagree » (`:192-193`).

#### 4.1.3 Allocations et éléments

```rust
        Expr::ObjectLit { .. } => StateValue::reference(Stability::PerRender),
        Expr::ArrayLit { .. } => StateValue::reference(Stability::PerRender),
        Expr::FnLit { .. } => StateValue::reference(Stability::PerRender),
        Expr::NativeElem { .. } => StateValue::reference(Stability::Stable),
```
(`src/domains/transfer/state_value.rs:153-156`)

```rust
        // `new` allocates: a fresh reference, whose members stay ⊤ (#158).
        Expr::New { .. } => StateValue::reference(Stability::PerRender),
```
(`src/domains/transfer/state_value.rs:180-181`)

- Un littéral d'objet, de tableau ou de fonction évalué dans un render est
  une **référence neuve à chaque render** : `PerRender` est un fait *must*
  (« A fresh reference every render, guaranteed (must bound) »,
  `src/domains/impls/stability.rs:47-48`). C'est la base de
  `always-unstable-deps`.
- `new X()` : ajouté au commit `e67b10a` (#158) ; auparavant un `Call` → ⊤.
- **`NativeElem` (et `CompApp`, §4.1.10) valent `ref(Stable)`** : un élément
  JSX est pourtant un objet neuf à chaque render en JavaScript
  (`React.createElement`). Voir D8 (§8.1) pour la conséquence observée.

Remarque : l'évaluation d'un `ObjectLit` ne **peuple pas** le tas ; seule la
liaison par `let`/affectation (`bind_rhs`) ou le passage en prop
(`eval_props_map`) le fait (§4.3.1).

#### 4.1.4 Appels

```rust
        Expr::Call { fn_, .. } if returns_fresh_reference(fn_) => {
            StateValue::reference(Stability::PerRender)
        }
        Expr::Call { .. } => StateValue::top(),
```
(`src/domains/transfer/state_value.rs:176-179`)

Un appel **n'est jamais inliné par l'évaluateur** : sa valeur est ⊤, sauf
pour une liste blanche de méthodes qui allouent toujours :

```rust
fn returns_fresh_reference(callee: &Expr) -> bool {
    match callee {
        // `structuredClone(x)` always allocates a deep copy.
        Expr::Var(v) => v == "structuredClone",
        Expr::FieldAccess { obj, field } => match field.as_str() {
            // Array methods returning a NEW array, array-only names (a
            // string receiver has none of these).
            "map" | "filter" | "flat" | "flatMap" | "toSorted" | "toReversed" | "toSpliced"
            | "with" | "split" => true,
            // Static allocators — receiver-restricted: a bare `.from`/`.keys`
            // on an unknown object could be anything.
            "from" | "of" => matches!(obj.as_ref(), Expr::Var(v) if v == "Array"),
            "keys" | "values" | "entries" | "fromEntries" => {
                matches!(obj.as_ref(), Expr::Var(v) if v == "Object")
            }
            "parse" => matches!(obj.as_ref(), Expr::Var(v) if v == "JSON"),
            _ => false,
        },
        Expr::TSAnnotated(inner) => returns_fresh_reference(inner),
        _ => false,
    }
}
```
(`src/domains/transfer/state_value.rs:238-259`)

Deux exclusions délibérées (doc `:224-237`) : `slice`/`concat` (sur une chaîne
ils rendent une primitive, comparée par valeur par `Object.is` — prétendre
`PerRender` serait une fausse preuve, issue #22) et les méthodes en place
(`sort`, `reverse`, `fill`, `copyWithin`, `Object.assign`) qui rendent le
receveur. Remarque : `split` est dans la liste alors que c'est une méthode de
`String` — elle rend bien un tableau neuf, donc la preuve tient.

Les **effets** d'un appel (setter, callbacks) ne passent pas par ici : ils
sont tirés par l'interpréteur (§4.3). La **valeur de retour** d'une fonction
locale appelée n'est jamais calculée (même inlinée en B6, son retour est
ignoré, `interpreter.rs:570`) — seul un corps de *functional updater*, un
initialiseur paresseux ou un argument de hook custom voit sa valeur de retour
utilisée (`exec_body`).

#### 4.1.5 Opérateurs binaires

Deux « vues » d'un opérande :

```rust
/// Numeric view of an operand for arithmetic, per JS `ToNumber` coercion.
///
/// `Some` only when the value's active slots are within {number, null}:
/// `ToNumber(null) = 0`, so a nullable number stays a precise interval —
/// this is what lets `useState(null)` counters (`setN(n + 1)`) keep widening.
/// `undefined` coerces to NaN and every other kind is unpredictable → `None`.
fn as_arith(v: &StateValue) -> Option<Interval> {
    if v.boolean == BoolVal::Bottom
        && v.str == StrConst::Bottom
        && v.reference == Stability::Bottom
        && !v.undef
        && v.setter == SetterVal::Bottom
        && !v.other
    {
        // NB: a ⊥ interval stays Some(⊥) — a narrowed-dead path must produce
        // ⊥ (joins as a no-op), not fall through to ⊤.
        Some(if v.null {
            v.num.hull(&Interval::point(0.0))
        } else {
            v.num
        })
    } else {
        None
    }
}
```
(`src/domains/transfer/state_value.rs:708-732`)

`as_str_only` (`:736-750`) : `Some(&v.str)` seulement si l'emplacement chaîne
est le **seul** actif.

```rust
fn eval_binop(op: &BinOp, lhs: StateValue, rhs: StateValue) -> StateValue {
    match op {
        BinOp::Add => {
            if let (Some(a), Some(b)) = (as_arith(&lhs), as_arith(&rhs)) {
                return StateValue::number(a.add(&b));
            }
            match (as_str_only(&lhs), as_str_only(&rhs)) {
                (Some(StrConst::Set(a)), Some(StrConst::Set(b))) => {
                    let product: BTreeSet<String> = a
                        .iter()
                        .flat_map(|s1| b.iter().map(move |s2| format!("{s1}{s2}")))
                        .collect();
                    StateValue::str_set(product)
                }
                (Some(_), Some(_)) => StateValue::str_top(),
                _ => StateValue::top(),
            }
        }
        BinOp::Sub => match (as_arith(&lhs), as_arith(&rhs)) {
            (Some(a), Some(b)) => StateValue::number(a.sub(&b)),
            _ => StateValue::top(),
        },
        BinOp::Mul => match (as_arith(&lhs), as_arith(&rhs)) {
            (Some(a), Some(b)) => StateValue::number(a.mul(&b)),
            _ => StateValue::top(),
        },
        BinOp::Div => StateValue::top(),
        // `%` and `**` are exact on the numeric operands the interval domain
        // can follow (`Interval::rem`, `Interval::pow`); where the result may
        // be `NaN`, which no interval holds, they are ⊤ like `/`.
        BinOp::Mod => match (as_arith(&lhs), as_arith(&rhs)) {
            (Some(a), Some(b)) => a
                .rem(&b)
                .map(StateValue::number)
                .unwrap_or_else(StateValue::top),
            _ => StateValue::top(),
        },
        BinOp::Pow => match (as_arith(&lhs), as_arith(&rhs)) {
            (Some(a), Some(b)) => a
                .pow(&b)
                .map(StateValue::number)
                .unwrap_or_else(StateValue::top),
            _ => StateValue::top(),
        },
        BinOp::And | BinOp::Or => StateValue::top(),
        // Always a boolean, whatever the operands: that alone keeps `num` and
        // `str` at ⊥ so a guard over the result still narrows.
        BinOp::Eq
        | BinOp::Neq
        | BinOp::Lt
        | BinOp::Gt
        | BinOp::Leq
        | BinOp::Geq
        | BinOp::In
        | BinOp::InstanceOf => StateValue::boolean(BoolVal::Top),
        BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr | BinOp::UShr => {
            eval_bitwise(op, &lhs, &rhs)
        }
    }
}
```
(`src/domains/transfer/state_value.rs:752-811`)

Lecture opération par opération :

- **`+`** : arithmétique si les deux côtés sont dans {nombre, null} (avec
  `ToNumber(null) = 0`) ; concaténation exacte si les deux sont des ensembles
  finis de chaînes (produit cartésien, ensuite seuillé par `str_set`) ;
  chaîne inconnue si les deux sont des chaînes dont l'une est ⊤ ; **⊤ sinon**
  (chaîne + nombre, mélange de sortes). Remarque : `"a" + 1` pourrait être
  `str_top()` (toujours une chaîne en JS) ; il vaut ⊤, ce qui est sound mais
  plus lâche.
- **`-`, `*`** : arithmétique d'intervalles, ⊤ hors {nombre, null}.
- **`/`** : **toujours ⊤** (les intervalles ne représentent pas `NaN` ni
  `±Infinity` produits par une division par zéro — limite #73 dans
  `docs/limitations.md`).
- **`%`, `**`** : exacts quand `Interval::rem` / `Interval::pow` savent le
  faire, ⊤ quand `NaN` est possible (diviseur pouvant valoir 0, base
  négative) — ajoutés par le commit `548f922`. Tests
  `eval_binop_mod_by_constant_cycles_the_index` (`:1163-1174`),
  `eval_binop_mod_by_possible_zero_is_top` (`:1178-1188`),
  `eval_binop_pow_is_exact_on_the_non_negative_quadrant` (`:1192-1206`).
- **`BinOp::And`/`Or`** : ⊤. En pratique ces variantes n'apparaissent pas
  pour `&&`/`||`/`??`, que le lowering transforme en **diamant de blocs**
  (§4.1.9) — décision ADR-020 n° 1 (« Keep the `&&`/`||` diamond in
  lowering »).
- **Comparaisons, `in`, `instanceof`** : `boolean(Top)`. Rien n'est calculé,
  mais le résultat **n'est qu'un booléen** : les emplacements nombre et
  chaîne restent ⊥, ce qui permet au *narrowing* de véracité de rester
  précis sur le résultat. Le raffinement des opérandes (`if (n < 10)`) se
  fait dans `cfg_analyzer::narrow_env_for_branch` (hors périmètre, dossier
  06), pas ici.

#### 4.1.6 Opérateurs bit à bit

```rust
fn eval_bitwise(op: &BinOp, lhs: &StateValue, rhs: &StateValue) -> StateValue {
    // A ⊥ operand is an unreachable path: the result stays ⊥ rather than
    // widening back to a live range.
    if lhs.is_bottom_value() || rhs.is_bottom_value() {
        return StateValue::bottom();
    }
    let l = as_arith(lhs);
    let r = as_arith(rhs);
    let in_i32 = |i: &Interval| !i.is_bottom() && i.lo >= I32_MIN && i.hi <= I32_MAX;

    let refined = match op {
        // `x & mask` can only keep bits the mask has: with a non-negative
        // constant mask the result is in `[0, mask]`. Either side may be it.
        BinOp::BitAnd => const_of(&r)
            .or_else(|| const_of(&l))
            .filter(|m| (0.0..=I32_MAX).contains(m))
            .map(|m| Interval {
                lo: 0.0,
                hi: m,
                is_int: true,
            }),
```
(`src/domains/transfer/state_value.rs:854-874`)

Principe (doc `:846-853`) : JS convertit **les deux** opérandes en int32
(uint32 pour `>>>`) quels qu'ils soient ; le résultat est donc **toujours**
un nombre d'une plage connue, sans aucune vérification des opérandes. Ce
plancher (`int32_range()` / `uint32_range()`, `:819-834`) vaut mieux que ⊤ :
il met chaîne, booléen et référence à ⊥. Raffinements : `x & m` avec `m`
constant ≥ 0 → `[0, m]` ; `x | y`, `x ^ y` avec opérandes ≥ 0 → `[0,
2^n − 1]` (`all_ones_above`, `:933-939`) ; `x << k` constant → bornes
multipliées si pas de débordement int32 (sinon plancher : un débordement rend
l'opération non monotone) ; `x >> k` → division entière monotone ;
`x >>> k` → `[0, u32::MAX >> k]`. `shift_amount` (`:842-844`) prend
`k mod 32`. Tests `:1229-1312` (helper `num` puis
`bitwise_of_unknown_operands_is_still_an_int32`,
`unsigned_shift_of_unknown_operands_is_a_uint32`,
`bitwise_and_with_a_constant_mask_is_bounded_by_it`,
`constant_shifts_move_the_bounds`,
`a_wrapping_shift_falls_back_to_the_int32_range`,
`bitwise_on_a_bottom_operand_stays_bottom`,
`bitwise_not_is_minus_x_minus_one`).

Soundness : un opérande ⊥ (chemin mort après narrowing) rend ⊥ — pas une
plage vivante, qui « ressusciterait » un chemin tué (`:1298-1302`).

Détails d'implémentation à citer exactement :

- `const_of` (`:836-839`) ne rend une constante que pour un intervalle
  **ponctuel non-⊥** (`i.is_point()`), donc `x & m` n'est raffiné que si le
  masque est une constante connue ; `.filter(|m| (0.0..=I32_MAX).contains(m))`
  exclut les masques négatifs (dont le bit de signe rend `[0, m]` faux).
- `shift_amount` (`:841-844`) refuse une distance non finie ou de valeur
  absolue ≥ 1e9 (`None` → plancher int32/uint32) et réduit sinon par
  `rem_euclid(32)` : `x << 33` est traité comme `x << 1`, exactement comme JS.
- `all_ones_above` (`:931-939`) plafonne à 31 bits (`.min(31)`) : pour des
  opérandes non négatifs dans int32, `x | y` tient dans `[0, 2^31 − 1]`.
- `x >> k` utilise `(a.lo / f).floor()` / `(a.hi / f).floor()` : division
  entière vers −∞, ce qui est bien la sémantique de `>>` sur int32 (y compris
  pour les négatifs : `-5 >> 1 = -3`).

#### 4.1.7 Opérateurs unaires

```rust
fn eval_unary(op: &UnaryOp, val: StateValue) -> StateValue {
    match op {
        UnaryOp::Neg => match as_arith(&val) {
            Some(i) => StateValue::number(i.neg()),
            None => StateValue::top(),
        },
        UnaryOp::Not => {
            // Boolean-only operand inverts precisely; anything else is Top.
            if val.num.is_bottom()
                && val.str == StrConst::Bottom
                && val.reference == Stability::Bottom
                && !val.null
                && !val.undef
                && val.setter == SetterVal::Bottom
                && !val.other
            {
                match val.boolean {
                    BoolVal::True => StateValue::boolean(BoolVal::False),
                    BoolVal::False => StateValue::boolean(BoolVal::True),
                    _ => StateValue::boolean(BoolVal::Top),
                }
            } else {
                StateValue::top()
            }
        }
        // `typeof x` is *always* a string, and an exact one whenever the operand
        // has a single inhabited kind. That exactness is what makes
        // `typeof x === "string"` a narrowable guard instead of `BoolVal::Top`.
        UnaryOp::TypeOf => {
            if val.is_bottom_value() {
                return StateValue::bottom();
            }
            match val.typeof_name() {
                Some(name) => StateValue::str_singleton(name.to_string()),
                None => StateValue::str_top(),
            }
        }
```
(`src/domains/transfer/state_value.rs:941-977`)

- **`-x`** : négation d'intervalle, ⊤ hors {nombre, null}.
- **`!x`** : exact sur un booléen pur ; **⊤ sinon** — alors que `!x` est
  toujours un booléen en JS. C'est une imprécision (sound) qui contraste avec
  la politique des comparaisons (« Always a boolean ») : `!n` sur un nombre
  rend ⊤ au lieu de `boolean(Top)` (observé, §6.1).
- **`typeof x`** : toujours une chaîne ; exacte (`"number"`, `"boolean"`,
  `"string"`, `"undefined"`, `"object"` pour `null`, `"function"` pour un
  setter) quand une seule sorte est habitée (`typeof_name`,
  `src/domains/impls/state_value.rs:250-269`) ; `str_top()` sinon (une
  référence est « object » ou « function » : ambigu).
- **`~x`** : `-(ToInt32(x) + 1)`, bornes échangées dans int32, plancher int32
  sinon (`:978-993`).
- **`+x`** : `ToNumber`, pas l'identité (`:994-1001`) ; `coerce_to_number`
  (`:1009-1040`) traite les booléens (`0`/`1`/`[0,1]`) et les ensembles de
  chaînes **dont chaque élément se parse** en nombre fini (un seul `NaN` rend
  le tout non représentable → ⊤). Le « parse » est celui de Rust
  (`s.trim().parse::<f64>()`, `:1027`), pas `ToNumber` : il est plus strict
  que JS sur quelques formes (`+""` vaut `0` en JS mais échoue en Rust ;
  `"0x10"`, `"0b1"` aussi) et ces cas tombent sur ⊤, ce qui reste sound ;
  les formes que Rust accepte et que JS lirait autrement (`"inf"`,
  `"NaN"`, `"infinity"`) donnent un flottant non fini, écarté par
  `if !n.is_finite() { return None; }` (`:1028-1030`). Un booléen passe par
  `typeof_name() == Some("boolean")` (`:1013`) : `True` → `[1,1]`, `False` →
  `[0,0]`, `Top` → `[0,1]` entier.
- **`UnaryOp::Unknown`** (`void`, `delete` non-membre, etc. — en réalité
  `void e` et `delete o.f` sont traités au lowering, `expr_lower.rs:204-232`)
  → ⊤. Le test `eval_unary_unknown_is_top` (`:1220-1227`) rappelle
  l'historique : `~5` valait `5` quand ces coercions étaient abaissées sur
  l'identité.

#### 4.1.8 Accès aux membres (lecture par le tas)

```rust
fn eval_field_access(
    obj: &Expr,
    field: &Symbol,
    env: &AbstractEnv<StateValue>,
    ctx: &mut AnalysisCtx<StateValue>,
) -> StateValue {
    if let Some(ids) = resolve_locs(obj, env, ctx.heap) {
        let vals: Vec<StateValue> = ids
            .iter()
            .filter_map(|id| ctx.heap.get(*id))
            .filter_map(|hv| match hv {
                HeapValue::Obj(fields) => fields.get(field).map(EnvVal::as_val),
                _ => None,
            })
            .collect();
        if !vals.is_empty() {
            return vals.into_iter().reduce(|a, b| a.join(&b)).unwrap();
        }
    }
    // A field of a versioned object can change only at the object's own
    // setter events: keep the version labels instead of degrading to ⊤
    // (the field's *kind* stays unknown — every other slot is ⊤). Same
    // no-mutation-during-render assumption as the `StateVal` read-side
    // conversion above; in-place writes have their own diagnostics
    // (`state-mutation`). ADR-017 §Limitations, member-deps.
    let obj_val = eval_state_value(obj, env, ctx);
    StateValue {
        reference: obj_val.versioned_reference().unwrap_or(Stability::Unknown),
        ..StateValue::top()
    }
}
```
(`src/domains/transfer/state_value.rs:598-628`)

Algorithme :

1. Résoudre les sites que le receveur peut désigner (`resolve_locs`, qui
   suit les chaînes `a.b.c` à travers les `Obj` du tas).
2. Pour chaque site qui est un `Obj` **possédant** le champ, prendre la
   valeur du membre ; joindre.
3. Si aucun site ne répond : repli sur ⊤, en **conservant** les étiquettes
   `Versioned` du receveur (un champ d'un état objet ne change qu'aux
   écritures de cet état).

**Point de soundness fragile** : l'étape 2 ignore les sites qui *n'ont pas*
le champ au lieu d'y joindre ⊤ (ou `undefined`). Si le receveur peut désigner
un objet sans ce membre (autre branche, objet issu d'un spread, receveur
nullable lu par `?.`), la réponse est sous-approximée — défaut D5 (§8.1),
observé jusqu'à un faux négatif de `missing-deps`.

Index constant = membre (#37) :

```rust
fn eval_index_access(
    arr: &Expr,
    idx: &Expr,
    env: &AbstractEnv<StateValue>,
    ctx: &mut AnalysisCtx<StateValue>,
) -> StateValue {
    match idx.peel_ts() {
        Expr::Lit(Prim::Int(i)) if *i >= 0 => eval_field_access(arr, &i.to_string(), env, ctx),
        _ => StateValue::top(),
    }
}
```
(`src/domains/transfer/state_value.rs:641-651`)

La déstructuration de tableau s'abaisse en `__arr[0]`, `__arr[1]` : un hook
résumé qui rend un tuple (`const [value, setValue] = useAtom(a)`) expose ses
positions comme membres `"0"`, `"1"` d'un `Shape` (doc `:630-640`). Un index
non constant (`xs[i]`) reste ⊤ (#76).

#### 4.1.9 Constructions JavaScript qui n'ont pas de bras propre

L'évaluateur n'a **pas** de cas pour les templates, les ternaires, `&&`,
`||`, `??`, le chaînage optionnel, le spread, `++`, `void`, `delete`, `await`,
les séquences : le lowering (`src/lowering/expr_lower.rs`) les réécrit en
constructions déjà traitées. Pour comprendre la valeur calculée, il faut
connaître ces réécritures :

| Source | IR produit | Où | Conséquence pour l'évaluation |
|---|---|---|---|
| `` `q0${e0}q1` `` | `("q0" + e0) + "q1"` (chaîne de `BinOp::Add`, gauche-associative) | `expr_lower.rs:156-184` | exact si toutes les interpolations sont des chaînes connues ; ⊤ dès qu'une interpolation est un nombre (`"n=" + n` → ⊤, observé §6.1) |
| `` tag`…${e}…` `` | `Call { fn_: tag, args: [e, …] }` | `:323-334` | ⊤ (appel) ; les lectures des interpolations survivent |
| `a ? b : c` | diamant : `Branch(a, then, else)`, `Let __tN = b` / `Let __tN = c`, jonction → `Var(__tN)` | `:675-724` | join des deux branches **au niveau du CFG** (dans `analyze_cfg`, avec narrowing) |
| `a && b`, `a \|\| b`, `a ?? b` | `Let __tN = a; Branch(Var(__tN), …)`, `Assign __tN = b` dans le bloc rhs | `:726-795` | idem ; **le bloc rhs contient `b` en position d'`Assign`**, ce qui compte pour D2 (§8.1) |
| `a?.b`, `a?.[i]`, `f?.(x)` | comme la forme non optionnelle | `:649-671` | la doc prétend qu'un receveur lié à un `Loc` n'est jamais nul ; faux après join (D5) |
| `{ ...o }` | champ sous clé synthétique `"...N"` (`SPREAD_KEY_PREFIX = "..."`) | `:382-390`, `src/ir/expr.rs:87` | `obj_members` ne garde que les membres **après** le dernier spread (`members_after_last_spread`, `src/ir/expr.rs:96-102`) |
| `[...xs]` | `xs` gardé comme élément, `arity: AtLeast(n)`, `spread_at` | `:405-438` | aucune influence sur la valeur (`PerRender`) ; utile aux deps |
| `f(...args)` | `args` émis comme **instruction** (`lower_for_effect`), pas comme argument | `:617-634` | un setter transmis par spread n'est pas vu comme argument (#76) |
| `{ [k]: v }`, accesseurs `get x()` | clés synthétiques `"[computed]N"`, `"[accessor]N"` | `:358-378` | membre introuvable par nom → repli |
| `i++`, `--i` | `Assign i = i ± 1`, valeur `Var(i)` (post-écriture pour les deux formes) | `:256-277` | sur-approximation d'un pas pour `a = i++` |
| `o.f++`, `delete o.f` | `MemberWrite` d'une valeur opaque / `Lit(Unit)` | `:204-216`, `:278-300` | mise à jour faible du champ (§4.3.1) |
| `void e` | `ExprStmt(e)` puis `Lit(Unit)` | `:229-232` | effets de `e` conservés |
| `(a, b, c)` | `ExprStmt(a)`, `ExprStmt(b)`, valeur `c` | `:550-566` | effets conservés (FN historique corrigé) |
| `await e` | `Let __tN = e` puis coupure de bloc (`split_at_await`) | `:577-606` | la suite s'exécute dans un bloc successeur (ADR-035) |
| `this`, `super` | `opaque()` = `SummaryVal(Top)` | `:194-196` | ⊤ |
| classe | `ObjectLit` de ses membres sous clés synthétiques (`lower_class`) | `:58-135` | référence fraîche |

#### 4.1.10 Application de composant : `eval_comp_app` (ADR-012)

```rust
        Expr::CompApp {
            name,
            props,
            origin,
            ..
        } => eval_comp_app(name, props, origin.as_deref(), env, ctx),
```
(`src/domains/transfer/state_value.rs:158-163`)

Pas à pas (`:470-594`) :

1. **Sans `inter`** (analyse intra) : tout enfant est inanalysable →
   `havoc_setter_props` puis `ref(Stable)` (`:477-483`).
2. **Résolution** par le registre : `inter.registry.resolve_child(name,
   origin)` → `ChildLookup::Resolved(key)` → `ComponentId` interné
   (ADR-040 : « who it is, is the `ComponentId` the registry resolves it
   to »). Enfant non résolu ou ambigu → statistique
   (`unknown_component_refs` / `ambiguous_component_refs`) + havoc +
   `ref(Stable)` (`:489-518`). Ces statistiques deviennent des Info
   `analysis-limit` (`src/rules/impls/analysis_limit_info.rs:53-77`).
3. **Récursion** : `inter.is_recursive(child)` → `recursion_cutoffs += 1`,
   `recursive_component_refs` et `ref(Stable)` **sans havoc** (`:520-528`).
4. **Props** : `eval_props_map` (§4.3.1) → `HashMap<Symbol, EnvVal>` ; version
   aplatie en `StateValue` pour le cache et le graphe d'appels (`:537-544`).
5. **Cache** (égalité stricte, double `⊑`, `component_cache.rs:50-63` et
   `props_equal`) : succès → `cache_hits`, arête du graphe, `ref(Stable)`
   (`:546-556`). L'enfant n'est **pas** ré-analysé.
6. **Construction du contexte enfant** : environnement vide, un site
   synthétique `props_id = ExprId::fresh()`, un tas initial contenant **une
   copie** des valeurs de tas des props qui sont des `Loc` (fermetures,
   objets) et l'`Obj` des props ; le paramètre de l'enfant est lié au site
   des props (`child_env.extend_loc(child_ir.param.clone(), props_id)`).
7. **Analyse** : `inter.analyze_child(&child_ir, child, child_env,
   initial_heap, &child_inter)` — c'est `analyze_component_inter`, une
   analyse **complète** (point fixe) de l'enfant avec un `InterCtx` fils
   (pile d'appel étendue).
8. **Résultats** : `inter.results.insert(child, child_result)` (écrase le
   résultat d'un site d'appel précédent : D7), `cache.insert`, arête du
   graphe d'appels.
9. Valeur rendue : `ref(Stable)`, dans tous les cas.

```rust
    // Build child initial env + heap:
    // - copy heap entries for any Loc-valued props (FnLit bodies) into child's heap
    // - insert the Obj (with full EnvVals) so the child can resolve FieldAccess → Loc
    let mut child_env = AbstractEnv::bottom();
    let props_id = ExprId::fresh();
    let mut initial_heap = crate::domains::stores::Heap::new();
    for ev in abstract_props_full.values() {
        if let EnvVal::Loc { ids, .. } = ev {
            for &id in ids {
                if let Some(hv) = ctx.heap.get(id) {
                    initial_heap.insert(id, hv.clone());
                }
            }
        }
    }
    initial_heap.insert(props_id, HeapValue::Obj(abstract_props_full.clone()));
    child_env.extend_loc(child_ir.param.clone(), props_id);

    // Create child inter context and analyze
    let child_inter = inter.child(child);
    let analyze_child = inter.analyze_child;
    let child_result = analyze_child(&child_ir, child, child_env, initial_heap, &child_inter);
```
(`src/domains/transfer/state_value.rs:559-580`)

Remarques :

- Seul le **premier niveau** des sites des props est copié (`:565-573` ne
  parcourt que les `ids` des props elles-mêmes) : un objet passé en prop dont
  un **membre** est une fermeture (`const bag = { run: () => setN(n + 1) };
  <Child bag={bag}/>`) arrive dans le tas de l'enfant **sans** la fermeture
  (le membre `run` garde bien son `Loc`, mais le site n'existe pas dans le tas
  enfant). **Vérifié par le relecteur** (`/tmp/dom05v/deep.tsx`, §6.9) :
  `const f = bag.run; useEffect(() => { f(); })` dans l'enfant ne tire pas
  `setN` (parent `n = [0, 0]`, CLI muet), alors que la même fermeture passée
  directement (`<Child2 run={run}/>`) donne
  `cross-component-infinite-loop`. Faux négatif non déclaré (D11, §8.1).
- Le flux **descendant** (parent → enfant) passe par les props ; le flux
  **montant** (enfant → parent) passe par le `SharedStateStore` : quand
  l'enfant appelle `props.onChange(42)`, la valeur appelée est un
  `SetterVal::One(parent, l)` et `exec_setter_call` écrit
  `shared_state[(parent, l)] ⊔= 42` (§4.3.2). Le parent l'importe à sa
  prochaine vérification de convergence (`fixpoint.rs:488-493`), ce qui
  relance sa boucle si son état a changé.
- `CompApp` est aussi évalué **pour ses effets** : `exec_expr_effects`
  (`interpreter.rs:123-125`) et la pré-passe (`:529-533`) appellent
  `transfer.eval_expr(expr, …)` sur tout `CompApp` rencontré, y compris dans
  les enfants JSX et les champs. C'est ce qui déclenche l'inlining depuis un
  `return <div><Child/></div>` (terminateur `Return` →
  `exec_expr_effects`).

### 4.2 Havoc et setters qui s'échappent

Un setter remis à un enfant **inanalysable** peut être appelé par lui avec
n'importe quel argument, n'importe quand. Ne rien faire sous-approximerait
l'état et fabriquerait des conclusions « l'état est stable » (TODO.md F4,
cité `state_value.rs:496-501`). D'où :

```rust
fn havoc_setter_props(
    props_expr: &Expr,
    env: &AbstractEnv<StateValue>,
    ctx: &mut AnalysisCtx<StateValue>,
) {
    let Expr::ObjectLit { fields, .. } = props_expr else {
        return;
    };
    let own = ctx.component;
    let mut setters: Vec<(ComponentId, crate::ir::types::HookLabel)> = Vec::new();
    for (_, v) in fields {
        // Bare setter prop (`<X onOpenChange={setOpen}/>`): the value
        // carries its owner (`StateSetter` always evals to a
        // `component_setter`, intra included).
        let val = eval_state_value(v, env, ctx);
        if let Some((c, l)) = val.as_setter() {
            setters.push((*c, *l));
        }
        // Everything a function value can smuggle a setter through: spread
        // objects, closures wrapping a setter call, heap-allocated FnLits.
        collect_escaping_setters(v, env, ctx.heap, own, &mut setters, &mut HashSet::new());
    }
    for (comp, label) in setters {
        if comp == own {
            ctx.state.update(label, StateValue::top());
        } else if let Some(inter) = &ctx.inter {
            inter
                .shared_state
                .borrow_mut()
                .update(comp, label, StateValue::top());
        }
    }
}
```
(`src/domains/transfer/state_value.rs:265-297`)

`collect_escaping_setters` (`:315-367`) poursuit transitivement :
les `FnLit` syntaxiques (parcours de leur corps), les variables liées à des
sites du tas (`Fn` → corps avec ses captures ; `Obj` → membres setters), les
`TSAnnotated`, les membres d'`ObjectLit` et les éléments d'`ArrayLit` —
**pas** les arguments d'appel ni les index (« only container/function VALUES
escape into the child's hands », `:362-365`).

Terminaison par **identité** et non par budget :

```rust
    let key = (
        cfg as *const _ as usize,
        captured.map_or(0, |c| c as *const _ as usize),
    );
    if !walked.insert(key) {
        return;
    }
    cfg.for_each_expr(&mut |e| setter_calls_in_expr(e, env, captured, heap, own, out, walked));
```
(`src/domains/transfer/state_value.rs:385-392`)

La clé est la paire (adresse du CFG, adresse de l'environnement capturé) :
le même corps atteint sous une autre capture résout ses callees
différemment. Historique (doc `:309-314`) : un ancien `depth > 4` était « a
budget doing a cycle guard's job — a setter smuggled through a fifth closure
was silently missed » (faux négatif). Test
`a_deeply_nested_escaping_setter_is_still_found` (`:1405-1447`, six niveaux).
Nuance (relecture) : les six niveaux du test sont six `ObjectLit`
emboîtés (`fields: vec![("nested", value)]`) autour d'**un seul** `FnLit`
qui appelle `StateSetter(0)` ; la descente dans les `ObjectLit` est
structurelle (bras `Expr::ObjectLit` de `collect_escaping_setters`) et ne
passe pas par `walked`. Le test ne couvre donc ni une chaîne de fermetures
du tas (`Var` → `HeapValue::Fn` → appel d'une autre fermeture capturée), ni
la clé `(corps, captures)` elle-même.

`callee_setter` (`:427-467`) résout une cible d'appel : `StateSetter(l)` →
propriétaire courant ; `Var` → capture de la fermeture d'abord, puis
environnement vivant ; `o.f` avec `o` lié à un `Obj` → membre setter ;
`TSAnnotated` → dedans. Un appel via index (`fns[0]()`) ou via un appel
(`get()(x)`) n'est pas suivi (#46).

**Limite vérifiée à la relecture (D13)** : la « transitivité » annoncée par
la doc (`:299-307`) s'arrête à un saut de fermeture. Dans un corps parcouru,
un appel `inner(v)` dont le callee est une **autre fermeture** (et non un
setter) est soumis à `callee_setter`, qui rend `None` (la valeur capturée de
`inner` est `ref(PerRender)`, pas un setter) ; `setter_calls_in_expr`
descend ensuite dans les enfants de l'appel (arguments), **jamais dans le
corps de `inner`**. Exemple (`/tmp/dom05v/esc2.tsx`, §6.9 (n)) : `const
inner = (v) => setN(v); const outer = (v) => inner(v); <Sheet cfg={{ cb:
outer }}/>` avec `Sheet` inconnu → `n = [0, 0]` (pas de havoc) ; avec un seul
saut (`outer = (v) => setN(v)`) → `n = ⊤`. Le cas n'est masqué ni par
l'extraction des handlers (qui ne relève que les props **directement**
fonctions, `hook_extractor.rs:204-215` : ici la prop est un objet) ni par la
clé `walked`. L'Info `analysis-limit` « component `Sheet` was not found … (FN
possible) » est bien émise et suspend les vérifications : le faux négatif est
donc **déclaré globalement** pour ce composant, mais la valeur calculée reste
fausse.

### 4.3 L'interpréteur d'instructions (`interp/interpreter.rs`)

#### 4.3.0 Vue d'ensemble

```
Transfer::exec_stmt  ──►  exec_stmt_with_callbacks (depth 0)
                              │
                              ▼
                        exec_full_stmt(stmt, depth)
          ┌───────────────────┼───────────────────────────┐
     ExprStmt(e)        Let/Assign{rhs}             MemberWrite{obj,key,rhs}
          │                   │                           │
 exec_expr_effects    exec_callbacks_depth(rhs)   exec_callbacks_depth(obj, idx, rhs)
   ├ exec_callbacks_depth    exec_stmt_core               exec_stmt_core
   ├ exec_setter_call          └ bind_rhs                   └ weak update du champ
   └ eval_expr si CompApp

exec_callbacks_depth(e, depth) :  depth ≥ 3 → stats.callback_depth_capped, stop
   Call{fn_, args} : classe = classify_callee(fn_)
       FnLit arg & InCycle      → exec_body_depth(corps, params = ⊤, depth+1)
       Var arg & InCycle        → exec_var_callback (B5)
       callee Var & Unknown     → exec_var_callback (B6)
   CompApp                     → eval_expr (inlining de l'enfant)
   autre                       → descente générique (for_each_child), jamais dans un FnLit

exec_body_depth(cfg, env, depth) → exec_body_impl : une seule passe topologique,
   chaque instruction par exec_full_stmt(depth), Return : pré-passe + setter + valeur
```

#### 4.3.1 Liaisons : `exec_full_stmt`, `exec_stmt_core`, `bind_rhs`

```rust
fn exec_full_stmt<T: Transfer>(
    transfer: &T,
    stmt: &Stmt,
    env: &mut AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) {
    match stmt {
        // An expression statement (`expr;`) contributes only its side effects.
        // The same is true of a concise-arrow `Return` body, which the fixpoint
        // engine feeds through here — both share the single definition in
        // [`exec_expr_effects`] (callback pre-pass + setter + inter-component).
        Stmt::ExprStmt(expr, _) => {
            exec_expr_effects(transfer, expr, env, ctx, depth);
        }
        // Binding statements: callback pre-pass over the RHS parts, then the
        // binding core.
        Stmt::Let { rhs, .. } | Stmt::Assign { rhs, .. } => {
            exec_callbacks_depth(transfer, rhs, env, ctx, depth);
            exec_stmt_core(transfer, stmt, env, ctx);
        }
        Stmt::MemberWrite { obj, key, rhs, .. } => {
            exec_callbacks_depth(transfer, obj, env, ctx, depth);
            if let MemberKey::Index(idx) = key {
                exec_callbacks_depth(transfer, idx, env, ctx, depth);
            }
            exec_callbacks_depth(transfer, rhs, env, ctx, depth);
            exec_stmt_core(transfer, stmt, env, ctx);
        }
    }
}
```
(`src/domains/interp/interpreter.rs:69-99`)

Ordre d'évaluation d'une liaison : **d'abord** la pré-passe des callbacks
(les effets des fermetures « dans le cycle » contenues dans le membre
droit), **ensuite** la liaison. Point notable : pour `Let`/`Assign`, il n'y a
**pas** d'appel à `exec_setter_call` — un setter appelé dans le membre droit
d'une liaison (`const r = setB(2)`, ou `Assign __t = setA(1)` produit par
`flag && setA(1)`) n'écrit pas dans le `StateStore` (D2, §8.1). Aggravation
(relecture, lecture du code) : si l'argument est un *functional updater*
(`const r = setB((x) => { setC(1); return x + 1; })`), la pré-passe classe
le callee `Setter` et ne descend pas le `FnLit` (bras `Expr::FnLit { .. } =>
{}`, `interpreter.rs:515`) — c'est `exec_setter_call` qui devait l'exécuter,
et il n'est pas appelé : ni `B` ni les effets du corps de l'updater ne sont
vus.

`bind_rhs`, la séquence unique de liaison (`let x = e` et `x = e` identiques,
« the arms had drifted, dropping the Assign aliases (FN) », `:143-146`) :

```rust
fn bind_rhs<T: Transfer>(
    transfer: &T,
    var: &str,
    rhs: &Expr,
    env: &mut AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) {
    if let Expr::StateSetter(label) = rhs {
        env.bind_setter(var.to_string(), *label);
    }
    if let Expr::CallbackVal(label) = rhs {
        env.bind_callback(var.to_string(), *label);
    }
    if let Expr::FnLit {
        id,
        params,
        body_cfg,
    } = rhs
    {
        env.extend_loc(var.to_string(), *id);
        ctx.heap.alloc_fn(*id, params, body_cfg, env);
    }
    // An object literal is a fresh container, but its members keep their own
    // identities: `{ onClear }` where `onClear` is a `useCallback` is a new
    // object every render holding the SAME function. Recording the per-member
    // map on the heap is what lets `o.onClear` resolve to the member's own
    // value instead of inheriting the container's `PerRender` (issue #88).
    if let Expr::ObjectLit { id, fields } = rhs {
        let members = obj_members(transfer, fields, env, ctx);
        ctx.heap.insert(*id, HeapValue::Obj(members));
        env.extend_loc(var.to_string(), *id);
    }
```
(`src/domains/interp/interpreter.rs:214-245`)

Suite (`:246-292`) : un `HookMarker(_, Summary(Shape{id, members}))` reçoit
lui aussi une carte par membre sur le tas (le conteneur reste ⊤, seuls les
membres nommés portent une promesse — `useForm().setValue`) ; un alias
`let y = x` propage les sites, la liaison de setter et la liaison de callback
de `x` ; un `let f = props.onClick` propage les sites résolus par
`resolve_locs` ; enfin la valeur est évaluée et stockée
(`env.extend(var, val)`). Remarques :

- l'ordre compte : pour un `FnLit`, `extend_loc` **précède** `alloc_fn`, et
  `alloc_fn` capture l'environnement **avant** que `var` soit liée par
  valeur — une fonction récursive `const f = () => f()` capture donc `f`
  avec sa valeur antérieure (⊤ si première liaison) ;
- un `ArrayLit` n'est jamais alloué dans le tas ;
- un `ObjectLit` imbriqué (`{ inner: { f } }`) ne l'est pas non plus : seul
  le littéral **directement** lié l'est, et `obj_members` n'attribue un
  `Loc` qu'aux membres `FnLit` ou résolus par `resolve_locs` (D6).

`obj_members` (`:301-333`) : pour chaque membre après le dernier spread, un
membre `FnLit` est alloué (`alloc_fn`) et garde son site ; les autres
reçoivent `resolve_locs` et leur valeur évaluée. Doc : « an absent member
falls back to the container's own value — sound, just imprecise » — ce
repli n'est en fait garanti que si **aucun** site du receveur n'a le membre
(§4.1.8, D5).

Écriture de champ, **mise à jour faible** :

```rust
        Stmt::MemberWrite { obj, key, rhs, .. } => {
            if let Expr::FnLit {
                id,
                params,
                body_cfg,
            } = rhs
            {
                ctx.heap.alloc_fn(*id, params, body_cfg, env);
            }
            let val = transfer.eval_expr(rhs, env, ctx);
            if let (Expr::Var(v), MemberKey::Field(field)) = (obj, key)
                && let Some(EnvVal::Loc { ids, .. }) = env.lookup_env_val(v)
            {
                let Some(sv) = val.as_state_value() else {
                    return;
                };
                let new_val = match rhs {
                    Expr::FnLit { id, .. } => EnvVal::Loc {
                        ids: std::collections::HashSet::from([*id]),
                        val: sv,
                    },
                    _ => EnvVal::Val(sv),
                };
                for id in ids.iter().copied().collect::<Vec<_>>() {
                    if let Some(HeapValue::Obj(fields)) = ctx.heap.get_mut(id) {
                        // Values always join; the allocation sites survive only
                        // when both sides have them (a plain value overwriting
                        // a literal leaves nothing to chase).
                        let joined = match (fields.get(field), &new_val) {
                            (None, new) => new.clone(),
                            (Some(old), new) => {
                                let val = old.as_val().join(&new.as_val());
                                match (old.locs(), new.locs()) {
                                    (Some(a), Some(b)) => EnvVal::Loc {
                                        ids: a.union(b).copied().collect(),
                                        val,
                                    },
                                    _ => EnvVal::Val(val),
                                }
                            }
                        };
                        fields.insert(field.clone(), joined);
                    }
                }
            }
        }
```
(`src/domains/interp/interpreter.rs:155-200`)

Justification (commentaire `:150-154`) : l'identité de `obj` ne change pas
(c'est une mutation) ; l'écriture peut n'avoir lieu que sur un chemin, et le
`Loc` peut désigner plusieurs sites : **join, jamais remplacement**. Limites :
seul `Var.field = …` est traité (pas `a.b.c = …`, pas `o[k] = …`) ; un champ
**nouveau** (`(None, new)`) est inséré avec la seule nouvelle valeur — alors
qu'avant l'écriture sa lecture valait `undefined`/repli. **Vérifié par le
relecteur** (`/tmp/dom05v/mw.tsx`, §6.9) : `const o = { f: 1 }; if (flag) {
o.g = 2 } const v = o.g;` donne `env[v] = Val(number[2, 2])`, sans
`undefined` — sous-approximation (D9, §8.1). Le champ **existant**, lui, est
bien joint : `o2.f = "x"` sur une branche donne `number[1, 1]|string{"x"}`.
Deux raisons se cumulent : le cas `(None, new)` n'ajoute pas `undefined`, et
le tas est **insensible au flot** (un seul `&mut Heap` pour tous les blocs et
tous les chemins d'une passe, §8.2) — l'écriture faite sur la branche `then`
est donc vue aussi par le chemin `else`.

#### 4.3.2 Appels de setter : `exec_expr_effects` et `exec_setter_call`

```rust
pub(crate) fn exec_expr_effects<T: Transfer>(
    transfer: &T,
    expr: &Expr,
    env: &mut AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) {
    exec_callbacks_depth(transfer, expr, env, ctx, depth);
    exec_setter_call(transfer, expr, env, ctx, depth);
    if let Expr::CompApp { .. } = expr {
        transfer.eval_expr(expr, env, ctx);
    }
}
```
(`src/domains/interp/interpreter.rs:114-126`)

« This is the single definition of "run an expression for its effects" »
(`:106`) — utilisée par `ExprStmt` et par le terminateur `Return` (moteur,
`cfg_analyzer.rs:84-86`). ADR-020 n° 7 interdit de la fusionner avec le
chemin `Return` d'`exec_body_impl`, qui a besoin de la **valeur**.

```rust
fn exec_setter_call<T: Transfer>(
    transfer: &T,
    expr: &Expr,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) {
    if let Expr::Call { fn_, args } = expr
        && let Expr::Var(name) = fn_.as_ref()
        && let Some(label) = env.setter_label(name)
    {
        let arg_val = match args.first() {
            Some(Expr::FnLit {
                params, body_cfg, ..
            }) => {
                let mut sub_env = env.clone();
                if let Some(param) = params.first() {
                    sub_env.extend(param.clone(), ctx.state.get(label));
                }
                exec_body_depth(transfer, body_cfg, &sub_env, ctx, depth + 1)
            }
            Some(a) => transfer.eval_expr(a, env, ctx),
            None => T::Domain::top(),
        };
        ctx.state.update(label, arg_val);
    }

    // Cross-component ComponentSetter call.
    // Handles fn_ = Var(name), FieldAccess { obj: Var, field }, or any other expr
    // that evaluates to ComponentSetter { component, label }.
    if let Expr::Call { fn_, args } = expr {
        let comp_setter = transfer
            .eval_expr(fn_, env, ctx)
            .as_state_value()
            .and_then(|sv| sv.as_setter().map(|(c, l)| (*c, *l)));
        if let Some((component, label)) = comp_setter
            && ctx.inter.is_some()
        {
            let arg_val = args
                .first()
                .map(|a| transfer.eval_expr(a, env, ctx))
                .and_then(|v| v.as_state_value())
                .unwrap_or(crate::domains::StateValue::top());
            if let Some(inter) = &ctx.inter {
                inter
                    .shared_state
                    .borrow_mut()
                    .update(component, label, arg_val);
            }
        }
    }
}
```
(`src/domains/interp/interpreter.rs:341-392`)

Premier bras — **setter local lié** (`env.setter_label(name)`), mise à jour
faible de `ctx.state` :

- argument `FnLit` (*functional updater* `setX(c => …)`) : le **premier**
  paramètre est lié à la valeur **courante du store** (`ctx.state.get(label)`,
  donc le join de tout ce qui a été écrit jusqu'ici, initialiseur compris),
  le corps est exécuté par `exec_body_depth` et sa **valeur de retour** est
  la valeur écrite. Test `functional_updater_increments_state`
  (`state_value.rs:1480-1521`) : état `5`, `setN(c => c + 1)` → store
  `[5, 6]` (join de l'ancien et du nouveau).
- autre argument : sa valeur ;
- aucun argument : ⊤ (JS écrirait `undefined` ; ⊤ est sound).

Second bras — **setter d'un propriétaire** (`SetterVal::One(c, l)` comme
seule sorte active) **quand `ctx.inter` est présent** : mise à jour faible
de `shared_state[(c, l)]` avec la valeur du premier argument. Ce bras est
**toujours évalué**, que le premier ait tiré ou non. Comme
`StateSetter(l)` s'évalue en `component_setter(ctx.component, l)`
(`state_value.rs:135`), un setter **local** tire *aussi* ce bras ; pour un
argument ordinaire, l'écriture dans le store partagé est redondante mais
inoffensive ; pour un *functional updater*, l'argument est évalué comme un
`FnLit`, donc `ref(PerRender)`, et c'est cette fermeture qui est écrite dans
`shared_state[(own, l)]` puis ré-importée dans l'état du composant par
`slice` — défaut D3 (§8.1), qui rend muet `useEffect(() => { setN(c => c + 1);
})` en CLI.

Positions où un setter est tiré : `ExprStmt` et terminateur `Return` (via
`exec_expr_effects`), et `Return` d'un corps exécuté par `exec_body_impl`.
Nulle part ailleurs (D2).

#### 4.3.3 La pré-passe des callbacks : `exec_callbacks_depth` (ADR-009 §4)

```rust
fn exec_callbacks_depth<T: Transfer>(
    transfer: &T,
    expr: &Expr,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) {
    if depth >= MAX_INLINE_DEPTH {
        if let Some(inter) = ctx.inter {
            inter
                .stats
                .borrow_mut()
                .callback_depth_capped
                .insert(inter.component);
        }
        return;
    }
    match expr {
        Expr::Call { fn_, args } => {
            let class = classify_callee(fn_, env);
            // Descend the receiver: handles chains like `a.then(x).then(y)`.
            exec_callbacks_depth(transfer, fn_, env, ctx, depth);
            for arg in args {
                match arg {
                    Expr::FnLit {
                        params, body_cfg, ..
                    } if class == TriggerClass::InCycle => {
                        let mut sub_env = env.clone();
                        for p in params {
                            sub_env.extend(p.clone(), T::Domain::top());
                        }
                        let _ = exec_body_depth(transfer, body_cfg, &sub_env, ctx, depth + 1);
                    }
                    // B5: variable callback resolve Identifier to heap Fn and execute.
                    Expr::Var(name) if class == TriggerClass::InCycle => {
                        exec_var_callback(transfer, name, env, ctx, depth);
                    }
                    // Setter/Subscription/Unknown inline closures not descended.
                    Expr::FnLit { .. } => {}
                    other => {
                        exec_callbacks_depth(transfer, other, env, ctx, depth);
                    }
                }
            }
            // B6: direct local call inlining Unknown callee that resolves to a heap Fn.
            // External/imported functions have no Loc → skipped (no FP).
            if class == TriggerClass::Unknown
                && let Expr::Var(name) = fn_.as_ref()
            {
                exec_var_callback(transfer, name, env, ctx, depth);
            }
        }
        Expr::CompApp { props, .. } => {
            exec_callbacks_depth(transfer, props, env, ctx, depth);
            // Fire inter-component inlining for CompApp inside children/fields.
            transfer.eval_expr(expr, env, ctx);
        }
        // Bare FnLit outside a call: never runs by itself — not descended.
        // Everything else: generic child descent.
        other => {
            other.for_each_child(&mut |c| exec_callbacks_depth(transfer, c, env, ctx, depth));
        }
    }
}
```
(`src/domains/interp/interpreter.rs:477-540`)

Pas à pas :

1. **Garde de profondeur** : à `depth ≥ 3`, on enregistre
   `callback_depth_capped` (si `inter`, seulement — en intra le plafond est
   **silencieux**) et on s'arrête. L'Info `analysis-limit` correspondante
   (« callback inlining reached the depth cap (3) … (FN possible on nested
   callbacks) », `analysis_limit_info.rs:79-88`) suspend aussi les
   « verified » du composant (observé §6.3).
2. **Appel** : on classe le callee, on descend le receveur (chaînes
   `.then(a).then(b)`), puis chaque argument :
   - `FnLit` + `InCycle` : exécuter le corps pour ses effets, paramètres à
     ⊤ (valeur résolue d'une promesse, élément d'un tableau : inconnus),
     environnement = environnement **vivant** du site d'appel (capture
     naturelle d'une fermeture inline) ;
   - `Var` + `InCycle` (B5) : résoudre par le tas (`exec_var_callback`) ;
   - `FnLit` d'une autre classe : **ne pas descendre** ;
   - tout le reste : descente récursive (un appel imbriqué en argument est
     traité à son tour).
3. **B6** : callee `Unknown` **et** `Var` → `exec_var_callback` (inlining
   d'une fonction locale appelée directement : `load()`). Un callee
   `FieldAccess` inconnu (`o.f()`) n'est **pas** inliné (D6).
4. **`CompApp`** imbriqué : pré-passe sur ses props puis
   `eval_expr` (inlining de l'enfant).
5. **Invariant** (doc `:473-476`) : ne jamais descendre *dans* un `FnLit`
   ici — les corps ne s'exécutent que par `exec_body_depth`, sinon
   `exec_body_impl → exec_full_stmt → exec_callbacks_depth` exécuterait deux
   fois.

Classification :

```rust
pub fn classify_callee<D: AbstractDomain>(fn_: &Expr, env: &AbstractEnv<D>) -> TriggerClass {
    match fn_ {
        Expr::Var(name) => {
            if env.setter_label(name).is_some() {
                TriggerClass::Setter
            } else {
                match name.as_str() {
                    "setTimeout" | "setInterval" | "queueMicrotask" | "requestAnimationFrame" => {
                        TriggerClass::InCycle
                    }
                    _ => TriggerClass::Unknown,
                }
            }
        }
        Expr::FieldAccess { obj, field } => match field.as_str() {
            "then" | "catch" | "finally" | "allSettled" | "any" => TriggerClass::InCycle,
            "map" | "forEach" | "reduce" | "filter" | "find" | "flatMap" | "some" | "every"
            | "findIndex" | "findLast" | "findLastIndex" | "reduceRight" | "sort" | "toSorted"
            | "replace" | "replaceAll" => TriggerClass::InCycle,
            // `Array.from(iterable, mapFn)`: the map callback runs
            // synchronously. Receiver-restricted — a bare `.from` on an
            // unknown object could be anything.
            "from" if matches!(obj.as_ref(), Expr::Var(v) if v == "Array") => TriggerClass::InCycle,
            "addEventListener" | "removeEventListener" => TriggerClass::Subscription,
            _ => TriggerClass::Unknown,
        },
        _ => TriggerClass::Unknown,
    }
}
```
(`src/domains/interp/callbacks.rs:28-56`)

Choix *nominal* (par nom de callee), sans résolution de type ; la
reconnaissance est donc syntaxique : un `x.map(cb)` où `x` n'est pas un
tableau est tout de même « dans le cycle » (sur-approximation : on exécute un
corps qui ne l'aurait peut-être pas été, faux positif possible, jamais faux
négatif). `Subscription` n'est pas perdu : `extract_subscriptions` relève
`addEventListener(str, FnLit)` d'un effet en `HookEntry::Handler`, analysé
comme **point d'entrée séparé** (ADR-009 §4, `tests/subscriptions.rs`).
`Unknown` non descendu est un **faux négatif accepté** (ADR-009 : « We accept
the FN » ; issue #19).

Résolution par variable (B5/B6) :

```rust
fn exec_var_callback<T: Transfer>(
    transfer: &T,
    name: &str,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) {
    if let Some(EnvVal::Loc { ids, .. }) = env.lookup_env_val(name) {
        let ids: Vec<_> = ids.iter().copied().collect();
        for id in ids {
            if let Some(HeapValue::Fn {
                params,
                body_cfg,
                captured,
            }) = ctx.heap.get(id)
            {
                let params = params.clone();
                let body_cfg = Arc::clone(body_cfg);
                let captured = captured.clone();
                let mut sub_env = env.clone();
                for (var, val) in captured {
                    sub_env.extend(var, T::Domain::from_state_value(val));
                }
                for p in &params {
                    sub_env.extend(p.clone(), T::Domain::top());
                }
                let _ = exec_body_depth(transfer, &body_cfg, &sub_env, ctx, depth + 1);
            }
        }
        return;
    }
    // useCallback binding: the body lives in the hook entry, not the heap
    // (the rewrite to `CallbackVal` moved it out of the expression tree).
    // Params stay unbound → env-miss ⊤, captures resolve in the live env.
    if let Some(label) = env.callback_label(name)
        && let Some(body_cfg) = ctx.query.callback_body(label)
    {
        let _ = exec_body_depth(transfer, &body_cfg, env, ctx, depth + 1);
    }
}
```
(`src/domains/interp/interpreter.rs:544-583`)

- **Tous** les sites possibles sont exécutés (« Multi-site join … All bodies
  are executed and their effects joined — correct by over-approximation »,
  ADR-010).
- Environnement d'exécution = environnement **vivant** + **captures**
  (qui l'emportent) + paramètres à ⊤. Les liaisons de setter/sites de
  l'environnement vivant restent visibles (le clone les porte).
- `useCallback` : corps obtenu par `QueryContext::callback_body`, captures
  résolues dans l'environnement vivant.
- Variable sans `Loc` ni callback (import, paramètre) : rien — « no FP ».

#### 4.3.4 Exécution d'un corps : `exec_body_impl`

```rust
/// Processes blocks in topological order. Back edges are ignored for env
/// propagation (forward-predecessor join only); statements still execute once so
/// loop-body setter side effects are captured. Return values from all exits are
/// joined; back-edge loops conservatively join return to Top.
fn exec_body_impl<T: Transfer>(
    transfer: &T,
    cfg: &CFG,
    entry_env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) -> T::Domain {
    let has_back_edge = cfg.edges.iter().any(|e| matches!(e.kind, EdgeKind::Back));

    let order = topo_sort(cfg);
    let mut env_at: HashMap<BlockId, AbstractEnv<T::Domain>> = HashMap::new();
    env_at.insert(cfg.entry, entry_env.clone());

    let mut return_val = T::Domain::bottom();

    for bid in order {
        let env = if bid == cfg.entry {
            env_at
                .get(&bid)
                .cloned()
                .unwrap_or_else(AbstractEnv::bottom)
        } else {
            cfg.predecessors(bid)
                .iter()
                .filter_map(|p| env_at.get(p))
                .cloned()
                .reduce(|a, b| a.join(&b))
                .unwrap_or_else(AbstractEnv::bottom)
        };
        let mut env = env;

        if let Some(block) = cfg.blocks.get(&bid) {
            for stmt in &block.stmts {
                exec_full_stmt(transfer, stmt, &mut env, ctx, depth);
            }
            match &block.term {
                Terminator::Return(expr) => {
                    // A concise arrow body (`() => EXPR`) lowers EXPR to this
                    // Return. EXPR may carry side effects (`() => setN(c => c+1)`,
                    // `() => arr.map(cb)`): fire them as a statement would, then
                    // take EXPR's value as the return. `eval_expr` alone is pure
                    // and would silently drop those effects.
                    exec_callbacks_depth(transfer, expr, &env, ctx, depth);
                    exec_setter_call(transfer, expr, &env, ctx, depth);
                    let v = transfer.eval_expr(expr, &env, ctx);
                    return_val = return_val.join(&v);
                }
                Terminator::Jump(next) => {
                    env_at
                        .entry(*next)
                        .and_modify(|e| *e = e.join(&env))
                        .or_insert(env);
                }
                Terminator::Branch { then_, else_, .. } => {
                    for &next in &[*then_, *else_] {
                        env_at
                            .entry(next)
                            .and_modify(|e| *e = e.join(&env))
                            .or_insert_with(|| env.clone());
                    }
                }
                Terminator::Unreachable => {}
            }
        }
    }

    // Loop-carried return values can't be computed precisely in a single pass; be conservative.
    if has_back_edge {
        return_val = return_val.join(&T::Domain::top());
    }
    return_val
}
```
(`src/domains/interp/interpreter.rs:394-469`)

C'est le **second interpréteur de CFG** du projet (issue #12 : « Two CFG
interpreters of different strength, and nothing says which ran ») :
`analyze_cfg` (moteur) est un worklist avec narrowing et widening ;
`exec_body_impl` est **une seule passe** topologique, sans worklist, sans
narrowing (les deux branches d'un `if (true)` sont jointes : test
`functional_updater_branch_joins`, `:1523-1602`, rend `[0, 3]`), sans
widening. Il sert à tout corps imbriqué : callbacks, *functional updaters*,
initialiseurs paresseux, arguments de hooks custom, fonctions locales
inlinées.

Arcs retour (ADR-009, mise à jour 2026-06-03) : le corps d'une boucle est
exécuté **une fois**, avec l'environnement d'avant la boucle ; les setters
de la boucle tirent donc (tests `setter_in_while_loop_in_body_fires`,
`setter_in_for_loop_in_body_fires`, `:1722-1872`), mais la valeur portée par
la boucle n'est vue qu'à sa première itération (FN résiduel « on the
value », issue #21) ; la **valeur de retour** est jointe à ⊤ dès qu'un arc
retour existe (`back_edge_in_fnlit_body_returns_top`, `:1604-1641`).

**Défaut de propagation (observé, D1)** : `env_at` est indexé par le bloc
*destinataire* (chaque terminateur écrit `env_at[next]`), c'est donc
l'environnement d'**entrée** de `next`. Or l'environnement d'un bloc
non-entrée est calculé comme le join de `env_at[p]` pour ses prédécesseurs
`p` — c'est-à-dire des environnements d'**entrée des prédécesseurs**, pas
de leurs sorties. Chaque bloc voit l'état d'un bloc « en retard » : les
liaisons faites dans le bloc d'entrée sont invisibles de ses successeurs
directs. On attendrait `env_at.get(&bid)`. Conséquences mesurées en §6.8 :
une valeur liée avant un `if` lit ⊤ (sound mais imprécis) ; un **alias de
setter**, une **fonction locale** (site) ou une liaison de `useCallback`
faits avant un `if` sont perdus (faux négatif). Les tests unitaires ne le
voient pas : tous leurs corps multi-blocs lisent des variables de
l'environnement d'entrée (`c`, `setN`), jamais une variable liée dans un
bloc antérieur.

#### 4.3.5 Profondeur d'inlining

`MAX_INLINE_DEPTH = 3` (`interpreter.rs:21`). La profondeur croît à chaque
entrée dans un corps (`exec_body_depth(…, depth + 1)` : updater, callback
inline, B5, B6, `useCallback`) et se propage aux instructions de ce corps
(`exec_full_stmt(…, depth)`). Tests : `depth_limit_stops_deep_inlining`
(`:2582-2667`, chaîne `f4 → f3 → f2 → f1 → setN` : `setN` jamais atteint,
état ⊥) et `depth_guard_still_holds_with_back_edge` (`:2669-2748`). La valeur
3 est distincte de `config.max_inline_depth` (inlining des utilitaires,
`fixpoint.rs:207`). Le plafond est un faux négatif **déclaré** (Info
`analysis-limit`) en analyse inter, **non déclaré** en analyse intra
(phase 2 de `analyze_program`, tests).

### 4.4 `recompute_memo` — la valeur d'un `useMemo`/`useCallback`

```rust
    fn recompute_memo(
        &self,
        component: ComponentId,
        deps: &DepsArg,
        env: &AbstractEnv<StateValue>,
        ctx: &mut AnalysisCtx<StateValue>,
    ) -> StateValue {
        // No readable deps array: the memo may recompute on any render and may
        // equally never recompute — ⊤. `Stable` here would be a *must* claim
        // about a list the IR never saw, which is how `useMemo(fn, deps)` used
        // to read as pinned forever.
        let Some(deps) = deps.list() else {
            return StateValue::reference(Stability::Unknown);
        };
        if deps.arity == Arity::Exact(0) {
            // `[]` pins the memo — but only an array *known* to be empty.
            return StateValue::reference(Stability::Stable);
        }
        if deps.is_empty() {
            // Every entry is a spread whose source the fold cannot reach.
            return StateValue::reference(Stability::Unknown);
        }
        let stability = deps.as_slice().iter().fold(Stability::Bottom, |acc, dep| {
            // Structural projection (ADR-017): a dep that IS a state slot
            // versions the memo by that slot regardless of the slot's kind —
            // it changes only at the slot's setter events, `Versioned({l})`.
            // `eval_state_value` can't express this: version labels live on
            // the reference slot, so a numeric/bool state dep would collapse
            // to a plain `Stable`/`PerRender` via `to_stability`. Not a store
            // workaround — a genuine memo-side projection.
            if let Expr::StateVal(l) = dep.peel_ts() {
                return acc.join(&Stability::versioned_by(component, *l));
            }
            // Every other dep is evaluated through the normal path against the
            // real fixpoint stores in `ctx` (so `MemoVal`, heap fields, and
            // compound deps resolve instead of reading a fabricated ⊥ store).
            let val = eval_state_value(dep, env, ctx);
            // A dep whose reference kind is already versioned keeps its
            // labels (`to_stability` would erase them if another slot is ⊤).
            acc.join(
                &val.versioned_reference()
                    .unwrap_or_else(|| val.to_stability()),
            )
        });
        StateValue::reference(stability)
    }
```
(`src/domains/transfer/state_value.rs:56-101`)

Sémantique (React : un mémo est recalculé ssi une dep change selon
`Object.is`) :

| Deps | Valeur du mémo |
|---|---|
| absentes ou illisibles (`DepsArg::Absent` / `Opaque`) | `ref(Unknown)` — peut changer à tout render, ou jamais |
| `[]` exact | `ref(Stable)` — épinglé au montage |
| éléments tous invisibles : seulement des élisions (`[,]`, `[, ,]`), `arity = Exact(n ≥ 1)` et `elems` vide. Le commentaire du code parle de spreads, mais un spread est **gardé** comme élément (`elems.push(lower_expr(&sp.argument, …))`, `expr_lower.rs:411-415`), donc `[...xs]` n'atteint jamais ce cas (vérifié à la relecture par lecture du code) | `ref(Unknown)` |
| sinon | `ref(⨆ stabilité(dep))`, où une dep `StateVal(l)` compte `Versioned({l})`, une dep déjà versionnée garde ses étiquettes, les autres passent par `to_stability` (*motion-wins*, dossier 04) |

Points notables :

- La valeur est une **référence** dont seule la stabilité compte : le corps
  du mémo n'est jamais évalué ici.
- La projection `StateVal` ne s'applique que si la dep **est
  syntaxiquement** `StateVal(l)`. Dans le code réel, `[n]` est `Var("n")`
  (la variable liée à `StateVal`), donc la branche générale s'applique ; un
  état numérique non ponctuel donne alors `PerRender` via `to_stability` —
  le FP de projection décrit au dossier 04 §6.6 (observé à nouveau §6.5 :
  `byN = ref(PerRender)`).
- Depuis ADR-020 (« `recompute_memo` gets the real stores ») les deps sont
  évaluées contre les vrais stores (`tests/memo_recompute.rs`) : un mémo
  dépendant d'un mémo lit sa valeur courante, non ⊥.

### 4.5 Place dans la boucle de point fixe (rappel)

Ordre d'une itération (`fixpoint.rs:355-530`) : passe render
(`analyze_cfg`, qui appelle `exec_stmt` sur chaque instruction et
`exec_expr_effects` sur chaque `Return`) → recalcul des mémos depuis
l'environnement de sortie → passes des effets (environnement d'entrée =
sortie du render) → passes des handlers → `new_state = render ⊔ effets ⊔
handlers ⊔ shared.slice(comp)` → arrêt si `new_state ⊑ state` → sinon
`widen_to` (après `widen_threshold` itérations) ou remplacement. Après
convergence : un rafraîchissement de la passe render (`inter = None`, pour
que les règles lisent les mémos convergés) et une ré-exécution des effets
depuis ⊥ pour isoler les écritures pures des setters (`effect_setter_writes`).

Conséquence pour ce dossier : **l'interpréteur ne voit jamais l'état « en
train de converger » comme un tout** ; il voit la copie `state_out` de la
passe, qui accumule les écritures. Un *functional updater* lit donc
`state_out.get(l)`, l'état de l'itération précédente joint aux écritures
déjà faites dans la passe.

### 4.6 Complexité

- `eval_state_value` : O(|e|) hors `CompApp` et produits de chaînes (≤ 16
  chaînes intermédiaires avant seuillage).
- `exec_body_impl` : O(blocs + instructions + arcs) par exécution
  (`predecessors` est lui-même un parcours linéaire des arcs, donc
  O(blocs × arcs) au pire). Chaque site d'appel ré-exécute le corps appelé :
  avec une profondeur ≤ 3 et `k` sites par corps, O(k³) exécutions au pire
  par instruction racine.
- `collect_escaping_setters` : chaque paire (corps, capture) visitée une
  fois — linéaire en la taille des corps atteignables.
- `eval_comp_app` : une analyse complète de l'enfant par jeu de props
  abstraites distinct (cache ≤ `max_per_component` entrées puis entrée
  dégradée par join, `component_cache.rs:66-92`). La capacité par défaut est
  `DEFAULT_MAX_PER_COMPONENT = 5` (`component_cache.rs:10`). Nuance
  (relecture, hors périmètre) : à l'éviction, ce sont les **props** qui sont
  jointes (`join_all_props`) ; le résultat associé à l'entrée dégradée est
  celui de la **dernière** analyse (`result: Arc::new(result)`,
  `:80-85`), pas un join des résultats évincés. Un appel ultérieur dont les
  props valent exactement le join tomberait sur ce résultat partiel (à
  vérifier : cas non reproduit).
- Le tas et les environnements sont clonés beaucoup (`env.clone()` à chaque
  corps exécuté, à chaque branche) ; pas de partage structurel.

### 4.7 Où la soundness est garantie (et où elle ne l'est pas)

| Mécanisme | Garantie | Où |
|---|---|---|
| variable inconnue ⇒ ⊤ | sur-approximation des valeurs | `abstract_env.rs:78-81` |
| join d'environnements : clé d'un seul côté ⇒ ⊤ | idem | `abstract_env.rs:130-142` |
| `StateStore::update` = join | écritures *may* | `state_store.rs:29-33` |
| setter vers enfant inconnu ⇒ ⊤ dans le slot (havoc) | pas de « stable » fabriqué | `state_value.rs:265-297`, `:506-517` |
| setters échappés : parcours par identité, pas par budget | pas de setter manqué par profondeur | `state_value.rs:385-392` |
| appel ⇒ ⊤ sauf allocateurs certains | valeurs | `state_value.rs:176-179` |
| `/`, `%` et `**` à `NaN` possible ⇒ ⊤ | les intervalles n'ont pas de `NaN` | `state_value.rs:778-795` |
| opérande ⊥ ⇒ ⊥ | chemins morts restent morts | `state_value.rs:855-859`, `:722-723` |
| corps avec arc retour ⇒ retour ⊤ | valeur de retour | `interpreter.rs:464-467` |
| paramètres de callbacks ⇒ ⊤ | valeurs | `interpreter.rs:505-507`, `:567-569` |
| `MemberWrite` = join | mutations *may* | `interpreter.rs:180-196` |
| deps illisibles ⇒ `ref(Unknown)`, jamais `Stable` | mémos | `state_value.rs:63-69` |
| **faux négatifs acceptés et déclarés** : callee `Unknown` non descendu (ADR-009, #19), plafond de profondeur (Info), enfant inconnu (Info), récursion (Info), valeur portée par une boucle dans un callback (#21) | | |
| **faux négatifs observés, non déclarés** : D1 à D12 et D14 (§8.1 ; D9-D14 ajoutés à la relecture ; D13 est couvert par l'Info « enfant inconnu ») | | |

---

## 5. Décisions de conception

### 5.1 ADR-002 — Abstract domains: stability lattice + 3 stores (Accepted, 2026-05-29)

**Décide** : treillis de stabilité `⊥ < Stable, Unstable < ⊤` ; table de
transfert statique (primitive → Stable, littéral d'objet/tableau/fonction →
Unstable, setter → Stable, `useRef()` → Stable, `useRef().current` →
Unknown, `useMemo`/`useCallback` → join des deps, appel non-hook →
« Unstable (conservative) », `obj.prop` → Unknown) ; **trois stores
séparés** parce que les hooks ont des sémantiques différentes vis-à-vis du
cycle de render : `StateStore` (seul sujet du point fixe, seul dont la
mise à jour déclenche un re-render), `MemoStore` (dérivé des deps, une passe
après stabilisation), `RefStore` (trivial). Alternative refusée : un store
unifié, qui « would force all hooks into the same fixpoint ».

**Ce qui a changé depuis** (sans que l'ADR soit marqué *superseded*) :

- le domaine est devenu le produit `StateValue` (ADR-015, qui *supersede*
  ADR-008) et le treillis de stabilité a gagné `Versioned`, `VersionedTop`,
  et `Unstable` s'appelle `PerRender` (ADR-017) ;
- un appel non-hook vaut **⊤**, pas `Unstable` (`state_value.rs:179`) ; seuls
  les allocateurs certains valent `PerRender` ;
- `obj.prop` est résolu par le tas quand c'est possible (ADR-010, #88) ;
- le `MemoStore` stocke une valeur (pas un couple `(deps, val)`) et est
  recalculé **à chaque itération**, pas une fois après stabilisation ;
- il n'y a pas de `RefStore` ; `useRef` est un `HookMarker(StableRef)` ;
- les fichiers cités (`src/domains/stability.rs`, `state_store.rs`, …,
  `product.rs`) ont été réorganisés en `src/domains/{impls,stores}/`.

### 5.2 ADR-009 — Semantic callback traversal: entry points + trigger class (Accepted, implémenté, 2026-06-02/04)

**Contexte** : le point fixe ne descendait que dans les *functional updaters* ;
`fetch(...).then(u => setUser(u))` était invisible. Piège : descendre
**uniformément** dans tous les callbacks armerait `InfiniteLoop` sur
`addEventListener('click', () => setCount(c => c + 1))`, correct en React
(le handler ne tourne que sur événement externe).

**Décide** :

1. niveau **sémantique** (le `StateStore` bouge), pas seulement structurel ;
2. classification par **ce qui déclenche** le callback (`TriggerClass`) :
   HOF synchrones et différés (`.then`, timers) → descendre ; souscriptions
   et inconnus → ne pas descendre ;
3. abstraction « **point d'entrée** » : render, effets, callbacks in-cycle,
   handlers — même machinerie, différant par (a) appartenance au cycle
   automatique, (b) « un widening induit est-il un bug ? » ;
4. **pré-passe** par instruction, `eval` reste pur ;
5. mécanique : environnement du site d'appel, paramètres ⊤, retour ignoré,
   mise à jour faible, deux callbacks de `.then(onF, onR)` descendus.

**Alternatives refusées** : `Unknown → descendre` (le choix *sound*) refusé
parce qu'il produirait un FP sur chaque wrapper de souscription custom
(`useInterval`, `useEventCallback`) : « for a linter, false positives are
more costly than false negatives … We accept the FN … It's a *knob* ». Une
provenance « event » dans le store n'est nécessaire que pour les handlers
(résolu depuis : les handlers sont des points d'entrée du point fixe mais
exclus de `widened_labels`, `fixpoint.rs:451-453`, `:514-525`).

**Mises à jour** : handlers comme racines (§1-3 et §5 implémentés le
2026-06-03), `addEventListener` des effets relevé en handler (§4, 2026-06-04),
suppression du *bail* sur arc retour (2026-06-03, FN « setter dans une
boucle » résolu, FN résiduel sur la valeur).

**Tension avec CLAUDE.md** : le choix `Unknown → skip` est un faux négatif
**par décision**, alors que l'invariant du projet est « faux négatifs
INTERDITS ». L'ADR l'assume et le déclare (issue #19) ; c'est l'un des rares
cas documentés où la précision l'emporte sur la soundness.

### 5.3 ADR-010 — Heap model: allocation-site abstraction (Accepted, implémenté, 2026-06-03/04)

**Contexte** : B5 (`const cb = () => setN(n + 1); setTimeout(cb, 1000)`) et
B6 (`async function load() { setUser(data) }; load()`) : l'IR ne reliait pas
un nom à son corps.

**Décide** : `ExprId` par nœud allouant (site d'allocation, stable entre
itérations) ; `Heap: ExprId → HeapValue` ; `AbstractEnv` à **deux tables**
(valeurs et sites) ; tas passé à `exec_stmt`/`eval_expr` et **un seul tas**
partagé par toutes les passes et itérations (« B5 cross-pass fixed ») ;
B5 dans la pré-passe, B6 pour les callees `Unknown` qui sont des `Var` liées à
un site ; garde de profondeur `MAX_INLINE_DEPTH = 3`.

**Alternative refusée** : une seule table `EnvVal = Val | Loc` (écrasement
du `Loc` par `extend`).

**Limites déclarées** : callee `Unknown` sans `Loc` → FN ; profondeur > 3 →
FN + Info ; `HeapValue::Arr` réservé (jamais implémenté) ; membres avant un
spread et accesseurs absents de la carte → repli sur le conteneur.

**Écarts** : `HeapValue::Fn` a gagné `captured` (ADR-012 §6) ;
`HeapValue::Obj` est une carte d'`EnvVal` (valeur + sites) et non de
valeurs ; le paramètre `heap` est maintenant dans `AnalysisCtx`.

### 5.4 ADR-012 — Inter-component analysis architecture (Accepted, 2026-06-04)

**Décide** : (1) **inlining descendant** (le parent analyse l'enfant au
`CompApp`, avec les props abstraites) plutôt que des résumés paramétriques
ascendants (« without justified gain at the current stage ») ; (2)
**sensibilité au contexte par égalité abstraite** des props (cache, double
`⊑`, borné, débordement par join) ; (4) registre multi-fichiers ; (5) flux
bidirectionnel : props vers le bas, setters vers le haut ; (6) fermetures =
variables libres capturées ; (7) `ComponentSetter{component, label}`
correspondant au `Set_clos{label, path}` de React-tRace ; (8)
**`SharedStateStore`** passé à toutes les analyses, « No separate
program-level fixpoint layer. » (ADR-012, l. 82) — une écriture inter-composants est
indiscernable d'un setter local pour le point fixe du parent, qui se relance
si sa tranche a changé ; (9) props = objet abstrait sur un `ExprId`
synthétique ; (10) détection des racines modulaire ; (11) récursion à la
MOPSA (pile, hypothèse `A_ignore_recursion`) ; (12) `ProgramAnalysisResult`.

**Écarts avec le code** : `ComponentSetter` est devenu l'emplacement
`setter: SetterVal` du produit (ADR-015) ; les clés sont des `ComponentId`
internés (ADR-040) et non des `Symbol` ; `AbstractObject` est `HeapValue::Obj`
; la récursion rend `ref(Stable)` (pas ⊤) et ne havoque pas ; le cache
ignore les corps des fermetures passées en props (D4).

**Limites acceptées** : imports hors registre → ⊤ + Info ; récursion profonde
→ Info ; composants dynamiques (`const C = cond ? A : B`) non analysés
(issue #63, **fermée wontfix**).

### 5.5 ADR-015 — Product value domain over disjoint JS kinds (Implemented, 2026-07-14, *supersedes* ADR-008)

**Décide** (pour ce qui touche le transfert) : `StateValue` produit de huit
emplacements ; `ComponentSetter` devient un emplacement (`SetterVal`,
`as_setter()` ne répond que si c'est la seule sorte active) ; arithmétique
avec `ToNumber(null) = 0` dans `eval_binop` (« This is exact JS semantics and
makes the unguarded `useState(null)` counter … widen and get flagged »),
`undefined` et autres sortes → ⊤, opérande ⊥ → ⊥. Supprime
`TypedStateStore`, `infer_state_type`, `type_hint` : « the fixpoint carrier
is a plain `StateStore<StateValue>` » (`fixpoint.rs:297-299`).

### 5.6 Autres ADR pertinents

- **ADR-001** (React-tRace comme sémantique concrète de référence) : les
  fonctions de transfert en dérivent ; `Set_clos{label, path}` ↔ setter
  porteur de son propriétaire.
- **ADR-017** (stabilité versionnée) : conversion côté lecture dans le bras
  `StateVal`, « in **one place** » ; double vue store (événements) /
  lecture (inter-renders) ; `FieldAccess` sur objet versionné garde les
  étiquettes.
- **ADR-020** (non-changements délibérés) : n° 1 garder le diamant `&&`/`||`
  ; n° 7 garder distinct le chemin `Return` d'`exec_body_impl` ; n° 8 la
  graine de tas d'`eval_in` est un argument par site (vide ≠ convergé) ;
  n° 10 ne pas brancher `TSType` dans le domaine ; « `recompute_memo` gets
  the real stores ».
- **ADR-040** (identité de composant = identifiant interné) : clé du cache,
  des résultats, du graphe, du `SharedStateStore`.

### 5.7 Issues fermées `wontfix` pertinentes

`gh issue list --state closed --label wontfix` (6 issues au 2026-09-28) ;
pertinentes ici :

- **#63** « Out of scope — dynamic components (`const C = cond ? A : B`) » :
  pas de `CompApp` généré, donc pas d'inlining.
- **#51** « By design — `node_modules` utilities/hooks/components are never
  lowered » : callees et composants de bibliothèque opaques (⊤, ou résumé).
- **#40** « FP by decision — whole-object read via guard/nullish is flagged »
  (règle `missing-deps`, lecture d'un objet entier par `if (!x)`/`x ?? d`).

Issue **#22** (ouverte, mais rédigée comme décision : « Closed as a recorded
decision ») : `slice`/`concat` exclus de `returns_fresh_reference`.

### 5.8 Principes de CLAUDE.md qui s'appliquent

- *Pas de workarounds* / *modulaire d'abord* : la séquence unique `bind_rhs`
  pour `Let`/`Assign` (`interpreter.rs:143-148`), le walker unique
  `resolve_locs` (`heap.rs:134-141`), le point unique `alloc_fn`
  (`heap.rs:45-49`), la table unique `summary_value`
  (`state_value.rs:192-193`), la définition unique d'`exec_expr_effects`
  (`interpreter.rs:101-113`) sont tous justifiés dans le code par ce
  principe.
- *Soundness* : havoc, parcours par identité, ⊤ par défaut, joins. Les
  défauts D1-D14 sont précisément des violations de cet invariant.
- *Paragraphe unique* : la projection `StateVal` de `recompute_memo` se
  justifie en un commentaire (« Not a store workaround — a genuine
  memo-side projection »).

### 5.9 Historique utile

`git log --oneline -- src/domains/transfer/state_value.rs` (extraits, du plus
récent au plus ancien) :

```
e67b10a feat(engine): every write that runs is a site, and a reviver that fires once revives once (#162, #160, #158, #161) (#163)
548f922 feat(domain): model `%`, `**`, `in` and `instanceof` instead of ⊤
806d114 fix: a JSX callee is resolved by the file that writes it, and identity is an id (#7)
e3d87eb fix: a tuple contract is indexed by position (#37)
767338f fix: a wrapper is not necessarily stable (#94)
d1d11bf fix: a wrapper does not run its argument (#94, timing half)
e36f78c fix: a library contract is about the members (#94, values half)
7607ac9 feat: a write is a write wherever it is written (#130)
48ffef9 fix: deps arity and the three-state deps argument — repair the a195bfa FNs
8c9639c fix: a member of a fresh object keeps its own stability (#88)
fb8a7b1 fix(lowering): stop dropping callback bodies, writes and reads
769a0f5 fix: keep unsupported binary operators sound
da2fe5d fix(engine): a hook the engine cannot model reads ⊤, and keeps its call site
b7bc459 fix(domains): an unresolved custom hook reads ⊤, not `undefined`
a8a08d7 refactor(engine): single effect-firing path, drop fabricated ExprStmt
31215be refactor(domains): pass real stores to recompute_memo (Thème 4)
29a6709 fix: resolve Wave-0 soundness FNs and wire cheap central primitives
c65c5da fix(domains): propagate version labels through field reads
ea1862b feat(rules): state-mutation rule + fresh-array return modeling
5812845 fix: erase remaining corpus FPs — module consts, closure captures, escape reachability
f31acae fix: kill corpus FP root causes F1-F5, add object-churn detection (ADR-017)
c32e1b0 refactor: product value domain over disjoint JS kinds (ADR-015)
bcffcf7 feat: inter-component analysis (ADR-012)
6e6b9bc feat: traversée side-effect-only des callbacks avec back-edge
```

Pour `src/domains/stores/` : `daa8d67 feat: stores & transfer trait.`
(origine), `cfc27bc feat: typed state stores …` (ADR-008, supprimé par
`c32e1b0`), `8a49f25 feat: heap model + variable callback resolution`
(ADR-010), `bcffcf7` (SharedStateStore), `f937160 feat: add widening up to`
(ADR-014), `8c9639c` (#88, `EnvVal::Loc` porte une valeur). Pour
`src/domains/interp/` : `be0e97b refacto: décomposer le fichier
state_value.rs …` (séparation transfert / interpréteur),
`4c106d7 feat: refacto all envs, state, heap etc... into one context object`
(`AnalysisCtx`), `17db96b feat: event listner handling`, `a8a08d7`
(`exec_expr_effects`), `17d5e57 fix: the binding chase answered a different
prop each run (#120)`.

Le commit `7607ac9` (#130) a rendu visible au **relationnel** (relation
*writers*) un setter appelé dans n'importe quelle position d'expression ; il
n'a pas touché `exec_setter_call`, d'où l'asymétrie D2.

---

## 6. Exemples concrets

### 6.0 Protocole

- Fichiers d'exemple dans `/tmp/dom05/ex*.tsx`, analysés par
  `./target/debug/reactant check /tmp/dom05/exN.tsx --no-color [--show-clean]
  [--info] [--trace]`.
- **Sonde** : crate temporaire `/tmp/dom05/probe` dépendant du dépôt par
  chemin (`reactant = { path = "/home/rboudrouss/reactant-analyzer",
  default-features = false }`), construit avec
  `CARGO_TARGET_DIR=/tmp/dom04/target` à la rédaction ; à la relecture, ce
  binaire n'y était plus et la sonde a été reconstruite à l'identique avec
  `CARGO_TARGET_DIR=/tmp/dom05/target` (binaire
  `/tmp/dom05/target/debug/probe05`, source inchangée
  `/tmp/dom05/probe/src/main.rs`, 91 lignes). `cargo` se trouve dans
  `~/.cargo/bin` (absent du `PATH` par défaut du shell). Deux modes :
  - `probe05 intra F vars…` : `lower_program` puis
    `analyze_component(comp, &StateValueTransfer, &Config::default())` (même
    chaîne que `tests/functional_updater.rs`) ; imprime `state_store.get(l)`,
    `widen_trace.contains_key(l)`, les `memo_store.get(l)` des
    `useMemo`/`useCallback`, et `exit_env().lookup_env_val(v)` pour les
    variables demandées ; `ComponentId(4294967295)` =
    `ComponentId::SYNTHETIC` ;
  - `probe05 prog F vars…` : `ComponentRegistry::from_components` puis
    `analyze_program(reg, HookRegistry::new(), RootStrategy::Heuristic,
    &Config::default())` (même chaîne que `tests/inter_component.rs`) ;
    imprime en plus `shared_state.get(id, l)` et les statistiques. Les
    composants y sont désignés par `ComponentId(n)` (ordre d'interning,
    alphabétique observé).
- Le CLI utilise le chemin **programme** (`analyze_program`) : quand les deux
  modes divergent, c'est le mode `prog` qui prédit la sortie du CLI.
- Rien n'a été écrit dans le dépôt en dehors de ce dossier.

### 6.1 Littéraux et opérateurs (`/tmp/dom05/ex1.tsx`)

```tsx
import { useState, useEffect } from "react";

export function Ops() {
  const [n, setN] = useState(0);
  const [label, setLabel] = useState("a");
  const masked = n & 7;
  const kind = typeof n;
  const text = `n=${n}`;
  const neg = ~5;
  const plus = +"5";
  const both = label + "b";
  const cmp = n < 3;
  const notTrue = !true;
  const notNum = !n;
  const nul = null + 1;
  useEffect(() => {
    setLabel(both);
  }, [both]);
  return <div>{masked}{kind}{text}{neg}{plus}{cmp}{notTrue}{notNum}{nul}</div>;
}
```

Sonde (`intra`) :

```
-- Ops (intra, iterations=4)
   state[0] = number[0, 0]   widened=false
   state[1] = string   widened=true
   env[n] = Val(number[0, 0])
   env[label] = Val(string)
   env[masked] = Val(number[0, 7])
   env[kind] = Val(string{"number"})
   env[text] = Val(⊤)
   env[neg] = Val(number[-6, -6])
   env[plus] = Val(number[5, 5])
   env[both] = Val(string)
   env[cmp] = Val(boolean)
   env[notTrue] = Val(false)
   env[notNum] = Val(⊤)
   env[nul] = Val(number[1, 1])
```

Lecture :

- `n & 7` → `[0, 7]` (masque constant, §4.1.6) ; `~5` → `-6` ; `+"5"` → `5` ;
  `null + 1` → `1` (`ToNumber(null) = 0`).
- `typeof n` → `{"number"}` (une seule sorte habitée).
- `` `n=${n}` `` → `"n=" + n` → **⊤** (chaîne + nombre, §4.1.5).
- `n < 3` → `boolean` (Top) ; `!true` → `false` ; `!n` → **⊤** (pas
  `boolean`, §4.1.7).
- `label + "b"` : `{"a"}` → `{"ab"}` écrit par l'effet → `{"a","ab"}` →
  `{"ab","abb"}`… l'ensemble dépasse le seuil de `StrConst` et devient
  `string` (⊤ de l'emplacement), `widened=true`.

CLI (`--info --show-clean`) :

```
  Ops  (3 hooks)  /tmp/dom05/ex1.tsx
    info   widening-info  (line 16:2)  state `label` kept changing during analysis and was approximated to converge, so findings that depend on it may be imprecise
       (2 trace step(s), rerun with --trace)
    verified  always-unstable-deps  no deps array is defeated by an always-fresh reference
    verified  conditional-hook  all hooks run unconditionally, in a stable order
    verified  derived-state  no effect merely mirrors other state
    verified  infinite-loop  no effect diverges into an infinite render loop
    …
✓  1 file(s) no issues found.
```

Remarque (hors périmètre, règles) : l'effet `[both]` → `setLabel(both)` est
une vraie boucle infinie en React (`"a" → "ab" → "abb" → …`), mais
`infinite-loop` se déclare « verified ». Le domaine a fait son travail
(`widened=true`) ; c'est la règle qui ne considère pas la croissance d'un
ensemble de chaînes comme une divergence (dossier 04 §6.3 décrit un faux
négatif voisin sur les chaînes).

### 6.2 *Functional updater* : unitaire, intra, programme (`/tmp/dom05/ex7.tsx`, `ex8.tsx`)

Au niveau unitaire (`state_value.rs:1480-1521`), l'IR construit à la main
`setN(c => c + 1)` sur un état `5` produit `[5, 6]` : le paramètre `c` est lié
à `ctx.state.get(0)` et le retour `c + 1` est joint dans le store.

`/tmp/dom05/ex7.tsx` :

```tsx
export function UpdaterNoDeps() {
  const [a, setA] = useState(0);
  useEffect(() => {
    setA((x) => x + 1);
  });
  return <div>{a}</div>;
}

export function UpdaterMount() {
  const [c, setC] = useState(0);
  useEffect(() => {
    setC((x) => x + 1);
  }, []);
  return <div>{c}</div>;
}

export function UpdaterGuarded() {
  const [b, setB] = useState(0);
  useEffect(() => {
    setB((x) => (x < 10 ? x + 1 : x));
  });
  return <div>{b}</div>;
}
```

Sonde, mode `intra` :

```
-- UpdaterNoDeps (intra, iterations=3)
   state[0] = number[0, inf]   widened=true
-- UpdaterMount (intra, iterations=3)
   state[0] = number[0, inf]   widened=true
-- UpdaterGuarded (intra, iterations=1)
   state[0] = ⊤   widened=false
```

Sonde, mode `prog` (celui du CLI) :

```
-- ComponentId(0) (iterations=1)
   state[0] = ⊤   widened=false   shared=ref(PerRender)
-- ComponentId(1) (iterations=2)
   state[0] = ⊤   widened=false   shared=ref(PerRender)
-- ComponentId(2) (iterations=2)
   state[0] = ⊤   widened=false   shared=ref(PerRender)
```

CLI (`--info --trace --show-clean` : la ligne `→` vient de `--trace`, les
lignes `verified` de `--info`) :

```
  UpdaterGuarded  (2 hooks)  /tmp/dom05/ex7.tsx
    warn   infinite-loop  [hook:1]  (line 21:2)  this effect has no dependency array and may store a fresh reference into state `b`, so it re-runs after every render and can re-trigger itself: possible infinite render loop
       → a fresh value is written to state `b` here [hook:1] (line 22:4)
  …
  UpdaterMount  (2 hooks)  /tmp/dom05/ex7.tsx  ✓
  …
  UpdaterNoDeps  (2 hooks)  /tmp/dom05/ex7.tsx  ✓
    …
    verified  infinite-loop  no effect diverges into an infinite render loop
```

Trois phénomènes :

1. En **intra**, `UpdaterNoDeps` et `UpdaterMount` élargissent correctement
   (`[0, ∞]`) ; la règle distingue ensuite `[]` (montage unique) d'un effet
   sans deps — c'est exactement ce que pin `tests/functional_updater.rs`
   (`functional_updater_without_deps_is_flagged`, 5 tests verts).
2. En **programme**, le second bras d'`exec_setter_call` écrit la
   **fermeture** `ref(PerRender)` dans `shared_state[(own, 0)]`, ré-importée
   par `slice` ; le paramètre `x` de l'updater devient `number ⊔ ref`, `x + 1`
   devient ⊤ et plus rien n'élargit. **`UpdaterNoDeps` — une vraie boucle
   infinie — est déclaré « verified » par le CLI** (D3). Même source isolée
   (`/tmp/dom05/ex8.tsx`, copie du test `functional_updater_without_deps_is_flagged`) :
   CLI `✓ 1 file(s) no issues found.`, JSON `"diagnostics": []`.
3. `UpdaterGuarded` vaut ⊤ **même en intra** : le corps `x < 10 ? x + 1 : x`
   est un diamant ; le bloc de jonction lit `__tN` depuis l'environnement
   d'entrée de ses prédécesseurs (D1), donc non lié → ⊤. Le Warning émis
   (« fresh reference ») est un effet de bord de ⊤, pas une preuve.

### 6.3 Traversée des callbacks (`/tmp/dom05/ex9.tsx`)

```tsx
import { useState, useEffect } from "react";
import { myHelper } from "./lib";

export function Callbacks() {
  const [user, setUser] = useState(null);
  const [n, setN] = useState(0);
  const [clicks, setClicks] = useState(0);
  const [h, setH] = useState(0);
  const [d, setD] = useState(0);
  useEffect(() => {
    fetch("/api").then((u) => setUser(u));
    const tick = () => setN(42);
    setTimeout(tick, 100);
    window.addEventListener("click", () => setClicks(7));
    myHelper(() => setH(9));
    const f1 = () => setD(1);
    const f2 = () => f1();
    const f3 = () => f2();
    const f4 = () => f3();
    f4();
  }, []);
  return <div>{user}{n}{clicks}{h}{d}</div>;
}
```

Sonde (`prog`) :

```
-- ComponentId(0) (iterations=1)
   state[0] = ⊤   widened=false   shared=⊤
   state[1] = number[0, 42]   widened=false   shared=number[42, 42]
   state[2] = number[0, 7]   widened=false   shared=number[7, 7]
   state[3] = number[0, 0]   widened=false   shared=⊥
   state[4] = number[0, 0]   widened=false   shared=⊥
stats: cache_hits=0 cache_misses=0 depth_capped={ComponentId(0)} unknown_refs={}
```

| Ligne | Mécanisme | Résultat |
|---|---|---|
| `fetch().then(u => setUser(u))` | `.then` → `InCycle`, `FnLit` descendu, `u` = ⊤ | `user` ⊇ ⊤ |
| `setTimeout(tick, 100)` | `setTimeout` → `InCycle`, argument `Var` → B5, site de `tick` dans le tas | `n = [0, 42]` |
| `addEventListener("click", …)` | `Subscription`, non descendu ; relevé en `HookEntry::Handler` par le lowering et analysé comme point d'entrée | `clicks = [0, 7]` |
| `myHelper(() => setH(9))` | callee importé → `Unknown`, `FnLit` non descendu ; pas de `Loc` → pas de B6 | `h = [0, 0]` (FN accepté, #19) |
| `f4()` → `f3` → `f2` → `f1` → `setD` | B6 à chaque niveau, profondeur 1, 2, 3 → plafond | `d = [0, 0]`, `depth_capped` |

La colonne `shared` montre que les écritures **locales** (setters du
composant lui-même) sont aussi versées au `SharedStateStore` en mode
programme (D3 ; inoffensif ici puisque les arguments ne sont pas des
updaters).

CLI (`--info`) :

```
  Callbacks  (7 hooks)  /tmp/dom05/ex9.tsx
    info   analysis-limit  callback inlining reached the depth cap (3), so deeper HOF chains are not descended (FN possible on nested callbacks)
    warn   missing-cleanup  [hook:5]  (line 14:4)  this effect calls `window.addEventListener` but returns no cleanup. The registration is repeated every time the effect re-runs (and on every mount, twice under StrictMode) and nothing ever undoes it; return a function that tears it down
    suspended  analysis-limit  10 passing check(s) withheld: the analysis was truncated in this component, so they are not guaranteed
```

Le plafond de profondeur est **déclaré** (Info) et **suspend** les
vérifications : le faux négatif possible est honnêtement signalé. Le
`myHelper` inconnu, lui, ne produit aucune Info (FN accepté par ADR-009).

### 6.4 Le tas : membres, sites, fermetures (`/tmp/dom05/ex10.tsx`, `ex11.tsx`)

```tsx
export function Heapy({ id }: { id: string }) {
  const [q, setQ] = useState("");
  const onClear = useCallback(() => setQ(""), []);
  const bag = { onClear, label: "x", n: 1 };
  const fromBag = bag.onClear;
  const { label } = bag;
  const fresh = new Map();
  const items = [1, 2].map((x) => x * 2);
  const nested = { inner: { f: () => setQ("n") } };
  const g = nested.inner.f;
  useEffect(() => {
    g();
  }, [fromBag, items]);
  const memo = useMemo(() => q.length, [q]);
  return <div onClick={bag.onClear}>{label}{memo}{String(fresh)}</div>;
}
```

Sonde (`intra`) :

```
-- Heapy (intra, iterations=0)
   state[0] = string{""}   widened=false
   memo[1] (Callback) = ref(Stable)
   memo[3] (Memo) = ref(Stable)
   env[q] = Val(string{""})
   env[onClear] = Val(ref(Stable))
   env[bag] = Loc { ids: {ExprId(2)}, val: ref(PerRender) }
   env[fromBag] = Val(ref(Stable))
   env[label] = Val(string{"x"})
   env[fresh] = Val(ref(PerRender))
   env[items] = Val(ref(PerRender))
   env[nested] = Loc { ids: {ExprId(6)}, val: ref(PerRender) }
   env[g] = Val(⊤)
   env[memo] = Val(ref(Stable))
```

Lecture :

- `bag` : `Loc` sur le site 2, valeur `PerRender` (nouveau conteneur à
  chaque render) — **mais** `bag.onClear` lit la valeur propre du membre,
  `ref(Stable)` (le `useCallback` à deps `[]`), grâce à la carte par membre
  (#88). Idem `label` (déstructuration `__obj.label`) → `{"x"}`.
- `new Map()` → `PerRender` (#158) ; `[1, 2].map(…)` → `PerRender`
  (`returns_fresh_reference`).
- `nested.inner.f` → **⊤** : le littéral `inner` n'a pas de site dans le tas
  (seul le littéral directement lié en a un), donc `g` n'a pas de `Loc`, et
  `g()` dans l'effet n'est pas inliné : `setQ("n")` est manqué, `q` reste
  `{""}` et le mémo `[q]` est jugé `Stable` alors qu'il changera.

CLI :

```
  Heapy  (4 hooks)  /tmp/dom05/ex10.tsx
    warn   always-unstable-deps  [hook:2]  (line 13:2)  this effect depends on `items`, a new reference every render, so `Object.is` always differs and the effect re-runs on every render regardless of the other deps
       (1 trace step(s), rerun with --trace)
    warn   missing-deps  [hook:2]  var:g  (line 13:2)  `g` is used in this effect but not in its deps array, and its value may change between renders
       (1 trace step(s), rerun with --trace)
```

`fromBag` n'est **pas** signalé (sa stabilité est prouvée) ; `items` l'est
(fraîcheur certaine). `g` est signalé pour `missing-deps` parce que ⊤.

Isolement du cas imbriqué (`/tmp/dom05/ex11.tsx`) :

```tsx
export function Flat() {
  const [q, setQ] = useState("");
  const o = { f: () => setQ("n") };
  const g = o.f;
  useEffect(() => {
    g();
  }, []);
  return <div>{q}</div>;
}

export function Nested() {
  const [q, setQ] = useState("");
  const o = { inner: { f: () => setQ("n") } };
  const g = o.inner.f;
  useEffect(() => {
    g();
  }, []);
  return <div>{q}</div>;
}

export function MethodCall() {
  const [q, setQ] = useState("");
  const o = { f: () => setQ("n") };
  useEffect(() => {
    o.f();
  }, []);
  return <div>{q}</div>;
}
```

```
-- Flat (intra, iterations=1)
   state[0] = string{"", "n"}   widened=false
   env[g] = Loc { ids: {ExprId(1)}, val: ref(PerRender) }
   env[o] = Loc { ids: {ExprId(0)}, val: ref(PerRender) }
-- Nested (intra, iterations=0)
   state[0] = string{""}   widened=false
   env[g] = Val(⊤)
   env[o] = Loc { ids: {ExprId(5)}, val: ref(PerRender) }
-- MethodCall (intra, iterations=0)
   state[0] = string{""}   widened=false
   env[o] = Loc { ids: {ExprId(11)}, val: ref(PerRender) }
```

`Flat` : `o.f` résolu par `resolve_locs` → site 1 → B6 → `setQ("n")` →
`{"", "n"}`. `Nested` et `MethodCall` : setter manqué (D6).

### 6.5 `recompute_memo` (`/tmp/dom05/ex17.tsx`)

```tsx
export function Memos({ p }: { p: number }) {
  const [n, setN] = useState(0);
  const [obj, setObj] = useState({ a: 1 });
  const pinned = useMemo(() => ({ k: 1 }), []);
  const always = useMemo(() => ({ k: 1 }));
  const byN = useMemo(() => n * 2, [n]);
  const byObj = useMemo(() => obj.a, [obj]);
  const byProp = useMemo(() => p + 1, [p]);
  const byFresh = useMemo(() => 1, [{}]);
  const chained = useMemo(() => byObj, [byObj]);
  const cb = useCallback(() => setN(1), []);
  useEffect(() => {
    setObj({ a: 2 });
  }, [n]);
  return <div onClick={cb}>{pinned.k}{always.k}{byN}{byObj}{byProp}{byFresh}{chained}</div>;
}
```

```
-- Memos (intra, iterations=1)
   state[0] = number[0, 1]   widened=false
   state[1] = ref(PerRender)   widened=false
   memo[2] (Memo) = ref(Stable)
   memo[3] (Memo) = ref(Unknown)
   memo[4] (Memo) = ref(PerRender)
   memo[5] (Memo) = ref(Versioned({(ComponentId(4294967295), 1)}))
   memo[6] (Memo) = ref(Unknown)
   memo[7] (Memo) = ref(PerRender)
   memo[8] (Memo) = ref(Versioned({(ComponentId(4294967295), 1)}))
   memo[9] (Callback) = ref(Stable)
   env[obj] = Val(ref(Versioned({(ComponentId(4294967295), 1)})))
   …
```

| Mémo | Deps | Valeur | Raison |
|---|---|---|---|
| `pinned` | `[]` | `ref(Stable)` | `Arity::Exact(0)` |
| `always` | absentes | `ref(Unknown)` | `deps.list()` = `None` |
| `byN` | `[n]`, `n ∈ [0,1]` | `ref(PerRender)` | dep `Var`, `to_stability` d'un intervalle non ponctuel (FP de projection, dossier 04 §6.6) |
| `byObj` | `[obj]` | `ref(Versioned({1}))` | lecture de `StateVal(1)` convertie côté lecture ; `versioned_reference()` garde l'étiquette |
| `byProp` | `[p]` | `ref(Unknown)` | `p` = paramètre, non lié → ⊤ |
| `byFresh` | `[{}]` | `ref(PerRender)` | littéral d'objet |
| `chained` | `[byObj]` | `ref(Versioned({1}))` | mémo → mémo, étiquettes propagées gratuitement (ADR-017 §4) |
| `cb` | `[]` | `ref(Stable)` | `useCallback` partage le même calcul |

Remarque : `state[1]` (l'objet) vaut `ref(PerRender)` dans le **store** (vue
événement : on y a écrit des littéraux frais), mais `obj` lu vaut
`Versioned` (vue inter-renders) — la double vue d'ADR-017 visible sur un
seul exemple.

### 6.6 Inter-composants : props, `SharedStateStore`, havoc (`/tmp/dom05/ex13.tsx`)

```tsx
import { useState, useEffect } from "react";
import { Sheet } from "./ui";

function Child({ onChange }: { onChange: (n: number) => void }) {
  useEffect(() => {
    onChange(42);
  }, []);
  return <span />;
}

export function Parent() {
  const [count, setCount] = useState(0);
  const [open, setOpen] = useState(false);
  return (
    <div>
      <Child onChange={setCount} />
      <Sheet onOpenChange={setOpen} />
      {count}{String(open)}
    </div>
  );
}
```

Sonde, `prog` :

```
-- ComponentId(0) (iterations=0)
   env[onChange] = Val(setter(#1#0))
-- ComponentId(1) (iterations=1)
   state[0] = number[0, 42]   widened=false   shared=number[42, 42]
   state[1] = ⊤   widened=false   shared=⊥
stats: cache_hits=1 cache_misses=1 depth_capped={} unknown_refs={(ComponentId(1), "Sheet")}
```

Sonde, `intra` (aucun enfant analysable) :

```
-- Child (intra, iterations=0)
-- Parent (intra, iterations=1)
   state[0] = ⊤   widened=false
   state[1] = ⊤   widened=false
```

Chaîne causale en mode programme :

1. `Parent` (racine, `ComponentId(1)`) : la passe render évalue le `Return`
   → `exec_expr_effects` → pré-passe → `CompApp Child` → `eval_comp_app`.
2. Props : `onChange = setCount` → `StateSetter(0)` → `setter(#1#0)`
   (`SetterVal::One(Parent, 0)`) ; tas enfant = `{ props_id: Obj{onChange} }`.
3. Analyse de `Child` : `const { onChange } = props` → `__obj.onChange` →
   valeur `setter(#1#0)` ; effet `onChange(42)` → `exec_setter_call`, second
   bras → `shared_state[(Parent, 0)] ⊔= 42`.
4. Retour dans la boucle de `Parent` : `slice(Parent)` rend `{0: 42}` →
   `new_state` n'est pas ⊑ `state` → itération suivante ; l'enfant est
   retrouvé dans le cache (`cache_hits=1`) ; convergence : `count =
   [0, 42]`.
5. `<Sheet onOpenChange={setOpen} />` : `Sheet` n'est pas dans le registre
   → `unknown_component_refs`, **havoc** : `open` ⊇ ⊤.
6. En `intra`, les deux enfants sont inanalysables : les deux slots sont
   havoqués (⊤).

CLI :

```
  Parent  (2 hooks)  /tmp/dom05/ex13.tsx
    info   analysis-limit  component `Sheet` was not found in the analysis registry. Pass its file on the command line to analyse it (FN possible)
    suspended  analysis-limit  4 passing check(s) withheld: the analysis was truncated in this component, so they are not guaranteed
   1 clean component(s) hidden, rerun with --show-clean
```

Test d'intégration équivalent sur IR construit à la main :
`setter_prop_propagates_to_shared_state` (`tests/inter_component.rs:185-329`,
assertion `SharedStateStore[(Parent,0)] should be Number([42,42])`) ; sur
fixture : `prop_drilling_two_levels_updates_shared_state` (`:736-755`,
Root → Middle → Leaf, valeur 99), `nativeelem_children_compapps_are_analyzed`
(`:757-781`, `[1, 2]`).

### 6.7 Boucle croisée détectée (`/tmp/dom05/ex15.tsx`)

```tsx
function Ticker({ onTick }: { onTick: () => void }) {
  useEffect(() => {
    onTick();
  });
  return <span />;
}

export function Solo() {
  const [b, setB] = useState(0);
  return (
    <div>
      <Ticker onTick={() => setB(b + 1)} />
      {b}
    </div>
  );
}
```

```
-- ComponentId(0) (iterations=3)
   state[0] = number[0, inf]   widened=false   shared=number[1, inf]
-- ComponentId(1) (iterations=0)
stats: cache_hits=3 cache_misses=1 depth_capped={} unknown_refs={}
```

CLI :

```
  Ticker  (1 hooks)  /tmp/dom05/ex15.tsx
    warn   cross-component-infinite-loop  [hook:0]  (line 4:2)  this effect calls `onTick`, a state setter of parent `Solo`. Parent re-renders → child re-renders → effect fires again: infinite loop
```

Le `FnLit` passé en prop est alloué par `eval_props_map` (captures : `setB`
= `setter(#0#0)`, `b`) et copié dans le tas de l'enfant ; dans `Ticker`,
`onTick` est un `Loc` vers ce site ; l'appel `onTick()` est un B6 : le corps
s'exécute avec ses captures, `setB(b + 1)` tire le second bras
d'`exec_setter_call` (le setter a pour propriétaire `Solo`). Remarque :
`widened=false` côté parent parce que la même fermeture est aussi un
`HookEntry::Handler` du parent (prop fonction d'un composant, relevée par
`extract_handlers`, `hook_extractor.rs:195-215`) et que les écritures des
handlers ne comptent pas comme élargissement.

### 6.8 Défauts reproduits (du plus simple au plus subtil)

Chaque cas ci-dessous est une **vraie boucle infinie ou une vraie dépendance
manquante en React**, que l'analyseur déclare sûre au commit `e67b10a`. Les
causes sont analysées en §8.1.

**(a) Setter hors position d'instruction — D2** (`/tmp/dom05/ex6.tsx`) :

```tsx
export function StmtPos() {
  const [a, setA] = useState(0);
  useEffect(() => {
    setA(a + 1);
  });
  return <div>{a}</div>;
}

export function AndPos({ flag }: { flag: boolean }) {
  const [a, setA] = useState(0);
  useEffect(() => {
    flag && setA(a + 1);
  });
  return <div>{a}</div>;
}

export function ArgPos() {
  const [a, setA] = useState(0);
  useEffect(() => {
    console.log(setA(a + 1));
  });
  return <div>{a}</div>;
}
```

```
-- StmtPos (intra, iterations=3)
   state[0] = number[0, inf]   widened=true
-- AndPos (intra, iterations=0)
   state[0] = number[0, 0]   widened=false
-- ArgPos (intra, iterations=0)
   state[0] = number[0, 0]   widened=false
```

CLI : `StmtPos` → `warn infinite-loop … keeps pushing state `a` to new values
on every run` ; `AndPos` et `ArgPos` → `✓`, avec `verified infinite-loop no
effect diverges into an infinite render loop`.

**(b) Liaison perdue d'un bloc à l'autre dans un corps imbriqué — D1**
(`/tmp/dom05/ex3.tsx`, `ex4.tsx`, `ex20.tsx`) :

```tsx
export function OneBlockBehind({ flag }: { flag: boolean }) {
  const [n, setN] = useState(0);
  const [m, setM] = useState(0);
  const [k, setK] = useState(0);
  useEffect(() => {
    setTimeout(() => {
      const v = 5;
      if (flag) {
        setN(v);
      }
    }, 10);
    setTimeout(() => {
      const w = 7;
      setM(w);
    }, 10);
    setTimeout(() => {
      const s = setK;
      if (flag) {
        s(3);
      }
    }, 10);
  }, [flag]);
  return <div>{n}{m}{k}</div>;
}
```

```
-- OneBlockBehind (intra, iterations=1)
   state[0] = ⊤   widened=false
   state[1] = number[0, 7]   widened=false
   state[2] = number[0, 0]   widened=false
```

`m` (ligne droite) est exact ; `n` lit `v` = ⊤ (liaison du bloc d'entrée
invisible dans le bloc `then`) ; `k` : l'alias `s` est perdu, **l'écriture
disparaît**. Forme plus naturelle (`/tmp/dom05/ex20.tsx`) :

```tsx
export function HelperThenBranch({ on }: { on: boolean }) {
  const [n, setN] = useState(0);
  useEffect(() => {
    const id = setTimeout(() => {
      const tick = () => setN(n + 1);
      if (on) {
        tick();
      }
    }, 100);
    return () => clearTimeout(id);
  }, [n, on]);
  return <div>{n}</div>;
}
```

avec son jumeau `HelperStraight` sans `if` : sonde `prog` →
`HelperStraight` `state[0] = number[0, inf] widened=true`,
`HelperThenBranch` `state[0] = number[0, 0] widened=false`. (Le CLI est muet
sur les deux : la règle ne résout pas non plus `tick` — un second défaut,
côté règles, hors périmètre.)

**(c) *Functional updater* en mode programme — D3** : voir §6.2
(`/tmp/dom05/ex8.tsx`, JSON `"diagnostics": []`).

**(d) Cache des composants aveugle aux corps de fermetures — D4**
(`/tmp/dom05/ex14.tsx` vs `ex16.tsx`) :

```tsx
export function Board() {
  const [a, setA] = useState(0);
  const [b, setB] = useState(0);
  return (
    <div>
      <Ticker onTick={() => setA(1)} />
      <Ticker onTick={() => setB(b + 1)} />
      {a}{b}
    </div>
  );
}
```

(`Ticker` comme en §6.7.) Sonde : `cache_hits=15 cache_misses=1` pour tout le
fichier (`Board` et sa variante `BoardSwapped`) ; CLI : aucun diagnostic.
Contrôle (`ex16.tsx`) : même `Board`, mais le second enfant est un
composant `Ticker2` identique → `warn cross-component-infinite-loop … this
effect calls `onTick`, a state setter of parent `Board``. La seule
différence est que les deux `onTick` ont la même valeur abstraite
(`ref(PerRender)`) : le second site d'appel touche le cache et l'enfant n'est
jamais analysé avec la fermeture qui boucle.

**(e) Résultat d'un enfant écrasé par un autre site d'appel — D7**
(`/tmp/dom05/ex23.tsx` vs `ex22.tsx`) :

```tsx
function Counter({ step }: { step: number }) {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + step);
  });
  return <span>{n}</span>;
}

export function OnlyOne() {
  return <Counter step={1} />;
}
```

CLI : `warn infinite-loop [hook:0] (line 5:2) this effect keeps pushing
state `n` to new values on every run.` En remplaçant `OnlyOne` par

```tsx
export function OneThenZero() {
  return (
    <div>
      <Counter step={1} />
      <Counter step={0} />
    </div>
  );
}
```

le CLI rend `Counter (2 hooks) ✓` ; sonde : `state[0] = number[0, inf]
widened=false` (la dernière analyse, `step = 0`, a écrasé la première dans
`results`, et l'état `[0, ∞]` vient du `SharedStateStore` global, pas d'un
élargissement de cette analyse). L'instance `step={1}` boucle pourtant.

**(f) Lecture de membre partielle — D5** (`/tmp/dom05/ex18.tsx`, `ex19.tsx`) :

```tsx
export function PartialField({ flag, p }: { flag: boolean; p: any }) {
  const [s] = useState(0);
  const o = flag ? { f: 1 } : { ...p };
  const v = o.f;
  const q = flag ? { f: 1 } : null;
  const w = q?.f;
  const r = flag ? { f: 1 } : { g: 2 };
  const x = r.f;
  return <div>{v}{w}{x}{s}</div>;
}
```

```
   env[o] = Loc { ids: {ExprId(1), ExprId(0)}, val: ref(PerRender) }
   env[v] = Val(number[1, 1])
   env[q] = Loc { ids: {ExprId(3)}, val: ref(PerRender)|null }
   env[w] = Val(number[1, 1])
   env[r] = Loc { ids: {ExprId(4), ExprId(5)}, val: ref(PerRender) }
   env[x] = Val(number[1, 1])
```

`v` peut valoir `p.f` (n'importe quoi), `w` et `x` peuvent valoir
`undefined` : les trois sont annoncés `1`. Conséquence observée :

```tsx
export function PartialDeps({ flag, p }: { flag: boolean; p: any }) {
  const o = flag ? { f: 1 } : { ...p };
  const v = o.f;
  useEffect(() => {
    console.log(v);
  }, []);
  return <div />;
}

export function SpreadOnlyDeps({ p }: { p: any }) {
  const o = { ...p };
  const v = o.f;
  useEffect(() => {
    console.log(v);
  }, []);
  return <div />;
}
```

CLI : `PartialDeps ✓` ; `SpreadOnlyDeps → warn missing-deps … `v` is used in
this effect but not in its deps array`.

**(g) Élément JSX réputé stable — D8** (`/tmp/dom05/ex24.tsx`) :

```tsx
export function ElementDep() {
  const el = <span />;
  const obj = { a: 1 };
  useEffect(() => {
    console.log(el);
  }, [el]);
  useEffect(() => {
    console.log(obj);
  }, [obj]);
  return <div>{el}</div>;
}
```

Sonde : `env[el] = Val(ref(Stable))`, `env[obj] = Loc { …, val:
ref(PerRender) }`. CLI : un seul `always-unstable-deps`, sur `obj`. En
React, `el` est un nouvel objet élément à chaque render : l'effet `[el]`
s'exécute à chaque render exactement comme `[obj]`.

**(h) Appel de méthode d'un objet local — D6** (`/tmp/dom05/ex12.tsx`) :

```tsx
export function ViaVar() {
  const [n, setN] = useState(0);
  const o = { f: () => setN(n + 1) };
  const g = o.f;
  useEffect(() => {
    g();
  });
  return <div>{n}</div>;
}

export function ViaMethod() {
  const [n, setN] = useState(0);
  const o = { f: () => setN(n + 1) };
  useEffect(() => {
    o.f();
  });
  return <div>{n}</div>;
}
```

Sonde `prog` : `ViaVar` `state[0] = number[0, inf] widened=true` ;
`ViaMethod` `state[0] = number[0, 0] widened=false`. CLI : les deux `✓`
(pour `ViaVar`, la valeur est juste mais la règle ne résout pas `g` —
défaut côté règles, hors périmètre ; pour `ViaMethod`, la valeur est déjà
fausse).

### 6.9 Exemples ajoutés à la relecture (`/tmp/dom05v/*.tsx`)

Même protocole (§6.0) ; sonde reconstruite avec
`CARGO_TARGET_DIR=/tmp/dom05/target cargo build` dans `/tmp/dom05/probe`
(binaire `/tmp/dom05/target/debug/probe05`), CLI
`target/debug/reactant check <f> --no-color`. Tous rejoués le 2026-09-28 au
commit `e67b10a`. Tous les exemples de §6.1 à §6.8 ont aussi été rejoués à
l'identique (sondes et CLI) à cette date.

**(i) Champ nouveau écrit sur une branche — D9** (`mw.tsx`) :

```tsx
import { useState } from "react";
export function NewField({ flag }: { flag: boolean }) {
  const [s] = useState(0);
  const o: any = { f: 1 };
  if (flag) {
    o.g = 2;
  }
  const v = o.g;
  const o2: any = { f: 1 };
  if (flag) {
    o2.f = "x";
  }
  const w = o2.f;
  return <div>{v}{w}{s}</div>;
}
```

```
-- NewField (intra, iterations=0)
   state[0] = number[0, 0]   widened=false
   env[v] = Val(number[2, 2])
   env[w] = Val(number[1, 1]|string{"x"})
   env[o] = Loc { ids: {ExprId(0)}, val: ref(PerRender) }
   env[o2] = Loc { ids: {ExprId(1)}, val: ref(PerRender) }
```

`w` est juste (le champ existait : join). `v` devrait contenir `undefined`
(chemin `flag` faux) : le cas `(None, new)` de `MemberWrite`
(`interpreter.rs:183-184`) insère `2` seul, et le tas unique fait voir cette
écriture au chemin qui ne l'a pas faite.

**(j) Setter choisi par un ternaire — D10** (`sj.tsx`) :

```tsx
import { useState, useEffect } from "react";
export function Pick({ c }: { c: boolean }) {
  const [a, setA] = useState(0);
  const [b, setB] = useState(0);
  useEffect(() => {
    const s = c ? setA : setB;
    s(1);
  }, [c]);
  return <div>{a}{b}</div>;
}
```

```
-- Pick (intra, iterations=1)
   state[0] = number[0, 1]   widened=false
   state[1] = number[0, 0]   widened=false
-- ComponentId(0) (iterations=1)
   state[0] = number[0, 1]   widened=false   shared=⊥
   state[1] = number[0, 0]   widened=false   shared=⊥
```

`b` n'est jamais écrit : la liaison de setter de `__tN` garde `setA`
(priorité à gauche, `abstract_env.rs:159-162`), et la valeur jointe
`SetterVal::Top` ne déclenche pas le second bras (`shared=⊥`). CLI `✓`.

**(k) Fermeture membre d'un objet prop — D11** (`deep.tsx`) :

```tsx
import { useState, useEffect } from "react";
function Child({ bag }: { bag: { run: () => void } }) {
  const f = bag.run;
  useEffect(() => {
    f();
  });
  return <span />;
}
function Child2({ run }: { run: () => void }) {
  useEffect(() => {
    run();
  });
  return <span />;
}
export function Parent() {
  const [n, setN] = useState(0);
  const bag = { run: () => setN(n + 1) };
  return <div><Child bag={bag} /></div>;
}
export function Parent2() {
  const [n, setN] = useState(0);
  const run = () => setN(n + 1);
  return <div><Child2 run={run} /></div>;
}
```

Sonde `prog` (`ComponentId` : 0 = `Child`, 1 = `Child2`, 2 = `Parent`,
3 = `Parent2`) :

```
-- ComponentId(0) (iterations=0)
   env[f] = Loc { ids: {ExprId(5)}, val: ref(PerRender) }
   env[bag] = Loc { ids: {ExprId(4)}, val: ref(PerRender) }
-- ComponentId(1) (iterations=0)
   env[run] = Loc { ids: {ExprId(8)}, val: ref(PerRender) }
-- ComponentId(2) (iterations=0)
   state[0] = number[0, 0]   widened=false   shared=⊥
   env[n] = Val(number[0, 0])
   env[bag] = Loc { ids: {ExprId(4)}, val: ref(PerRender) }
-- ComponentId(3) (iterations=3)
   state[0] = number[0, inf]   widened=false   shared=number[1, inf]
   env[n] = Val(number[0, inf])
   env[run] = Loc { ids: {ExprId(8)}, val: ref(PerRender) }
stats: cache_hits=3 cache_misses=2 depth_capped={} unknown_refs={}
```

Dans `Child`, `f` pointe bien vers le site 5 (la fermeture), mais le tas de
l'enfant ne contient que le site 4 (l'objet `bag`) : `exec_var_callback`
ne trouve pas de `HeapValue::Fn` et n'exécute rien. CLI : seul `Child2`
reçoit `warn cross-component-infinite-loop … a state setter of parent
`Parent2``.

**(l) Composant récursif transmettant un autre setter** (`rec.tsx`) :

```tsx
import { useState, useEffect } from "react";
function Tree({ onPick, depth }: { onPick: (n: number) => void; depth: number }) {
  const [local, setLocal] = useState(0);
  useEffect(() => {
    onPick(local + 1);
  });
  return <div>{depth > 0 ? <Tree onPick={setLocal} depth={depth - 1} /> : null}{local}</div>;
}
export function Root() {
  const [x, setX] = useState(0);
  return <Tree onPick={setX} depth={3} />;
}
```

```
-- ComponentId(0) (iterations=1)
   state[0] = number[0, 1]   widened=false   shared=number[1, 1]
-- ComponentId(1) (iterations=0)
   state[0] = number[0, 0]   widened=false   shared=⊥
stats: cache_hits=3 cache_misses=1 depth_capped={} unknown_refs={}
```

(`0` = `Root`, `1` = `Tree`.) `local` reste `[0, 0]` : l'instance récursive
n'est pas analysée et `setLocal` n'est pas havoqué. Mais le CLI `--info`
le déclare : `info analysis-limit recursive component reference `Tree` is
not followed, so its props are treated as unknown; cross-component cycles
are not fully analysed (FN possible)` et `suspended analysis-limit 8
passing check(s) withheld`. Faux négatif **déclaré**, donc hors de la liste
D.

**(m) `useReducer` — D12** (`red.tsx`) :

```tsx
import { useReducer, useEffect } from "react";
function reducer(s: number, a: number) { return s + a; }
export function Red() {
  const [s, dispatch] = useReducer(reducer, 0);
  useEffect(() => {
    dispatch(1);
  });
  return <div>{s}</div>;
}
```

```
-- Red (intra, iterations=1)
   state[0] = number[0, 1]   widened=false
   env[s] = Val(number[0, 1])
   env[dispatch] = Val(setter(#4294967295#0))
```

`dispatch` est un setter ordinaire ; l'action `1` est écrite telle quelle,
le store converge sur `[0, 1]` et le CLI répond `✓ 1 file(s) no issues
found.` En React, `s` vaut 1, 2, 3, … et l'effet sans deps relance le render
indéfiniment.

**(n) Setter à deux sauts de fermeture vers un enfant inconnu — D13**
(`esc2.tsx`) :

```tsx
import { useState, useEffect } from "react";
import { Sheet } from "./ui";
export function TwoHopsObj() {
  const [n, setN] = useState(0);
  const inner = (v: number) => setN(v);
  const outer = (v: number) => inner(v);
  useEffect(() => {}, [n]);
  return <Sheet cfg={{ cb: outer }} />;
}
export function OneHopObj() {
  const [n, setN] = useState(0);
  const outer = (v: number) => setN(v);
  useEffect(() => {}, [n]);
  return <Sheet cfg={{ cb: outer }} />;
}
```

```
-- TwoHopsObj (intra, iterations=0)
   state[0] = number[0, 0]   widened=false
-- OneHopObj (intra, iterations=1)
   state[0] = ⊤   widened=false
```

(mode `prog` identique : `OneHopObj` ⊤, `TwoHopsObj` `[0, 0]`.) Contrôle
(`esc.tsx`) : quand la fermeture est passée **directement** en prop
(`<Sheet onX={outer}/>`), elle est aussi relevée en `HookEntry::Handler` et
exécutée comme point d'entrée (paramètres ⊤), ce qui rattrape l'écriture
(`TwoHops` : `state[0] = boolean`, soit `false ⊔ true`). CLI `--info` sur
`esc2.tsx` : pour les deux composants, `info analysis-limit component
`Sheet` was not found in the analysis registry. Pass its file on the command
line to analyse it (FN possible)` puis `suspended analysis-limit 10 passing
check(s) withheld`.

---

## 7. Contexte React nécessaire

- **Render pur, commit, effets.** Un composant fonction est rappelé à chaque
  render ; tout ce qu'il alloue (objets, tableaux, fonctions, éléments JSX)
  est neuf à chaque fois. Les effets (`useEffect`) s'exécutent **après** le
  commit, selon leurs deps. C'est ce qui fonde `ObjectLit/ArrayLit/FnLit →
  PerRender` et le découpage render → mémos → effets → handlers de la
  boucle (`fixpoint.rs:355-483`).
- **`useState` et ses setters.** L'identité du setter est stable
  (ADR-002 : « Stable (guaranteed by React) ») ; l'état ne change **que** par
  son setter, et React **abandonne** (*bail out*) le re-render si la nouvelle
  valeur est `Object.is`-égale à l'ancienne. D'où la conversion côté lecture
  `StateVal → Versioned` et le fait qu'écrire la même constante converge.
- **Batching et *functional updater*.** Plusieurs `setX` d'un même
  gestionnaire sont regroupés ; `setX(prev => f(prev))` reçoit la valeur la
  plus récente (après les updates en file), alors que `setX(x + 1)` lit la
  valeur capturée par la fermeture. L'analyse lie le paramètre de l'updater
  au **join du store** (`ctx.state.get(label)`), une sur-approximation de
  toute valeur en file ; elle n'essaie pas de modéliser l'ordre des updates.
- **`useReducer`.** `const [s, dispatch] = useReducer(reducer, init)` : la
  nouvelle valeur est `reducer(s, action)`, calculée par React au moment du
  traitement de l'action ; `dispatch` est stable comme un setter. L'analyse
  modélise `dispatch` exactement comme un setter mais **n'exécute pas le
  réducteur** : elle écrit l'action (D12, §6.9 (m)). Un troisième argument
  `init` de `useReducer(reducer, arg, init)` (initialiseur) n'est pas lu non
  plus (le lowering prend le deuxième argument comme valeur initiale,
  `hook_extractor.rs:716-717`).
- **Bail-out de `setState`.** React compare la nouvelle valeur à l'ancienne
  par `Object.is` et n'ordonne pas de nouveau render si elles sont égales
  (pour un état primitif ; un objet neuf est toujours différent). C'est ce qui
  rend terminante la boucle `useEffect(() => setN(5))` (sans deps) : dans
  l'analyse, le store vaut `[0, 5]` (initialiseur ⊔ écriture) après la
  première passe, la deuxième n'ajoute rien, `new_state ⊑ state` et la boucle
  s'arrête (`/tmp/dom05v/bail.tsx` : `iterations=1`, `state[0] =
  number[0, 5]`, CLI `✓`) — le point fixe abstrait est l'image de ce
  bail-out.
- **Ordre des effets et des renders.** Les effets d'un enfant s'exécutent
  avant ceux de son parent (ordre de commit, feuilles d'abord) ; l'analyse
  n'ordonne pas les effets : elle joint toutes les écritures d'une itération
  (sur-approximation), et une écriture d'un enfant n'est vue du parent qu'à
  la vérification de convergence suivante (`slice`, §3.8).
- **Initialiseur paresseux.** `useState(() => e)` exécute la fonction une
  fois au montage et stocke son **retour** (`fixpoint.rs:337-346`, TODO.md F2).
- **Comparaison des deps par `Object.is`.** Un effet/mémo se ré-exécute si
  une dep change selon `Object.is` : une primitive égale ne change pas, une
  nouvelle référence change toujours. D'où `recompute_memo` (join des
  stabilités) et la prudence sur `slice`/`concat` (chaîne → primitive).
- **Stabilité référentielle.** `useRef` rend le même conteneur à chaque render
  (`MarkerVal::StableRef`) ; `useCallback(f, [])` rend la même fonction ;
  `useMemo(f, [])` la même valeur. Un objet littéral qui *contient* une
  fonction stable est lui-même neuf (#88 : le conteneur est `PerRender`, le
  membre garde sa stabilité).
- **Callbacks et moment d'exécution.** `.then`, `setTimeout`,
  `queueMicrotask`, `requestAnimationFrame` s'exécutent plus tard mais
  **en conséquence** de l'effet qui les planifie (dans le cycle automatique) ;
  `arr.map(cb)` s'exécute tout de suite ; un `addEventListener` ou un
  `onClick` ne s'exécute que sur **événement externe** (hors cycle). C'est
  exactement la distinction `InCycle` / `Subscription` d'ADR-009.
- **Props et remontée d'état.** Un parent transmet ses setters à ses enfants
  (*lifting state up*) ; un enfant qui appelle un setter reçu en prop dans un
  effet sans deps crée une boucle parent → enfant → effet → parent
  (`cross-component-infinite-loop`, §6.7). Un composant enfant est
  ré-exécuté quand son parent re-render (sauf `React.memo`, non modélisé,
  #64).
- **Éléments JSX.** `<span/>` et `<Child/>` sont des appels à
  `React.createElement`/`jsx()` qui allouent un **nouvel objet élément** à
  chaque render ; React compare les éléments par type et par `key` pour la
  réconciliation, pas par identité. L'analyse les évalue en `ref(Stable)`
  (D8) : juste pour « la valeur ne porte pas d'information », faux pour
  `Object.is` dans un tableau de deps.
- **Composants récursifs et listes.** Un composant peut se rendre lui-même
  (arbres) ou être rendu `n` fois (`items.map(i => <Row …/>)`) : chaque
  instance a son propre état. L'analyse n'a qu'**un** résultat par
  `ComponentId` (`results`, D7) et coupe la récursion (Info
  `analysis-limit`, §6.9 (l)).
- **Strict Mode** (mentionné par `missing-cleanup`, §6.3) : en
  développement, React monte, démonte et remonte, donc exécute deux fois les
  effets de montage ; l'analyse ne le modélise pas explicitement dans ce
  sous-système.
- **Mutations.** Muter un objet d'état en place (`obj.f = v`) ne change pas
  son identité : React ne voit rien. L'interpréteur traite `MemberWrite`
  comme une mutation (identité conservée, champ joint), et la règle
  `state-mutation` le signale.
- **Contexte, Server Components.** Hors de ce sous-système : `useContext`
  n'est pas modélisé (⊤, issue #28, bloqué par la phase 2 intra de
  `analyze_program`) ; les Server Components relèvent de
  `server-component-hook` (dossier 13).
- **Sémantique de référence.** ADR-001 fixe React-tRace (Lee, Ahn, Yi, OOPSLA
  2025) comme sémantique concrète : *Tree Memory* + boucle de render
  (StepInit → StepEffect → StepCheck), règles SttReBind, CheckEffect,
  CheckNoEffect ; `Set_clos{label, path}` est le setter porteur de son
  composant, dont `SetterVal::One(component, label)` est l'abstraction.
  React-tRace ne couvre que `useState` et `useEffect` sans deps ; le reste
  (deps, `useMemo`, `useCallback`, `useRef`, objets) est une extension locale
  « without an equivalent formal guarantee ».

---

## 8. Subtilités, pièges, limites

### 8.1 Défauts de soundness observés (non tracés dans le tracker à la date du dossier)

Recherches effectuées : `gh issue list --state all --search` sur « expression
position setter », « exec_setter_call », « exec_body_impl », « cache »,
« results map », « call site overwrite » ; aucune issue ne décrit D1-D8
(l'issue fermée #130 traitait l'équivalent de D2 **dans la relation
*writers***, pas dans le `StateStore`). Ces constats sont à confirmer par le
mainteneur ; ils sont reproduits en §6.8. Les lignes D9 à D12 ont été
ajoutées à la relecture (2026-09-28), après reproduction (§6.9) ; recherche
complémentaire `gh issue list --state all --search "useReducer"` : aucune
issue sur le sujet (seule #128, rapport de campagne, sort).

| # | Où | Défaut | Effet observé |
|---|---|---|---|
| D1 | `interpreter.rs:413-426` | `exec_body_impl` calcule l'environnement d'un bloc comme le join des environnements d'**entrée** de ses prédécesseurs (`env_at.get(p)`) au lieu de son propre environnement d'entrée (`env_at[bid]`) | liaisons d'un bloc invisibles au bloc suivant : valeurs → ⊤ (sound), alias de setter / sites / callbacks → perdus (FN) ; diamants (ternaires) dans un updater → ⊤ |
| D2 | `interpreter.rs:86-89`, `:341-366` | `exec_setter_call` n'est appelé que pour `ExprStmt` et `Return` ; un appel de setter dans le membre droit d'un `Let`/`Assign` (dont le bras droit de `&&`/`\|\|`/`??` et de `?:`) ou en argument d'un autre appel n'écrit jamais dans le `StateStore` | `flag && setA(a + 1)` dans un effet sans deps : `infinite-loop` « verified » |
| D3 | `interpreter.rs:368-391` + `state_value.rs:135` | le second bras (setter porteur de propriétaire) tire aussi pour les setters **locaux** quand `inter` est présent, et évalue un updater `FnLit` comme `ref(PerRender)` au lieu d'exécuter son corps ; la fermeture est ré-importée par `slice` | `useEffect(() => { setN(c => c + 1); })` : signalé en intra (tests verts), muet en CLI |
| D4 | `state_value.rs:546-556`, `component_cache.rs:50-63` | clé du cache = props aplaties en `StateValue` ; deux fermetures différentes valent `ref(PerRender)` ; le second site d'appel n'est pas analysé | boucle croisée manquée quand l'enfant est réutilisé |
| D5 | `state_value.rs:604-616` | `eval_field_access` joint seulement les objets **qui ont** le champ ; les autres sites (autre branche, spread, receveur nullable) sont ignorés | `missing-deps` manqué (`PartialDeps`) ; `q?.f` annoncé sans `undefined` |
| D6 | `interpreter.rs:521-527`, `:241-245`, `:301-333` | B6 seulement pour un callee `Var` ; littéraux d'objets imbriqués jamais alloués | `o.f()` et `o.inner.f` : setter manqué |
| D7 | `state_value.rs:583-586` | `inter.results.insert(child, …)` écrase le résultat d'un site d'appel précédent de l'enfant | `<Counter step={1}/><Counter step={0}/>` : la boucle de la première instance disparaît |
| D8 | `state_value.rs:156`, `:482`, `:593` | `NativeElem` et `CompApp` valent `ref(Stable)` | `useEffect(…, [el])` avec `el = <span/>` non signalé |
| D9 *(relecture)* | `interpreter.rs:183-184` + tas insensible au flot | un `MemberWrite` sur un champ **absent** insère la seule nouvelle valeur (`(None, new) => new.clone()`), sans `undefined` ; comme le tas est partagé par tous les chemins, la lecture sur le chemin qui n'a pas écrit voit aussi la valeur | `if (flag) { o.g = 2 } const v = o.g;` → `v = [2, 2]` (§6.9) |
| D10 *(relecture)* | `abstract_env.rs:159-166` + `interpreter.rs:348-351` | une variable liée à deux setters sur deux chemins n'en garde qu'un (priorité à gauche du join) ; la valeur jointe (`SetterVal::Top`) ne satisfait pas `as_setter()` | `const s = c ? setA : setB; s(1)` → `b` jamais écrit (§6.9) |
| D11 *(relecture)* | `state_value.rs:565-573` | seuls les sites **directs** des props sont copiés dans le tas de l'enfant, pas les sites des membres d'un objet prop | fermeture dans un objet prop : boucle croisée manquée (§6.9) |
| D12 *(relecture)* | `src/lowering/hook_extractor.rs:715-719` (cause) + `interpreter.rs:348-366` (effet) | `useReducer` : le réducteur est ignoré, `dispatch(a)` écrit l'action `a` dans le slot | `useEffect(() => { dispatch(1); })` avec `s + a` : `✓` (§6.9) |
| D13 *(relecture, déclaré globalement)* | `state_value.rs:404-412`, `:427-467` | `collect_escaping_setters` ne suit pas un appel vers une autre fermeture du tas : un setter à deux sauts de fermeture d'une prop remise à un enfant inconnu n'est pas havoqué | `<Sheet cfg={{ cb: outer }}/>`, `outer → inner → setN` : `n = [0, 0]` ; Info `analysis-limit` « component … not found » émise (§6.9) |
| D14 *(relecture)* | `interpreter.rs:494-539` | `exec_callbacks_depth` ne classe que les `Call` ; l'exécuteur d'un `new Promise(fn)` (synchrone en JS) n'est jamais exécuté | `new Promise((resolve) => { setN(n + 1); … })` dans un effet sans deps : `✓` (§8.2) |

Remarques de nuance :

- D1 et D2 violent directement le principe « faux négatifs INTERDITS » de
  CLAUDE.md ; D5 aussi (la doc d'`obj_members` affirme le contraire :
  « an absent member falls back to the container's own value — sound »).
- D3 est un défaut d'**interaction** : chaque bras est raisonnable isolément.
  La correction centrale serait vraisemblablement de ne tirer le second bras
  que si le premier n'a pas tiré, ou si le propriétaire n'est pas
  `ctx.component` (« à vérifier » : le mainteneur tranchera).
- D4 et D7 relèvent de la **sensibilité au contexte** d'ADR-012 : le cache et
  la table `results` supposent que l'analyse d'un enfant ne dépend que des
  valeurs aplaties des props ; le tas (corps de fermetures) et la pluralité
  des sites d'appel sont oubliés.
- D8 pourrait être une décision implicite (éviter des FP sur `children`),
  mais aucun ADR ni commentaire ne la justifie ; à vérifier.

### 8.2 Autres pièges et subtilités

- **Deux interpréteurs de CFG** (issue #12) : `analyze_cfg` (render, effets,
  handlers : worklist, narrowing, widening) et `exec_body_impl` (tout corps
  imbriqué : une passe, ni narrowing ni widening). Une même expression peut
  donc être évaluée avec une précision très différente selon qu'elle est
  écrite dans un effet ou dans le `setTimeout` de cet effet.
- **Le store n'est pas ce que lit le render** : `state_store.get(l)` est la
  vue événement (join des écritures, fraîcheur comprise) ; `StateVal(l)` est
  la vue inter-renders (`Versioned`). Une règle qui lit l'une à la place de
  l'autre se trompe (ADR-017 le documente comme invariant).
- **Défauts inverses des stores** : `AbstractEnv` et `MemoStore` → ⊤,
  `StateStore` et `SharedStateStore` → ⊥. La cohérence tient parce que
  chaque slot d'état est amorcé ; une lecture d'un label d'état jamais amorcé
  (hook mal extrait) lirait ⊥, soit un chemin mort.
- **Les sites d'une variable ne sont jamais retirés** : `let f = () => a();
  f = () => b();` → `f` désigne `{site1, site2}` ; les deux corps sont
  exécutés (sur-approximation) — y compris après une réaffectation qui rend
  le premier inatteignable.
- **Priorité à gauche des liaisons de setter au join** : si une même variable
  est liée à deux setters différents sur deux chemins, le second est oublié
  (`abstract_env.rs:159-162`). En pratique le lowering passe par un
  temporaire `__tN` du diamant, dont la *valeur* joint deux `SetterVal::One`
  en `SetterVal::Top`. **Vérifié par le relecteur** (`/tmp/dom05v/sj.tsx`,
  §6.9) : sur `const s = c ? setA : setB; s(1);` dans un effet, les deux
  bras du ternaire passent par `bind_rhs` (`Let __tN = Var(setA)` /
  `Var(setB)`, alias de setter), le join garde **une seule** liaison
  (priorité à gauche) et `s` hérite de celle-ci ; le premier bras
  d'`exec_setter_call` n'écrit que dans ce slot-là (`a = [0, 1]`), le second
  bras ne tire pas (`as_setter()` = `None` sur la valeur jointe) et
  **`b` reste `[0, 0]`** en intra comme en programme (D10, §8.1). Le résultat
  dépend en outre de l'ordre `self`/`other` du join, donc de l'ordre de
  traitement des blocs.
- **Le paramètre unique de l'updater** : seul le premier paramètre est lié
  (`params.first()`) ; un updater à paramètres déstructurés
  (`setX(({ a }) => …)`) passe par la déstructuration du corps.
- **`setX()` sans argument** écrit ⊤ (JS : `undefined`).
- **Le tas est écrasé par ré-allocation** : une fermeture ré-allouée à
  l'itération suivante remplace ses captures ; le tas n'entre pas dans le
  test de convergence (seul `StateStore::leq`).
- **Le plafond de profondeur n'est déclaré qu'en inter** (`if let
  Some(inter) = ctx.inter`, `interpreter.rs:485-491`) : une analyse intra
  (phase 2, tests) le subit en silence.
- **Nominalité de `classify_callee`** : un objet quelconque avec une méthode
  `then`/`map` est traité comme une promesse/un tableau (sur-approximation,
  FP possible) ; un `.subscribe(cb)` custom est `Unknown` (FN accepté).
- **`new` n'est jamais classé** (relecture) : `exec_callbacks_depth` n'a de
  bras que pour `Expr::Call` ; un `Expr::New` passe par la descente
  générique, et `for_each_child` ne descend jamais dans un `FnLit`
  (`src/ir/expr.rs:463-473`). L'exécuteur de `new Promise((resolve) => { … })`
  — que JS exécute **synchroniquement** — n'est donc jamais exécuté.
  Vérifié (`/tmp/dom05v/np.tsx`) : `useEffect(() => { new Promise((resolve)
  => { setN(n + 1); resolve(null); }); })` donne `n = [0, 0]`, CLI `✓`, aucune
  Info (D14, §8.1). Le cas fréquent `new Promise((r) => setTimeout(r,
  100)).then(() => setN(…))` n'est pas touché : le `.then` est un `Call`
  `InCycle` et son callback est descendu.
- **`!x` rend ⊤ sur un non-booléen**, alors que les comparaisons rendent
  `boolean` : une garde `if (!n)` sur un nombre ne profite pas du
  raffinement « le résultat est un booléen » (le narrowing de véracité
  opère de toute façon sur `n` lui-même, dans `analyze_cfg`).
- **Templates** : dès qu'une interpolation n'est pas une chaîne connue, la
  valeur est ⊤ (et non `string`).
- **`eval_comp_app` rend toujours `ref(Stable)`**, y compris quand l'enfant
  est inconnu ou récursif : la valeur d'un élément n'encode pas l'échec.
- **Récursion sans havoc** : un composant récursif qui reçoit un setter du
  parent n'est pas havoqué (`state_value.rs:520-528` retourne avant tout
  havoc). **Vérifié par le relecteur** (`/tmp/dom05v/rec.tsx`, §6.9) : cela
  sous-approxime bien quand l'appel récursif transmet un **autre** setter que
  celui reçu (`<Tree onPick={setLocal} …/>` dans `Tree`) : `local` reste
  `[0, 0]` alors que l'instance intérieure l'incrémente. Ce faux négatif est
  en revanche **déclaré** : Info `analysis-limit` « recursive component
  reference `Tree` is not followed, so its props are treated as unknown; … (FN
  possible) » et 8 vérifications suspendues. (Le texte de l'Info dit « props
  treated as unknown », ce qui n'est pas ce que fait le code : aucune prop
  n'est évaluée ni havoquée pour l'instance récursive.)
- **Le tas est insensible au flot** : `analyze_cfg` passe le même `&mut Heap`
  à tous les blocs (`cfg_analyzer.rs:69-76`), et `analyze_component_impl` le
  même tas à toutes les passes et itérations (`fixpoint.rs:301`). Un
  `MemberWrite` fait sur une branche est donc lu sur toutes les autres (c'est
  une sur-approximation tant que le champ existait, D9 sinon), et une
  ré-exécution de `const o = { … }` (`bind_rhs` → `heap.insert`, écrasement)
  **efface** les écritures de champ accumulées jusque-là sur ce site. Seul
  l'environnement (`AbstractEnv`) est sensible au flot.
- **`useReducer` : le réducteur est ignoré** (hors périmètre pour la cause,
  dans le périmètre pour l'effet). Le lowering extrait
  `useReducer(reducer, init)` en `HookEntry::State { init }` en sautant le
  réducteur (`src/lowering/hook_extractor.rs:715-719`, `let _reducer =
  it.next(); // skip reducer fn`) et `dispatch` est lié comme un setter
  (`StateSetter`). `exec_setter_call` écrit donc **l'action** dans le slot,
  pas `reducer(state, action)`. **Vérifié par le relecteur**
  (`/tmp/dom05v/red.tsx`, §6.9) : `useEffect(() => { dispatch(1); })` avec
  `reducer = (s, a) => s + a` donne `s = [0, 1]`, CLI `✓` — une vraie boucle
  infinie déclarée sûre (D12, §8.1). Aucune mention dans
  `docs/limitations.md` ni dans le tracker (recherche « useReducer » au
  2026-09-28).
- **`inter = None` dans les passes post-convergence** (`fixpoint.rs:540-541`,
  `:590`) : ni inlining d'enfant ni écriture partagée lors du rafraîchissement
  et du rejeu des effets.

### 8.3 Limites documentées (`docs/limitations.md`)

- « Hooks and callees it cannot reach: npm-package callees and utilities cut
  off by the inlining depth #19 ; … a setter called through an index or a
  returned function #46. A setter *call* in any expression position,
  `wrap(setN(1))`, a ternary arm, a JSX prop, is seen; it is the callee's
  *inlining* that is not. » — vrai pour la relation *writers* (#130), **pas**
  pour le `StateStore` (D2).
- « Loop-carried values inside callbacks are computed without the
  loop-carried contribution #21. »
- « By decision: `arr.slice()` and `arr.concat()` in a deps array are not
  proven fresh … #22. »
- « Intervals never hold `NaN`, so an arithmetic result that may be `NaN` is
  ⊤: every `/`, a `%` whose divisor may be zero …, a `**` with a negative base
  or a negative exponent. `in` and `instanceof` are known to be booleans and
  nothing more #73. »
- « A spread or computed key is kept for its reads but not modeled, so `{
  ...opts }.foo` does not resolve and a setter forwarded through
  `f(...handlers)` is not seen #76. »
- « Cross-component rules need the parent to be reached top-down. A parent
  analyzed only intra leaves `cross-component-infinite-loop` silent #20. »
- « `useContext` is unmodelled, so a context value reads ⊤ … #28. »

### 8.4 Issues ouvertes pertinentes

`#12` (deux interpréteurs), `#19` (callees inconnus sans `Loc`), `#20` (parent
intra seulement), `#21` (valeurs portées par une boucle dans un callback),
`#22` (`slice`/`concat`), `#28` (`useContext`), `#46` (setters échappés
résolus sur un niveau), `#52`/`#53`/`#56`/`#57` (inlining d'utilitaires),
`#64` (`React.memo`/`forwardRef`), `#76` (spreads, clés calculées), `#7`
(identité de composant, partiellement résolu par `806d114`), `#14` (les tests
d'intégration construisent `Config::default()`, « which no user ever runs » —
et, on l'a vu en §6.2, souvent le chemin intra au lieu du chemin programme).
`docs/TODO.md` n'est plus qu'une redirection vers le tracker.

---

## 9. Glossaire

| Terme | Définition | Où |
|---|---|---|
| **fonction de transfert** | fonction qui calcule l'effet abstrait d'une expression ou instruction sur l'état abstrait | trait `Transfer`, `src/domains/mod.rs:114-162` |
| **`StateValue`** | valeur abstraite produit de huit emplacements (nombre, booléen, chaîne, référence, null, undefined, setter, autre) | `src/domains/impls/state_value.rs:26-44` (dossier 04) |
| **emplacement** (*slot* de valeur) | une composante du produit `StateValue` | idem |
| **slot d'état** | un `useState`/`useReducer`, identifié par son `HookLabel` (et son composant) | `StateStore`, `SharedStateStore` |
| **`HookLabel`** | numéro d'un hook dans son composant | `src/ir/types.rs` |
| **site (d'allocation)** | le nœud IR qui alloue (`ObjectLit`, `ArrayLit`, `FnLit`, `New`), identifié par un `ExprId` stable entre itérations | `src/ir/types.rs:11-25`, ADR-010 |
| **`Loc`** | variante d'`EnvVal` : ensemble de sites + valeur | `abstract_env.rs:23-26` |
| **tas abstrait** | `ExprId → HeapValue` (fermetures avec captures, objets par membre) | `heap.rs:17-34` |
| **captures** | valeurs des variables libres d'une fermeture à son allocation | `HeapValue::Fn::captured`, `heap.rs:21-26` (champ), `:57-61` (calcul) |
| **mise à jour faible** (*weak update*) | écrire par join plutôt que remplacer | `StateStore::update`, `SharedStateStore::update`, `MemberWrite` |
| **vue événement / vue inter-renders** | le store tient le join des écritures ; une lecture de `StateVal` rend la version (`Versioned`) | `state_value.rs:122-134`, ADR-017 |
| **conversion côté lecture** (*read-side conversion*) | remplacement de l'emplacement référence par `Versioned({(c, l)})` à la lecture d'un état | idem |
| **`Versioned(S)`** | « ne change qu'aux écritures des slots de S » (borne *may*) | `stability.rs:41-44` |
| **`PerRender`** | « référence neuve à chaque render » (borne *must*) | `stability.rs:47-49` |
| **must / may** | fait vrai sur toute exécution / sur au moins une exécution possible ; `PerRender` est must, `Versioned` est may, le store est may | ADR-017 |
| **classe de déclenchement** (*trigger class*) | ce qui déclenche un callback : `Setter`, `InCycle`, `Subscription`, `Unknown` | `callbacks.rs:6-23` |
| **dans le cycle** (*in-cycle*) | exécuté en conséquence du render/effet courant (HOF synchrones, promesses, timers) | ADR-009 §2 |
| **point d'entrée** | CFG analysé séparément : render, effet, handler | ADR-009 §3 |
| **pré-passe** | descente d'une expression pour exécuter les callbacks in-cycle avant l'effet principal | `exec_callbacks_depth`, `interpreter.rs:477-540` |
| **B5 / B6** | callback passé **par variable** (`setTimeout(cb)`) / appel **direct** d'une fonction locale (`load()`), résolus par le tas | ADR-010 §6-7, `exec_var_callback` |
| **profondeur d'inlining** | nombre de corps imbriqués en cours, plafonné à `MAX_INLINE_DEPTH = 3` | `interpreter.rs:21` |
| ***functional updater*** | argument fonction d'un setter, `setX(prev => …)` | `exec_setter_call`, `interpreter.rs:352-361` |
| **setter porteur de propriétaire** | `SetterVal::One(component, label)`, ex-`ComponentSetter` | ADR-012 §7, ADR-015 |
| **havoc** | joindre ⊤ dans un slot dont le setter s'échappe vers un enfant inanalysable | `havoc_setter_props`, `state_value.rs:265-297` |
| **setter qui s'échappe** (*escaping setter*) | setter atteignable depuis une valeur remise à un enfant | `collect_escaping_setters`, `:315-367` |
| **tranche** (*slice*) | les entrées du `SharedStateStore` d'un composant, sous forme de `StateStore` | `shared_state_store.rs:57-66` |
| **inlining descendant** (*top-down*) | analyse de l'enfant au point `CompApp` du parent, avec les props abstraites | ADR-012 §1, `eval_comp_app` |
| **cache des composants** | `(ComponentId, props abstraites) → résultat`, égalité par double ⊑ | `src/engine/component_cache.rs` |
| **graphe d'appels** | arêtes parent → `CallSite{callee, props, location}` | `record_call_site`, `state_value.rs:691-706` |
| **projection mémo** | valeur d'un mémo = `ref(⨆ stabilités des deps)` | `recompute_memo`, `:56-101` |
| **motion-wins** | règle de `to_stability` : un emplacement en mouvement domine | dossier 04 |
| **Shape** | résumé de hook dont seuls les membres nommés portent une promesse | `SummaryValue::Shape`, `src/ir/expr.rs:365-368` |
| **marqueur de hook** | `Expr::HookMarker(label, MarkerVal)` : ancre le site d'appel d'un hook sans valeur suivie | `src/ir/expr.rs:287-294` |
| **analysis-limit** | Info émise quand l'analyse a coupé (profondeur, enfant inconnu, récursion) | `src/rules/impls/analysis_limit_info.rs` |
| **churn, reviver, seed, guard, witness, anchor** | termes des relations et des règles, **hors périmètre** : churn (écriture fraîche qui relance un effet, `src/engine/churn.rs`), reviver (écriture qui ranime une garde dans la preuve de convergence, `src/engine/guards.rs`, `churn.rs`), seed (relation d'amorçage d'un slot, `src/engine/seeds.rs:51`, ADR-031), guard / anchor (vocabulaire des packs déclaratifs, `src/rules/declarative/schema.rs:111`, `:312`), witness (chaîne de preuve d'un diagnostic, ADR-019) | dossiers 07, 11, 12 |
| **`EnvVal::Val` / `EnvVal::Loc`** | vue d'une liaison : valeur seule / sites d'allocation + valeur | `abstract_env.rs:15-27` |
| **liaison de setter / de callback** | tables annexes de l'environnement : variable → label d'un `useState` / d'un `useCallback` ; lues « rien par défaut », jamais ⊤ | `abstract_env.rs:55-59`, `:111-128` |
| **environnement vide** (`AbstractEnv::bottom()`) | ⊥ pour `leq`, mais « tout ⊤ » en lecture et non neutre pour `join` | `abstract_env.rs:210-213`, §3.5 |
| **`ExprId::fresh()`** | site d'allocation synthétique (compteur atomique ≥ 1 000 000 000), utilisé pour l'objet des props d'un enfant | `src/ir/types.rs:17-25`, `state_value.rs:563` |
| **`ComponentId::SYNTHETIC`** | identité d'un composant analysé seul (tests, sonde intra) : `ComponentId(u32::MAX)`, affiché `4294967295` | `src/ir/component_id.rs:38`, `context.rs:113-117` |
| **`ChildLookup`** | réponse du registre à la résolution d'un nom JSX : `Resolved(ComponentKey)`, `Unknown` (nom jamais abaissé : npm, fichier hors du run), `Ambiguous` (plusieurs fichiers le définissent) | `src/engine/component_registry.rs:10-20`, `state_value.rs:489-518` |
| **`AnalyzeChildFn`** | pointeur de fonction vers `analyze_component_inter`, qui casse la dépendance circulaire `domains::transfer` ↔ `engine::fixpoint` | `context.rs:17-25` |
| **`QueryContext` / `FixpointCtx` / `NullCtx`** | requêtes inter-domaines ; seule requête : `callback_body(label)` ; `FixpointCtx` la sert depuis la table des corps de `useCallback`, `NullCtx` répond `None` | `context.rs:30-49`, `:154-171` |
| **phase 1 / phase 2** | `analyze_program` : racines analysées avec `InterCtx` (enfants inlinés) / composants non atteints analysés seuls, `inter = None` | `fixpoint.rs:723-795` |
| **diamant** | forme de CFG d'un `?:`, `&&`, `\|\|`, `??` : bloc `Branch`, deux blocs qui affectent un temporaire `__tN`, bloc de jonction qui lit `Var(__tN)` | `expr_lower.rs:675-795` |
| **plancher int32 / uint32** | plage garantie de tout résultat bit à bit (`[-2^31, 2^31-1]`, `[0, 2^32-1]` pour `>>>`) | `state_value.rs:813-834` |
| **`ToNumber`** | coercition numérique JS ; modélisée par `as_arith` (`null` → 0) et `coerce_to_number` (booléens, chaînes parsables) | `state_value.rs:708-732`, `:1006-1040` |
| **seuil `StrConst`** | au-delà de 4 chaînes (`STR_WIDEN_THRESHOLD`), un ensemble de chaînes devient `string` (⊤ de l'emplacement) | `src/domains/impls/str_const.rs:8` |
| **`SetterVal::One` / `Top`** | setter d'identité connue `(composant, label)` / setter inconnu (« which one was lost at a join ») ; seul `One` sans autre sorte satisfait `as_setter()` | `src/domains/impls/setter_val.rs:16-23`, `impls/state_value.rs:270-271` |
| **tas insensible au flot** | un seul tas mutable par composant, partagé par tous les blocs, chemins, passes et itérations ; seul l'environnement suit le flot | `fixpoint.rs:301`, `cfg_analyzer.rs:69-76`, §8.2 |
| **garde-fou des 100 itérations** | au-delà de 100 itérations, `StateStore::widen` sur tous les labels puis arrêt | `fixpoint.rs:500-512` |

---

## 10. Plan pédagogique suggéré

### 10.1 Prérequis

- Dossier 03 (IR : `Expr`, `Stmt`, CFG, `HookEntry`, `ExprId`).
- Dossier 04 (domaines : `StateValue`, `Stability` avec `Versioned`,
  intervalles, `StrConst`, `to_stability`).
- Notions d'interprétation abstraite : treillis, join, widening,
  sur-approximation, mise à jour faible.
- Le chapitre sur la boucle de point fixe (dossier 06) peut venir **après**
  : ce chapitre-ci se lit comme « la sémantique d'un pas », l'autre comme
  « l'itération des pas ».

### 10.2 Ordre d'exposition

1. **Le problème** : « `setN(n + 1)` dans un effet sans deps — l'analyseur
   doit savoir que `n` grandit ». Qu'est-ce qu'évaluer `n + 1`
   abstraitement ? (exemple §6.1).
2. **L'environnement et l'évaluateur d'expressions** : `AbstractEnv::lookup`
   (⊤ par défaut), littéraux, `Var`, opérateurs arithmétiques avec
   `ToNumber(null)`, comparaisons, `typeof`, bitwise. Exercices de calcul à
   la main.
3. **Ce que le lowering a déjà fait** : templates, ternaires et `&&` en
   diamants, `?.`, spreads (tableau §4.1.9). Montrer qu'un ternaire n'est pas
   évalué par l'évaluateur mais par le CFG.
4. **Les stores** : `StateStore` (⊥, join, amorçage), `MemoStore` (⊤,
   écrasement), et le tableau comparatif §3.12 ; ADR-002 et ce qui en reste.
5. **L'instruction** : `bind_rhs`, `ExprStmt` → `exec_setter_call`, mise à
   jour faible ; le *functional updater* (§6.2, test unitaire).
6. **La conversion côté lecture** et la double vue (ADR-017), illustrée par
   `obj` vs `state[1]` au §6.5.
7. **Les callbacks** : le piège de l'`onClick` (ADR-009), `TriggerClass`, la
   pré-passe, `exec_body_impl` (une passe, arcs retour), exemple §6.3.
8. **Le tas** : sites d'allocation, `EnvVal::Loc`, `alloc_fn` et captures,
   `resolve_locs`, B5/B6, membres d'objets (#88), exemple §6.4.
9. **Les mémos** : `recompute_memo` et sa table (§6.5).
10. **L'inter-composants** : `eval_comp_app`, props → tas enfant,
    `SharedStateStore`, havoc, cache (§6.6, §6.7).
11. **Soundness en pratique** : le tableau §4.7 puis les défauts D1-D14
    (§6.8, §8.1) comme étude de cas : pour chacun, identifier l'hypothèse
    violée.

### 10.3 Idées de schémas

- **Flot d'appel** : `analyze_cfg → exec_stmt → exec_full_stmt → {pré-passe,
  cœur} → eval_expr`, avec les retours de profondeur (diagramme §4.3.0 à
  mettre au propre en TikZ).
- **Carte des stores** : cinq boîtes (env, state, memo, heap, shared) avec
  leurs défauts (⊤/⊥), leur mode de mise à jour (fort/faible) et leur portée
  (point / composant / programme).
- **Tas** : un environnement `{bag ↦ Loc{2}}`, le tas `{2 ↦ Obj{onClear ↦
  Val(ref(Stable)), label ↦ Val({"x"})}}`, et la résolution `bag.onClear`.
- **Inter-composants** : parent et enfant côte à côte, flèche descendante
  (props, copie de tas), flèche montante (`SharedStateStore`), boucle de
  convergence du parent.
- **Diamant d'un ternaire** dans `exec_body_impl`, avec `env_at` annoté,
  pour montrer D1 (quel environnement arrive au bloc de jonction).
- **Treillis `TriggerClass` → politique** (tableau d'ADR-009 §3).

### 10.4 Exercices

1. Calculer à la main `eval_binop(Add, number[0,3]|null, number[1,1])`, puis
   `eval_binop(BitAnd, ⊤, number[7,7])`, `eval_unary(TypeOf, null)`,
   `eval_unary(Plus, str{"5","6"})`.
2. Pour `const t = `id-${n}`;`, donner l'IR produit par le lowering et la
   valeur abstraite ; proposer une amélioration sound (réponse : `str_top()`
   au lieu de ⊤).
3. Dérouler `setN(c => c + 1)` sur un store `{0 ↦ [0, 2]}` : quelle valeur
   écrit-on ? pourquoi le résultat contient-il encore `0` ?
4. Classer `promise.then(f)`, `arr.forEach(f)`, `el.addEventListener("x", f)`,
   `myLib.subscribe(f)`, `Array.from(xs, f)`, `router.from(f)` et justifier
   la politique de chacun.
5. Pourquoi `AbstractEnv` a-t-il deux tables `stabs` et `locs` ? Construire
   le contre-exemple d'ADR-010.
6. Montrer qu'avec `MAX_INLINE_DEPTH = 3`, `f4()` n'atteint pas `setD` dans
   §6.3 ; où l'utilisateur en est-il averti ? Pourquoi ne l'est-il pas en
   analyse intra ?
7. Expliquer pourquoi `useMemo(() => n * 2, [n])` vaut `ref(PerRender)` et
   `useMemo(() => obj.a, [obj])` vaut `ref(Versioned({1}))`.
8. Sur `/tmp/dom05/ex14.tsx`, expliquer pourquoi la boucle n'est pas vue, et
   proposer une clé de cache qui la verrait (indice : les sites des props).
9. Localiser la ligne d'`exec_body_impl` responsable de D1 et écrire la
   correction d'une ligne ; quel test unitaire écrire pour qu'elle ne
   régresse plus ?
10. Pour D2, proposer une correction **centrale** (principe « pas de
    workaround ») : où tirer `exec_setter_call` pour qu'un setter soit pris en
    compte dans toute position d'expression, sans double tir ?
11. (relecture) Sur `/tmp/dom05v/red.tsx`, expliquer pourquoi `dispatch(1)`
    converge alors que React boucle ; quelle information manque à
    `HookEntry::State` pour modéliser `useReducer` ?
12. (relecture) Montrer que `AbstractEnv::bottom()` n'est pas neutre pour
    `join` : calculer `bottom().join(&e)` pour `e = {x ↦ [1,1]}`, puis
    expliquer pourquoi `exec_body_impl` n'est pas affecté.

---

## Vérification

Relecture-vérification du 2026-09-28, dépôt au commit `e67b10a` (arbre de
travail propre hors `docs/manuscrit/`), binaire `target/debug/reactant`
reconstruit (`cargo build`, aucun changement), sonde reconstruite dans
`/tmp/dom05/target`. Seul ce fichier a été modifié.

**Méthode.**

- Les **47 extraits `rust`** du dossier ont été comparés automatiquement,
  caractère pour caractère, aux lignes citées (`chemin:Ldébut-Lfin`) : tous
  identiques après correction.
- Toutes les tables de lignes de §2 ont été recalculées par `grep -n` des
  définitions ; les références ponctuelles de §1, §3, §4, §8 et du glossaire
  vers `fixpoint.rs`, `cfg_analyzer.rs`, `context.rs`, `domains/mod.rs`,
  `expr_lower.rs`, `hook_extractor.rs`, `component_cache.rs`,
  `analysis_limit_info.rs`, `eval.rs`, `written.rs`, `infinite_loop.rs`,
  `ir/{expr,stmt,types,cfg}.rs` ont été relues par `sed -n`.
- Tests relancés : `cargo test -q --lib domains::transfer` → 44 passed,
  `domains::interp` → 4 passed, `domains::stores` → 30 passed (identique au
  dossier).
- **Tous les exemples** de §6.1 à §6.8 (`/tmp/dom05/ex1, 3, 4, 6, 7, 8, 9,
  10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 22, 23, 24`) ont été rejoués
  (sonde `intra` et `prog`, CLI) : sorties identiques à celles citées, sauf
  les chemins affichés (le CLI affiche le chemin tel que passé en argument).
- Citations d'ADR vérifiées par `grep -F` dans `docs/adr/`.

**Corrigé.**

- `interp/cfg.rs` : `topo_sort` `:3-9` → `:5-11`, `dfs_post` `:11-24` →
  `:13-26` (§2.6, et la référence de l'extrait).
- `stores/heap.rs` : toutes les références étaient décalées de 7 lignes —
  `HeapValue`/`Heap` `:10-27` → `:17-34`, `alloc_fn` `:38-63` → `:45-70`,
  `insert` `:34-36` → `:41-43`, opérations de treillis `:73-122` →
  `:80-131`, `resolve_locs` `:127-158`/`:135-158` → `:134-165`, doc de
  `resolve_locs` `:127-134` → `:134-141`, doc d'`alloc_fn` `:38-43` →
  `:45-49`, captures `:62-65`/`:44-62` → `:58-61` / `:21-26` + `:57-61`
  (§3.1, §2.11, §3.9, §5.8, glossaire). §2.11 donne désormais la ligne de
  chaque méthode.
- Tests bit à bit `:1237-1302` → `:1229-1312` (liste complète) ; test ⊥
  `:1296-1302` → `:1298-1302` ; test d'échappement `:1406-1447` →
  `:1405-1447`, avec la nuance que ses « six niveaux » sont des `ObjectLit`
  emboîtés et non une chaîne de fermetures.
- `lower_class` `:60-135` → `:58-135`.
- Citation ADR-012 : « no separate program-level fixpoint layer » →
  « No separate program-level fixpoint layer. » (ADR-012 l. 82).
- §6.2 : les options CLI nécessaires aux lignes citées (`--info --trace
  --show-clean`) sont précisées.
- §6.0 : l'emplacement réel de la sonde (reconstruite dans
  `/tmp/dom05/target`) et du binaire `cargo` est précisé.
- Liste des tests de `state_store.rs` complétée (8 tests).
- Glossaire : ligne `SetterVal` et `ChildLookup` précisées avec leurs
  définitions exactes.

**Ajouté.**

- §3.13 : inventaire exhaustif des items publics / `pub(crate)` /
  `pub(super)` du périmètre avec leurs appelants de production (dont les
  homonymes `classify_callee` hors périmètre, `AbstractEnv::contains` utilisé
  seulement par `missing-deps`, `Heap::join/widen/leq` jamais appelés),
  l'usage exact de chaque méthode de `StateStore` par la boucle (dont le
  garde-fou des 100 itérations et `StateStore::widen`), et le rôle
  d'`AnalysisCtx::null` (amorçage `useState` avec tas neuf, `inter = None`).
- §3.5 : `AbstractEnv::bottom()` n'est pas neutre pour `join`.
- §3.6 : remarques sur `leq` et `changed_labels`.
- §4.1.6 : détails exacts de `const_of`, `shift_amount`, `all_ones_above`,
  `>>`.
- §4.1.7 : `coerce_to_number` utilise le parse Rust, pas `ToNumber`
  (écarts sound).
- §4.2 : D13 (setter à deux sauts de fermeture non havoqué).
- §4.3.1 : D9 confirmé (champ nouveau) ; aggravation de D2 pour un updater
  en position de liaison.
- §4.1.10 : D11 confirmé (fermeture membre d'un objet prop non copiée).
- §4.6 : capacité du cache (5) et comportement de l'éviction.
- §6.9 : six exemples nouveaux reproduits (`/tmp/dom05v/mw.tsx`, `sj.tsx`,
  `deep.tsx`, `rec.tsx`, `red.tsx`, `esc2.tsx`, plus les contrôles
  `esc.tsx`, `np.tsx`, `bail.tsx`).
- §7 : `useReducer`, bail-out `Object.is` (avec exemple vérifié), ordre des
  effets, éléments JSX, composants récursifs et listes.
- §8.1 : défauts D9 à D14 ; §8.2 : tas insensible au flot, `useReducer`,
  `new` jamais classé (D14), confirmation de la perte d'un setter au join
  (D10) et de la récursion sans havoc (FN **déclaré** par l'Info
  `analysis-limit`).
- §9 : 17 entrées de glossaire (liaisons, environnement vide,
  `ExprId::fresh`, `SYNTHETIC`, `ChildLookup`, `AnalyzeChildFn`,
  `QueryContext`, phases 1/2, diamant, planchers int32, `ToNumber`, seuil
  `StrConst`, `SetterVal`, tas insensible au flot, garde-fou des 100
  itérations…).
- §10.4 : exercices 11 et 12.

**Reste incertain.**

- D1 à D14 sont des constats reproduits, pas des diagnostics confirmés par le
  mainteneur ; certains peuvent relever d'une décision non écrite (D8 surtout,
  et D12 si `useReducer` est hors du périmètre voulu malgré ADR-005 qui le
  range parmi les hooks « always active »).
- L'éviction du cache des composants (résultat non joint) n'a pas été
  reproduite par un exemple : « à vérifier ».
- Le texte de l'Info de récursion (« its props are treated as unknown ») ne
  correspond pas au code (aucune prop évaluée ni havoquée) ; laissé tel quel
  car hors périmètre (règle `analysis-limit`).
- ADR-010 affirme encore « `IndexAccess` is still ⊤ », rendu faux par #37
  (index constant = membre) ; noté ici, non corrigé (ADR hors périmètre du
  dossier).
- L'issue #158 (`new X()`) est toujours **ouverte** dans le tracker alors que
  le commit `e67b10a` la cite et que `Expr::New` existe : statut à confirmer.
- Les notes « à vérifier » de §4.4 (élisions vs spreads pour
  `deps.is_empty()`, levée par lecture de `expr_lower.rs:395-438`, non
  rejouée en CLI) et de §3.5 (priorité à gauche des liaisons de setter,
  levée par l'exemple D10) sont closes. Restent « à vérifier » : la
  correction de D3 (§8.1, à trancher par le mainteneur) et le caractère
  voulu ou non de D8.
- Les exemples `/tmp/dom05/ex2.tsx`, `ex5.tsx`, `ex21.tsx` existent mais ne
  sont cités nulle part dans le dossier ; ils n'ont pas été examinés.