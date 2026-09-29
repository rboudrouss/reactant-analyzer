# Dossier 04 — Domaines abstraits : treillis de stabilité, intervalles, `StateValue` produit, constantes de chaînes, booléens, setters, contexte

> Matière première pour le manuscrit. État du dépôt : `main` au commit
> `e67b10a` (2026-09-27). Tous les extraits sont verbatim et référencés
> `chemin:Ldébut-Lfin`. Les sorties citées ont été obtenues le 2026-09-28 avec
> `cargo run -q -- check …` (binaire `reactant`) et avec une **sonde
> temporaire** (§6.0) qui affiche les valeurs abstraites convergées. Ce qui n'a
> pas pu être vérifié est marqué **« à vérifier »**. Une passe de
> relecture-vérification (extraits, références, exemples rejoués, items
> publics) a eu lieu le 2026-09-28 : voir la section finale « Vérification ».

Périmètre lu intégralement : `src/domains/mod.rs` (162 l.),
`src/domains/context.rs` (171 l.), `src/domains/impls/mod.rs` (65 l.),
`src/domains/impls/stability.rs` (367 l.), `src/domains/impls/interval.rs`
(614 l.), `src/domains/impls/state_value.rs` (1262 l.),
`src/domains/impls/str_const.rs` (153 l.), `src/domains/impls/bool_val.rs`
(13 l.), `src/domains/impls/setter_val.rs` (72 l.) ; ADR-002, ADR-008, ADR-014,
ADR-015, ADR-017 (plus ADR-001, ADR-007, ADR-020 pour le contexte) ; commit
`548f922`.

Suivis dans les modules voisins (lus partiellement, cités quand utile) :
`src/domains/transfer/state_value.rs` (évaluateur, `eval_binop`,
`recompute_memo`), `src/domains/stores/{state_store,memo_store,mod}.rs`,
`src/engine/cfg_analyzer.rs` (`analyze_cfg`, `narrow_env_for_branch`),
`src/engine/fixpoint.rs` (`Config`, amorçage, boucle de convergence,
`collect_thresholds`), `src/engine/written.rs` (`value_freshness`),
`src/rules/api/query.rs` (`StabilityVerdict`), `src/rules/helpers/mod.rs`
(`describe_value`), `src/rules/impls/infinite_loop.rs` (portes du bras
point fixe). Tests : blocs `#[cfg(test)]` des fichiers du périmètre (103 tests,
tous verts : `cargo test -q --lib domains::impls` → `103 passed`),
`tests/narrowing.rs` (22 tests), `tests/widening_e2e.rs` (4 tests) +
`tests/fixtures/widening.tsx`, `tests/always_unstable_deps.rs`.

---

## 1. Rôle et position dans le pipeline

### 1.1 Ce que fait le sous-système, en une phrase

`src/domains/impls/` définit **l'alphabet des valeurs abstraites** que tout le
reste de l'analyseur manipule : un produit `StateValue` de huit
« emplacements » (un par *sorte* de valeur JS : nombre, booléen, chaîne,
référence, `null`, `undefined`, setter, résidu), chacun muni de son propre
treillis (intervalles, treillis plats, ensembles bornés de chaînes, treillis de
**stabilité référentielle versionnée**), avec `join`/`meet`/`widen`/`widen_to`
et les opérations de *narrowing* sur gardes. `src/domains/mod.rs` fixe les deux
traits d'interface (`AbstractDomain`, `Transfer`) et `src/domains/context.rs`
le paquet d'état mutable (`AnalysisCtx`) et les contextes de requête
(`QueryContext`, `NullCtx`, `FixpointCtx`, `InterCtx`).

### 1.2 Place dans la chaîne parse → lowering → IR → engine → rules → driver

```
parse (oxc) ─► lowering (src/lowering/) ─► IR (ComponentIR, CFG, Expr, HookEntry)
                                              │
driver::run_check ─► resolver::analyze_lowered ─► engine::analyze_program   src/engine/fixpoint.rs:704-709
                                              │
                   analyze_component_impl(…)  src/engine/fixpoint.rs:130   (boucle de point fixe)
                       ├─ amorçage des slots : transfer.eval_expr(init, …)       fixpoint.rs:314-353
                       ├─ analyze_cfg::<T: Transfer>(…)                           cfg_analyzer.rs:34-124
                       │     ├─ transfer.exec_stmt / exec_expr_effects           (par bloc)
                       │     ├─ AbstractEnv::join / widen_to (arcs retour)       cfg_analyzer.rs:97-112
                       │     └─ narrow_env_for_branch (gardes)                    cfg_analyzer.rs:201-294
                       ├─ transfer.recompute_memo(…)  → MemoStore
                       ├─ StateStore::leq / changed_labels / widen_to            fixpoint.rs:495-530
                       └─ résultat : AnalysisResult<StateValue>
                                              │
rules (src/rules/) ◄── lisent StateValue : to_stability, is_unstable_reference_only,
                       is_unbounded, is_finitely_valued, as_setter, is_stable, …
```

Le **seul** `Transfer` concret est `StateValueTransfer`
(`src/domains/transfer/state_value.rs:24-102`), de domaine `StateValue`. Le
moteur est générique en `T: Transfer<Domain = StateValue>`
(`src/engine/fixpoint.rs:90`, `:130`) ; `analyze_cfg` est générique en
`T: Transfer` (`src/engine/cfg_analyzer.rs:34`).

Points d'entrée exacts vus depuis ce sous-système :

| Qui appelle | Quoi | Où |
|---|---|---|
| `engine::analyze_component` (API publique, tests) | `analyze_component_as(comp, ComponentId::SYNTHETIC, transfer, config)` | `src/engine/fixpoint.rs:90-96` |
| `engine::analyze_program` (CLI) | construit un `InterCtx` par racine, `analyze_child: analyze_component_inter as AnalyzeChildFn` | `src/engine/fixpoint.rs:729-741` |
| `eval_comp_app` (transfer) | `inter.is_recursive(child)` puis `inter.child(child)` et `(inter.analyze_child)(…)` | `src/domains/transfer/state_value.rs:521`, `:578` |
| `analyze_cfg` | construit un `AnalysisCtx { component, state, memo, heap, query, inter }` par bloc | `src/engine/cfg_analyzer.rs:69-76` |
| fixpoint | construit des `FixpointCtx { state, memo, callbacks }` | `src/engine/fixpoint.rs:361`, `:424`, `:461`, `:543`, `:570` |
| interpréteur | `ctx.query.callback_body(label)` | `src/domains/interp/interpreter.rs:579` |

### 1.3 Ce qui entre, ce qui sort

- **Entre** : des expressions de l'IR (`crate::ir::expr::{Expr, Prim, BinOp,
  UnaryOp}`), des identités (`ComponentId`, `HookLabel`, `QualifiedSlot`).
  `StateValue::from_init` lit une `Expr` (`state_value.rs:289-306`).
- **Sort** : des valeurs `StateValue` rangées dans `StateStore<StateValue>`
  (valeurs *écrites* des slots `useState`), `MemoStore<StateValue>` (valeurs
  des `useMemo`/`useCallback`), `AbstractEnv<StateValue>` (variables locales),
  et des **projections** consommées par les règles : `Stability`
  (`to_stability`), des prédicats booléens (`is_unbounded`,
  `is_unstable_reference_only`, `is_finitely_valued`, `is_stable`), des
  identités (`as_setter`), une chaîne `typeof` (`typeof_name`).

### 1.4 Dépendances

- `domains::impls` dépend de `crate::ir` (types `Expr`, `Prim`,
  `ComponentId`, `HookLabel`, `QualifiedSlot`) et de `crate::domains::AbstractDomain`.
- `domains::context` dépend de `crate::engine` (`AnalysisResult`,
  `ComponentCache`, `ComponentCallGraph`, `ComponentRegistry`, `HookRegistry`,
  `fixpoint::Config`) : c'est la seule flèche `domains → engine`. La
  dépendance inverse `transfer → fixpoint` est cassée par un **pointeur de
  fonction** (`AnalyzeChildFn`, §3.8).

---

## 2. Inventaire des fichiers du périmètre

| Fichier | Lignes | Rôle | Types/items publics | Fonctions d'entrée | Dépendances internes | Tests |
|---|---|---|---|---|---|---|
| `src/domains/mod.rs` | 162 | Déclare les sous-modules, ré-exporte, définit les deux traits | `trait AbstractDomain`, `trait Transfer` ; ré-exports `AnalysisCtx, AnalyzeChildFn, FixpointCtx, InterCtx, NullCtx, QueryContext, BoolVal, Interval, Stability, StateValue, AbstractEnv, EnvVal, Heap, HeapValue, MemoStore, StateStore, StateValueTransfer` (`mod.rs:7-10`) | méthodes des traits | `ir::{Expr, Stmt}`, `context`, `impls`, `stores`, `transfer` | 0 |
| `src/domains/context.rs` | 171 | Contextes passés aux fonctions de transfert | `type AnalyzeChildFn`, `trait QueryContext`, `struct NullCtx`, `struct InterCtx<'a>`, `struct AnalysisCtx<'a, D>`, `struct FixpointCtx<'a>` | `InterCtx::child`, `InterCtx::is_recursive`, `AnalysisCtx::null`, `FixpointCtx::callback_body` | `domains::stores`, `engine::{AnalysisResult, …, fixpoint::Config}`, `ir::{ComponentId, ComponentIR}` | 0 |
| `src/domains/impls/mod.rs` | 65 | Macro `flat_lattice!`, déclaration et ré-export des domaines | `macro_rules! flat_lattice` (non exportée), `pub use SetterVal, Stability, BoolVal, Interval, StateValue, StrConst` | — | `domains::AbstractDomain` (via `$crate`) | 0 |
| `src/domains/impls/bool_val.rs` | 13 | Treillis plat des booléens | `enum BoolVal { Bottom, True, False, Top }` | via `flat_lattice!` | macro | 0 (testé via `state_value.rs`) |
| `src/domains/impls/setter_val.rs` | 72 | Treillis plat des setters de `useState` avec identité | `enum SetterVal { Bottom, One(ComponentId, HookLabel), Top }` | `as_one` | `ir::{ComponentId, HookLabel}`, macro | 4 |
| `src/domains/impls/str_const.rs` | 153 | Ensemble fini borné de constantes de chaînes | `enum StrConst { Bottom, Set(Arc<BTreeSet<String>>), Top }`, `const STR_WIDEN_THRESHOLD = 4` (privée) | `singleton`, `from_set` (`pub(crate)`) | `AbstractDomain` | 5 |
| `src/domains/impls/interval.rs` | 614 | Intervalles `f64` avec bit d'intégralité | `struct Interval { lo, hi, is_int }` | `point, top, bottom, is_bottom, is_point, is_top, hull, widen, widen_to, add, sub, mul, neg, rem, pow, narrow_{lt,leq,gt,geq,eq,neq}` | `AbstractDomain` | 20 |
| `src/domains/impls/stability.rs` | 367 | Treillis de stabilité référentielle versionnée (ADR-017) | `enum Stability { Bottom, Stable, Versioned(BTreeSet<QualifiedSlot>), VersionedTop, PerRender, Unknown }`, `pub const VERSIONED_LABELS_THRESHOLD = 4` | `versioned`, `versioned_by`, `join`, `meet`, `widen`, `is_bottom` | `ir::{ComponentId, QualifiedSlot, HookLabel}` | 20 |
| `src/domains/impls/state_value.rs` | 1262 | Produit ponctuel des sortes JS (ADR-015) | `struct StateValue { num, boolean, str, reference, null, undef, setter, other }` ; `KindMask` (privé) | constructeurs `number, boolean, str_set, str_singleton, str_top, reference, null, undefined, component_setter` ; prédicats `is_bottom_value, is_top_value, typeof_name, as_setter, is_unstable_reference_only, from_init, to_stability, is_unbounded, is_finitely_valued, versioned_reference, is_stable` | tous les sous-domaines, `ir::expr::{Expr, Prim}` | 54 |

Total du périmètre : 2 879 lignes (`wc -l`). Les 103 tests unitaires se
répartissent : `interval.rs` 20, `stability.rs` 20, `state_value.rs` 54,
`str_const.rs` 5, `setter_val.rs` 4 (relevé `grep -c "#\[test\]"`, et
`cargo test -q --lib domains::impls` → `103 passed`, rejoué le 2026-09-28).

**Graphe des ré-exports** (utile pour lire les `use` du reste du code) :
`src/domains/mod.rs:1-5` déclare `pub mod context; impls; interp; stores;
transfer;`, puis ré-exporte (`:7-10`) :

```rust
pub use context::{AnalysisCtx, AnalyzeChildFn, FixpointCtx, InterCtx, NullCtx, QueryContext};
pub use impls::{BoolVal, Interval, Stability, StateValue};
pub use stores::{AbstractEnv, EnvVal, Heap, HeapValue, MemoStore, StateStore};
pub use transfer::StateValueTransfer;
```
(`src/domains/mod.rs:7-10`)

`SetterVal` et `StrConst` ne sont **pas** ré-exportés au niveau
`crate::domains` : on les nomme `crate::domains::impls::{SetterVal,
StrConst}`. Détail curieux : `impls/mod.rs:64` ré-exporte `BoolVal` et
`Interval` **via** `state_value` (`pub use state_value::{BoolVal, Interval,
StateValue};`), parce que `state_value.rs:9-12` les ré-exporte lui-même
(`pub use super::bool_val::BoolVal;` etc.) ; `impls/mod.rs:62-65`
ré-exporte `SetterVal`, `Stability`, `StrConst` depuis leurs propres modules.
Les deux chemins désignent les mêmes types.

---

## 3. Types et structures centraux

### 3.1 Le trait `AbstractDomain`

```rust
/// Core abstract domain trait.
///
/// Supertrait bounds:
/// - `Clone + Copy`  values are small, freely copyable
/// - `PartialEq`     needed for convergence checks
/// - `PartialOrd`    lattice order (a ≤ b = a ⊑ b)
/// - `Debug`         required for diagnostics and derive macros on generic containers
pub trait AbstractDomain: Clone + PartialEq + PartialOrd + std::fmt::Debug {
    fn bottom() -> Self;
    fn top() -> Self;
    fn is_bottom(&self) -> bool;
    fn join(&self, other: &Self) -> Self;
    fn meet(&self, other: &Self) -> Self;
    fn widen(&self, other: &Self) -> Self;

    /// Widening "up to" a finite set of thresholds (ASTRÉE-style).
    ///
    /// A growing bound jumps to the tightest enclosing threshold rather than
    /// straight to ⊤/±∞, recovering precision on guarded growth. Default falls
    /// back to plain `widen` (sound, threshold-unaware). Numeric domains override.
    fn widen_to(&self, other: &Self, _thresholds: &[f64]) -> Self {
        self.widen(other)
    }
```
(`src/domains/mod.rs:16-38`)

Remarques :

- Le commentaire mentionne `Clone + Copy` mais la borne réelle est
  `Clone + PartialEq + PartialOrd + Debug` : **pas de `Copy`** (depuis
  ADR-008, `StateValue` n'est plus `Copy` ; depuis ADR-017, `Stability` non
  plus). Commentaire obsolète — à signaler au lecteur.
- L'ordre du treillis est **encodé par `PartialOrd`** : `a <= b` signifie
  `a ⊑ b`, et `partial_cmp == None` signifie « incomparables ». Les stores s'en
  servent via `leq_pointwise` (`src/domains/stores/mod.rs:19-25`), qui ne
  retient que `Less | Equal`.
- `widen_to` (seuils, ADR-014) a une implémentation par défaut qui ignore les
  seuils ; seuls `Interval` et `StateValue` la redéfinissent.

La suite du trait porte deux ponts vers `StateValue` et les **opérateurs de
narrowing** (tous l'identité par défaut, ce qui est *sound* : ne rien raffiner
est toujours une sur-approximation) :

```rust
    /// Try to recover the underlying `StateValue`, if this domain IS `StateValue`.
    /// Default returns `None` (other domains). Used when the heap needs to store
    /// a captured environment with `StateValue` type (e.g. closure capture at FnLit creation).
    fn as_state_value(&self) -> Option<StateValue> {
        None
    }

    /// Convert a `StateValue` back to this domain.
    /// Default returns `Self::bottom()` (other domains drop the value conservatively).
    /// `StateValue` overrides to return itself. Used when restoring captured closure env.
    fn from_state_value(sv: StateValue) -> Self {
        let _ = sv;
        Self::bottom()
    }

    // Branch narrowing: default = identity (sound, imprecise).
    // Override for numeric domains to refine interval bounds on branch conditions.

    // Nullability narrowing (ADR-015). The IR conflates `==`/`===` into `Eq`,
    // so the refinements below are the sound envelope of both semantics.
    /// Taken `x !== null` (or false `x === null`): null impossible.
    fn narrow_drop_null(self) -> Self {
        self
    }
    /// Taken `x !== undefined` (or false `x === undefined`): undefined impossible.
    fn narrow_drop_undef(self) -> Self {
        self
    }
    /// Taken `x == null` / `x == undefined`: only null/undefined survive.
    fn narrow_keep_nullish_only(self) -> Self {
        self
    }
    /// Taken truthiness guard `if (x)`: excludes every falsy JS value
    /// (null, undefined, 0, "", false).
    fn narrow_truthy(self) -> Self {
        self
    }
    /// Falsy branch (`else` of `if (x)`, taken `if (!x)`): only falsy values
    /// survive (null, undefined, 0, "", false).
    fn narrow_falsy(self) -> Self {
        self
    }

    fn narrow_lt(self, _v: f64) -> Self {
        self
    }
```
(`src/domains/mod.rs:40-85` ; suivent `narrow_leq`, `narrow_gt`, `narrow_geq`,
`narrow_eq`, `narrow_neq`, même forme, `:86-100`)

Qui redéfinit quoi (relevé des `impl AbstractDomain for …`) :

| Domaine | `widen_to` | `as_state_value` / `from_state_value` | `narrow_{lt,leq,gt,geq,eq,neq}` | `narrow_drop_null/undef`, `keep_nullish_only`, `truthy`, `falsy` |
|---|---|---|---|---|
| `BoolVal`, `SetterVal` (macro) | défaut (`widen` = `join`) | défaut | défaut (identité) | défaut (identité) |
| `StrConst` | défaut | défaut | défaut | défaut |
| `Stability` | défaut | défaut | défaut | défaut |
| `Interval` | **oui** (`interval.rs:359-361`) | défaut | **oui**, délégation aux inhérentes (`:364-381`) | défaut |
| `StateValue` | **oui** (`state_value.rs:670-676`) | **oui** (`:529-535`) | **oui**, sur `num` seul (`:604-629`) | **oui** (`:539-602`) |

Seul `StateValue` redéfinit donc les raffinements de nullité et de véracité
— c'est cohérent : ce sont des propriétés de *sorte*, que seul le produit
connaît. Le doc du trait `Transfer` (`mod.rs:104-113`) conseille encore
« Pass `&NullCtx` when no cross-domain queries are needed » : daté, les
méthodes prennent aujourd'hui un `&mut AnalysisCtx` (dont `AnalysisCtx::null`
fournit la variante sans requêtes, §3.8).

**Piège sur `from_state_value`** : la valeur par défaut est `bottom()`, ce qui
serait *unsound* si un domaine autre que `StateValue` servait de carrier
d'analyse et restaurait un environnement capturé (⊥ = « inatteignable »).
C'est sans conséquence aujourd'hui : l'unique `Transfer` concret a pour
domaine `StateValue`, qui redéfinit les deux ponts (`state_value.rs:529-535`).

**Note** : le mot « narrowing » désigne ici le **raffinement sur garde**
(guard refinement), pas l'opérateur de rétrécissement de Cousot (phase
descendante), qu'ADR-014 a choisi de *ne pas* implémenter (§5.3).

`meet` n'est appelé **nulle part hors des tests** (recherche
`grep -rn "\.meet(\|::meet(" src` : les seuls appels sont la délégation
ponctuelle de `StateValue::meet` vers les emplacements, la délégation de
`impl AbstractDomain for Stability` vers la méthode inhérente, et les tests).
Le raffinement passe par les `narrow_*`, pas par `meet`.

### 3.2 Le trait `Transfer`

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

    /// Compute the abstract value for a memoized hook from its dependency list.
    /// Called by the engine after each render-pass to refresh the memo store.
    ///
    /// `ctx` carries the real analysis stores so a dep can be evaluated through
    /// the normal path (`MemoVal`/heap reads resolve against the current
    /// fixpoint state instead of a fabricated empty store).
    ///
    /// A deps argument the engine could not read bounds nothing: it must not
    /// share an answer with a written `[]`, which pins the memo forever.
    fn recompute_memo(
        &self,
        component: crate::ir::ComponentId,
        deps: &crate::ir::hooks::DepsArg,
        env: &AbstractEnv<Self::Domain>,
        ctx: &mut context::AnalysisCtx<Self::Domain>,
    ) -> Self::Domain;
}
```
(`src/domains/mod.rs:114-162`)

`env` reste un paramètre séparé parce que sa mutabilité diffère entre
`eval_expr` (`&`) et `exec_stmt` (`&mut`) (commentaire de `AnalysisCtx`,
`context.rs:107-111`). L'implémentation unique :

```rust
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
```
(`src/domains/transfer/state_value.rs:26-45`)

### 3.3 La macro `flat_lattice!`, `BoolVal` et `SetterVal`

```rust
/// Generate the flat-lattice `PartialOrd` + `AbstractDomain` impls for an enum
/// with a distinguished `$bottom` (⊥) and `$top` (⊤) *unit* variant. Every
/// other variant is an incomparable "mid" element: `a ⊔ b = a` when `a == b`,
/// else `⊤` (dually for meet); `⊥ < mid < ⊤`, two distinct mids incomparable.
/// Widen = join (a flat lattice has finite height). Requires `PartialEq + Clone`.
///
/// Shared by [`bool_val::BoolVal`] and [`setter_val::SetterVal`] — the two flat
/// lattices in the product domain, whose only difference is the payload of
/// their mid elements.
macro_rules! flat_lattice {
    ($ty:ident, bottom = $bottom:ident, top = $top:ident) => {
        impl ::std::cmp::PartialOrd for $ty {
            fn partial_cmp(&self, other: &Self) -> ::core::option::Option<::std::cmp::Ordering> {
                use ::std::cmp::Ordering;
                match (self, other) {
                    (a, b) if a == b => Some(Ordering::Equal),
                    ($ty::$bottom, _) | (_, $ty::$top) => Some(Ordering::Less),
                    ($ty::$top, _) | (_, $ty::$bottom) => Some(Ordering::Greater),
                    _ => None,
                }
            }
        }

        impl $crate::domains::AbstractDomain for $ty {
            fn bottom() -> Self {
                $ty::$bottom
            }
            fn top() -> Self {
                $ty::$top
            }
            fn is_bottom(&self) -> bool {
                matches!(self, $ty::$bottom)
            }
            fn join(&self, other: &Self) -> Self {
                match (self, other) {
                    (a, b) if a == b => a.clone(),
                    ($ty::$bottom, x) | (x, $ty::$bottom) => x.clone(),
                    _ => $ty::$top,
                }
            }
            fn meet(&self, other: &Self) -> Self {
                match (self, other) {
                    (a, b) if a == b => a.clone(),
                    ($ty::$top, x) | (x, $ty::$top) => x.clone(),
                    _ => $ty::$bottom,
                }
            }
            fn widen(&self, other: &Self) -> Self {
                self.join(other)
            }
        }
    };
}
```
(`src/domains/impls/mod.rs:1-53`)

Subtilité Rust : la macro est définie **avant** les `pub mod bool_val;` et
`pub mod setter_val;` (`impls/mod.rs:55-60`) ; c'est la portée *textuelle* des
`macro_rules!` qui la rend visible dans ces sous-modules sans `#[macro_export]`.
Introduite par le commit `61d7a0b` (2026-07-22, dette D5 d'ADR-020).

Ordre des bras dans `partial_cmp` : le bras `(⊥, _) | (_, ⊤) → Less` est testé
avant `(⊤, _) | (_, ⊥) → Greater` ; le cas `(⊥, ⊤)` tombe donc bien sur
`Less`, et l'égalité est traitée en premier.

**`BoolVal`** :

```rust
/// Flat lattice for the boolean slot of `StateValue`: `⊥ < {True, False} < ⊤`,
/// with `True`/`False` incomparable. Lattice ops via the `flat_lattice!` macro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoolVal {
    /// ⊥ unreachable.
    Bottom,
    True,
    False,
    /// ⊤ may be either.
    Top,
}

flat_lattice!(BoolVal, bottom = Bottom, top = Top);
```
(`src/domains/impls/bool_val.rs:1-13`)

```
        Top  {true,false}
       /    \
    True    False
       \    /
       Bottom  ∅
```

γ(⊥) = ∅, γ(True) = {true}, γ(False) = {false}, γ(⊤) = {true, false}. Ici
le treillis plat est **exactement** le treillis des parties de {true,false} :
la concrétisation est une bijection, `join`/`meet` sont exacts.

**`SetterVal`** :

```rust
/// Flat lattice for the component-setter slot of `StateValue`.
///
/// ```text
///            Top   (some setter, identity unknown)
///          /  |  \
///   One(a) One(b) One(c) ...
///          \  |  /
///           Bottom  (not a setter)
/// ```
///
/// React guarantees setter identity across renders, so any non-bottom
/// value maps to `Stability::Stable`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetterVal {
    /// ⊥ — no setter value possible.
    Bottom,
    /// Exactly this component's setter for this hook label.
    One(ComponentId, HookLabel),
    /// ⊤ — some setter, but which one was lost at a join.
    Top,
}

impl SetterVal {
    /// Payload accessor: `Some` only when the setter identity is exact.
    pub fn as_one(&self) -> Option<(&ComponentId, &HookLabel)> {
        match self {
            SetterVal::One(c, l) => Some((c, l)),
            _ => None,
        }
    }
}

// Flat lattice: `⊥ < One(..) < ⊤`, distinct `One(..)` incomparable. React
// guarantees setter identity across renders, so any non-⊥ value is `Stable`.
flat_lattice!(SetterVal, bottom = Bottom, top = Top);
```
(`src/domains/impls/setter_val.rs:3-37` ; la ligne 1 est
`use crate::ir::{ComponentId, types::HookLabel};`)

γ(One(c, l)) = { la fonction `setState` du slot `l` du composant `c` } ;
γ(⊤) = { un setter de `useState` quelconque }. Le treillis plat
sur-approxime : `One(a) ⊔ One(b) = ⊤` perd l'information « a ou b ».
Tests : `join_same_is_identity`, `join_different_is_top`,
`meet_different_is_bottom`, `partial_ord_flat` (`setter_val.rs:39-72`).
Historique : jusqu'au commit `806d114` (#7, 2026-09-05) le composant était
identifié par son nom (`Symbol`) ; ADR-015 montre encore
`One(Symbol, HookLabel)` — obsolète, c'est un `ComponentId` interné
(`src/ir/component_id.rs:29`).

### 3.4 `StrConst` — ensemble borné de constantes de chaînes

```rust
/// Max strings tracked before widening to Top.
const STR_WIDEN_THRESHOLD: usize = 4;

/// Abstract string-constant domain: a finite powerset lattice with widening.
///
/// Bottom ≤ Set(s) ≤ Top. Set(a) ≤ Set(b) iff a ⊆ b.
/// join(Set(a), Set(b)) widens to Top when |a ∪ b| > STR_WIDEN_THRESHOLD.
#[derive(Debug, Clone, PartialEq)]
pub enum StrConst {
    /// ⊥ no possible string value (unreachable path).
    Bottom,
    /// Finite known set of string constants.
    Set(Arc<BTreeSet<String>>),
    /// ⊤ any string (widened beyond threshold).
    Top,
}
```
(`src/domains/impls/str_const.rs:7-22`)

Le constructeur singleton (public) et le constructeur canonique
(`pub(crate)`) :

```rust
impl StrConst {
    pub fn singleton(s: String) -> Self {
        let mut set = BTreeSet::new();
        set.insert(s);
        StrConst::Set(Arc::new(set))
    }
```
(`src/domains/impls/str_const.rs:24-29`)

```rust
    pub(crate) fn from_set(set: BTreeSet<String>) -> Self {
        if set.is_empty() {
            StrConst::Bottom
        } else if set.len() > STR_WIDEN_THRESHOLD {
            StrConst::Top
        } else {
            StrConst::Set(Arc::new(set))
        }
    }
```
(`src/domains/impls/str_const.rs:31-39`)

**Invariant (implicite)** : un `Set(s)` construit par `from_set` vérifie
`1 ≤ |s| ≤ 4`. `singleton` le respecte. Le variant étant public, un
`StrConst::Set(Arc::new(∅))` reste constructible à la main (les tests le font
avec des ensembles non vides) ; aucune assertion ne l'interdit.

Ordre, join, meet :

```rust
impl PartialOrd for StrConst {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (a, b) if a == b => Some(Ordering::Equal),
            (StrConst::Bottom, _) => Some(Ordering::Less),
            (_, StrConst::Bottom) => Some(Ordering::Greater),
            (_, StrConst::Top) => Some(Ordering::Less),
            (StrConst::Top, _) => Some(Ordering::Greater),
            (StrConst::Set(a), StrConst::Set(b)) => {
                let a_sub_b = a.is_subset(b);
                let b_sub_a = b.is_subset(a);
                match (a_sub_b, b_sub_a) {
                    (true, true) => Some(Ordering::Equal),
                    (true, false) => Some(Ordering::Less),
                    (false, true) => Some(Ordering::Greater),
                    (false, false) => None,
                }
            }
        }
    }
}
```
(`src/domains/impls/str_const.rs:42-62`)

```rust
    fn join(&self, other: &Self) -> Self {
        match (self, other) {
            (a, b) if a == b => a.clone(),
            (StrConst::Bottom, x) | (x, StrConst::Bottom) => x.clone(),
            (StrConst::Top, _) | (_, StrConst::Top) => StrConst::Top,
            (StrConst::Set(a), StrConst::Set(b)) => {
                let union: BTreeSet<String> = a.iter().cloned().chain(b.iter().cloned()).collect();
                StrConst::from_set(union)
            }
        }
    }

    fn meet(&self, other: &Self) -> Self {
        match (self, other) {
            (a, b) if a == b => a.clone(),
            (StrConst::Bottom, _) | (_, StrConst::Bottom) => StrConst::Bottom,
            (StrConst::Top, x) | (x, StrConst::Top) => x.clone(),
            (StrConst::Set(a), StrConst::Set(b)) => {
                let inter: BTreeSet<String> = a.intersection(b).cloned().collect();
                StrConst::from_set(inter)
            }
        }
    }

    fn widen(&self, other: &Self) -> Self {
        // join already applies the threshold widening = join for this domain
        self.join(other)
    }
```
(`src/domains/impls/str_const.rs:75-102`)

Treillis (k-ensembles, k = 4) :

```
                      Top  (toutes les chaînes)
      ┌──────────┬───────┴──────┬──────────┐
  {a,b,c,d}  {a,b,c,e}  …  (ensembles de taille 4)
      │   ╲  ╱   │
    {a,b,c}   {a,b,d}  …   (taille 3)
      …                     (tailles 2, 1)
     {a}   {b}   {c}  …
        ╲   │   ╱
          Bottom  ∅
```

γ(⊥) = ∅ ; γ(Set S) = S ; γ(⊤) = l'ensemble de toutes les chaînes. `join`
est la borne supérieure *dans ce treillis* (l'union si elle a au plus 4
éléments, sinon ⊤ — l'union n'étant pas représentable, ⊤ est la plus petite
borne supérieure disponible) ; `meet` est l'intersection (exacte). Hauteur
finie (plus longue chaîne : ⊥ < {a} < {a,b} < {a,b,c} < {a,b,c,d} < ⊤, six
éléments), donc `widen = join` suffit à la terminaison. Les trois méthodes
triviales de l'impl (`bottom`, `top`, `is_bottom` par `matches!`) sont
aux lignes `str_const.rs:64-73`. Tests (5) : `singleton_is_not_top_or_bottom`,
`join_two_singletons_gives_pair`, `join_beyond_threshold_widens_to_top`,
`meet_gives_intersection`, `partial_ord_subset` (`str_const.rs:104-153` ;
le module de tests commence ligne 106 et l'aide `set(&[…])` construit des
`Set` directement, sans `from_set`).

Remarque sur l'égalité : `StrConst` dérive `PartialEq`, et
`Arc<BTreeSet<String>>` compare le **contenu** (l'`impl PartialEq for Arc<T>`
délègue à `T`), pas les pointeurs : deux ensembles égaux mais alloués
séparément sont égaux, ce qui est requis par le test de convergence.

Note : ADR-020 non-changement n°5 refuse d'extraire un combinateur
`BoundedPowerset<T,N>` : seul `StrConst` est un pur k-ensemble ; `Stability`
est plus riche (§5.6).

### 3.5 `Interval` — intervalles de `f64` avec bit d'intégralité

```rust
/// Closed interval [lo, hi] over f64. `lo > hi` = bottom (empty).
///
/// `is_int` records whether every concrete value the interval denotes is an
/// integer (`true` = proven all-integer; `false` = may contain non-integers).
/// It is a *precision* annotation only: it lets `narrow_lt`/`narrow_gt` apply
/// the integer tightening `x < 5 ⟹ x ≤ 4` (ADR-014 threshold widening relies on
/// this to bound counting loops) while staying sound over reals when the flag is
/// clear — a float state `x = 1.7` under `x < 2` must keep 1.7, not drop it.
/// Because it carries no set-membership information beyond `[lo, hi]`, it is
/// deliberately excluded from `PartialEq`/`PartialOrd`: two intervals with the
/// same bounds are equal (and the fixpoint converges) regardless of `is_int`.
#[derive(Debug, Clone, Copy)]
pub struct Interval {
    pub lo: f64,
    pub hi: f64,
    pub is_int: bool,
}

impl PartialEq for Interval {
    fn eq(&self, other: &Self) -> bool {
        self.lo == other.lo && self.hi == other.hi
    }
}
```
(`src/domains/impls/interval.rs:5-27`)

Rôle des champs :

- `lo`, `hi` : bornes fermées ; `±∞` représentent l'absence de borne.
- `is_int` : **annotation de précision**. `true` = « prouvé entier » ;
  `false` = « peut contenir des non-entiers ». Elle ne participe ni à
  l'égalité ni à l'ordre.

Concrétisation (en mots puis en formule) : un intervalle dénote les nombres JS
(non-`NaN`) compris entre ses bornes, restreints aux entiers si `is_int` :

```
γ([a,b], is_int)  = { x ∈ ℝ | a ≤ x ≤ b }  ∩  (ℤ si is_int, sinon ℝ)
γ(⊥)              = ∅            (toute paire lo > hi)
γ(top)            = ℝ            ([-∞, +∞], is_int = false)
```

`NaN` n'appartient **jamais** à γ d'un intervalle (dit explicitement par les
commentaires de `rem`/`pow` et de `narrow_truthy`) : toute opération pouvant
produire `NaN` renvoie ⊤ au niveau `StateValue` (§4.2). La question des
valeurs concrètes `±Infinity` (une borne `+∞` dit-elle « non borné » ou
« contient `Infinity` » ?) n'est tranchée nulle part : **à vérifier** (voir
§8, pièges 2 et 3).

Constructeurs et tests élémentaires :

```rust
impl Interval {
    pub fn point(v: f64) -> Self {
        Interval {
            lo: v,
            hi: v,
            is_int: v.is_finite() && v.fract() == 0.0,
        }
    }

    pub fn top() -> Self {
        Interval {
            lo: f64::NEG_INFINITY,
            hi: f64::INFINITY,
            is_int: false,
        }
    }

    /// Empty interval represents ⊥ for the numeric sub-lattice. `is_int` is
    /// vacuously true (the empty set contains no non-integer) and never read:
    /// every combinator short-circuits on `is_bottom` before touching it.
    pub fn bottom() -> Self {
        Interval {
            lo: f64::INFINITY,
            hi: f64::NEG_INFINITY,
            is_int: true,
        }
    }

    pub fn is_bottom(&self) -> bool {
        self.lo > self.hi
    }

    pub fn is_point(&self) -> bool {
        !self.is_bottom() && self.lo == self.hi
    }

    pub fn is_top(&self) -> bool {
        self.lo == f64::NEG_INFINITY && self.hi == f64::INFINITY
    }
```
(`src/domains/impls/interval.rs:29-67`)

`point(v)` n'est « entier » que si `v` est fini et sans partie fractionnaire :
`point(f64::INFINITY)` a `is_int = false`. Le join est l'enveloppe
convexe (*hull*) :

```rust
    /// Least upper bound: smallest interval containing both. Integer-valued only
    /// if both operands are (the union of two integer sets is integer-valued).
    pub fn hull(&self, other: &Self) -> Self {
        if self.is_bottom() {
            return *other;
        }
        if other.is_bottom() {
            return *self;
        }
        Interval {
            lo: self.lo.min(other.lo),
            hi: self.hi.max(other.hi),
            is_int: self.is_int && other.is_int,
        }
    }
```
(`src/domains/impls/interval.rs:69-83`)

`hull` n'est pas l'union ensembliste (`[0,0] ⊔ [10,10] = [0,10]` contient
1…9) : c'est la seule source d'imprécision du join numérique, inhérente au
domaine convexe. La négation échange les bornes et conserve `is_int` :

```rust
    pub fn neg(&self) -> Self {
        if self.is_bottom() {
            return Interval::bottom();
        }
        Interval {
            lo: -self.hi,
            hi: -self.lo,
            is_int: self.is_int,
        }
    }
```
(`src/domains/impls/interval.rs:189-198`)

Ordre (inclusion des bornes) :

```rust
/// `[a,b] ≤ [c,d]` iff `[a,b] ⊆ [c,d]` (i.e. c ≤ a && b ≤ d). Bounds only — `is_int`
/// is a precision annotation and does not participate (see the struct docs).
impl PartialOrd for Interval {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        if self == other {
            return Some(Ordering::Equal);
        }
        let self_in_other = other.lo <= self.lo && self.hi <= other.hi;
        let other_in_self = self.lo <= other.lo && other.hi <= self.hi;
        match (self_in_other, other_in_self) {
            (true, false) => Some(Ordering::Less),
            (false, true) => Some(Ordering::Greater),
            (true, true) => Some(Ordering::Equal),
            (false, false) => None,
        }
    }
}
```
(`src/domains/impls/interval.rs:312-328`)

**Piège (vérifié par la sonde, §6.0)** : ⊥ a plusieurs représentations. Le ⊥
canonique est `[+∞, −∞]`, mais un narrowing produit des ⊥ non canoniques :
`[10,20].narrow_lt(5) = [10, 4]`. Or `PartialEq`/`PartialOrd` comparent les
bornes brutes : `[10,4] == Interval::bottom()` est `false`, et
`partial_cmp([10,4], [0,3]) = None` (incomparables) alors que ⊥ devrait être
`Less` que tout. Les combinateurs (`hull`, `widen`, `widen_to`, `add`, …)
testent `is_bottom()` d'abord et restent corrects ; seules les comparaisons
brutes sont affectées. Effet observable sur la convergence : **à vérifier**
(`StateStore::update` fait `current.join(val)`, et `hull` renvoie l'autre
opérande quand l'un est ⊥, ce qui masque en pratique la plupart des ⊥ non
canoniques).

`impl AbstractDomain for Interval` :

```rust
impl AbstractDomain for Interval {
    fn bottom() -> Self {
        Interval::bottom()
    }
    fn top() -> Self {
        Interval::top()
    }
    fn is_bottom(&self) -> bool {
        Interval::is_bottom(self)
    }
    fn join(&self, other: &Self) -> Self {
        self.hull(other)
    }
    fn meet(&self, other: &Self) -> Self {
        // Intersection: integer-valued if *either* operand is (the meet is a
        // subset of each, so a subset of an all-integer set is all-integer).
        let lo = self.lo.max(other.lo);
        let hi = self.hi.min(other.hi);
        Interval {
            lo,
            hi,
            is_int: self.is_int || other.is_int,
        } // bottom if lo > hi
    }
    fn widen(&self, other: &Self) -> Self {
        Interval::widen(self, other)
    }
    fn widen_to(&self, other: &Self, thresholds: &[f64]) -> Self {
        Interval::widen_to(self, other, thresholds)
    }

    // Use fully-qualified inherent methods to avoid recursive trait dispatch.
    fn narrow_lt(self, v: f64) -> Self {
        Interval::narrow_lt(&self, v)
    }
```
(`src/domains/impls/interval.rs:332-366`)

**Piège Rust** (commentaire « fully-qualified ») : les méthodes du trait
prennent `self` par valeur, les méthodes inhérentes `&self`. Dans le corps du
trait, `self.narrow_lt(v)` trouverait la méthode du trait à l'étape « par
valeur » de la résolution de méthode, avant l'étape « auto-ref » où vit la
méthode inhérente : récursion infinie. D'où `Interval::narrow_lt(&self, v)`.

Treillis des intervalles (hauteur infinie) :

```
                    [-∞, +∞]
                   /        \
           [-∞, 5]            [0, +∞]
              …     [0, 5]      …
                  /   |   \
             [0,1] [2,3]  [4,5] …
               |      …
             [0,0]  [1,1]  … (points)
                  \   |   /
                     ⊥  (lo > hi)
```

Chaînes ascendantes infinies (`[0,0] ⊑ [0,1] ⊑ [0,2] ⊑ …`) : un *widening*
est indispensable (§4.3).

### 3.6 `Stability` — stabilité référentielle versionnée (ADR-017)

```rust
/// Threshold on `Versioned` label sets before widening to `VersionedTop`
/// (same pattern as `StrConst`).
pub const VERSIONED_LABELS_THRESHOLD: usize = 4;

/// Stability lattice: bounds on the *change trace* of a value — the set of
/// renders where `Object.is(vᵢ, vᵢ₋₁)` fails (the only thing React observes).
///
/// Two kinds of bounds coexist (ADR-017):
/// - **may** bound (over-approx): used by rules to *stay silent* soundly.
/// - **must** bound (under-approx): used by rules to *fire* without FPs.
///
/// ```text
///               Unknown  (⊤)
///              /         \
///      VersionedTop    PerRender
///           |              |
///    Versioned(S) ⊆-chains |
///           |              |
///        Stable            |
///              \          /
///               Bottom  (⊥)
/// ```
///
/// `Versioned`/`VersionedTop` and `PerRender` are incomparable — not
/// opposites, different bounds: `join = Unknown` (guarantees nothing in
/// either direction).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stability {
    /// ⊥ no information (unreachable path / uninitialized).
    Bottom,
    /// Never changes: the same reference on every render (safe as a dep).
    Stable,
    /// Changes *only* at setter events of these state slots (may bound).
    /// Invariant: non-empty (canonicalised — `Versioned(∅) ≡ Stable`) and
    /// `len() ≤ VERSIONED_LABELS_THRESHOLD` (widened to `VersionedTop` above).
    Versioned(BTreeSet<QualifiedSlot>),
    /// Versioned by unknown state slots (threshold-widened `Versioned`).
    VersionedTop,
    /// A fresh reference every render, guaranteed (must bound).
    /// For non-reference kinds via `to_stability`: "may change every render".
    PerRender,
    /// ⊤ no bound in either direction.
    Unknown,
}
```
(`src/domains/impls/stability.rs:9-52`)

`QualifiedSlot` est `(ComponentId, HookLabel)` (`src/ir/types.rs:6-9`) : un
slot d'état qualifié par le composant qui le possède, parce que `HookLabel`
(`usize`) n'est unique que par composant.

Constructeurs canonisants :

```rust
impl Stability {
    /// Canonicalising constructor: ∅ → `Stable`, over-threshold → `VersionedTop`.
    pub fn versioned(labels: BTreeSet<QualifiedSlot>) -> Self {
        if labels.is_empty() {
            Stability::Stable
        } else if labels.len() > VERSIONED_LABELS_THRESHOLD {
            Stability::VersionedTop
        } else {
            Stability::Versioned(labels)
        }
    }

    /// Single-slot `Versioned`.
    pub fn versioned_by(component: ComponentId, label: HookLabel) -> Self {
        Stability::Versioned(BTreeSet::from([(component, label)]))
    }
}
```
(`src/domains/impls/stability.rs:54-70`)

**Invariant** documenté (`stability.rs:42-43`) : `Versioned(S)` a
`1 ≤ |S| ≤ 4`. Garanti par `versioned` (utilisé par `join` et `meet`) et par
`versioned_by` (singleton). Le variant est public ; aucune assertion.

#### La concrétisation : des *traces de changement*

L'objet concret n'est pas une valeur mais, pour un emplacement observé au fil
des rendus r₀, r₁, …, sa **trace de changement** : l'ensemble
C = { i ≥ 1 | ¬Object.is(vᵢ, vᵢ₋₁) } (ADR-017 §« Semantic framework »). Deux
sortes de bornes sur C :

- borne **may** (sur-approximation) : « C ⊆ {rendus précédés d'un appel au
  setter d'un slot de S} » — un fait de sûreté, sert à *se taire* ;
- borne **must** (sous-approximation) : « C = tous les rendus » — un fait de
  certitude, sert à *tirer* (Error).

Lecture de chaque point (en notant Set(S) l'ensemble des rendus précédés d'un
appel de setter d'un slot de S) :

```
γ(Bottom)        = ∅                         (inatteignable)
γ(Stable)        = { traces avec C = ∅ }      (même référence à chaque rendu)
γ(Versioned(S))  = { traces avec C ⊆ Set(S) } (ne change qu'aux sets de S)
γ(VersionedTop)  = { traces avec C ⊆ Set(slots quelconques) }
γ(PerRender)     = { traces avec C = {1, 2, …} } (nouvelle référence à chaque rendu)
γ(Unknown)       = toutes les traces
```

L'ordre est l'inclusion des γ sur la chaîne may : `Stable ⊑ Versioned(S) ⊑
Versioned(T) (S ⊆ T) ⊑ VersionedTop`. `PerRender` n'est ni au-dessus ni
au-dessous : ce n'est pas l'« opposé » de `Stable` mais une borne d'une autre
nature.

Le tableau (may, must) d'ADR-017, avec l'ordre may `Never ⊑ OnSet ⊑ Every` et
must ordonné dualement :

| point (may, must) | lecture | exemple |
|---|---|---|
| (Never, Never) | `Stable` | `useRef`, setter, littéral |
| (OnSet, Never) | `Versioned` | lecture d'un état objet |
| (OnSet, OnSet) | change exactement aux sets | *(non retenu)* |
| (Every, OnSet) | change aux sets, peut-être plus | *(non retenu)* |
| (Every, Every) | `PerRender` | `ObjectLit` dans le corps de rendu |
| (Every, Never) | `Unknown` | `cond ? freshObj : stateObj` |

(ADR-017, tableau recopié.) Les deux points « must-change on set » sont
écartés : leur seul consommateur (détection de churn) a besoin d'une analyse
d'atteignabilité du corps d'effet et vit donc dans la règle.

#### Ordre, join, meet, widen

```rust
impl PartialOrd for Stability {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        use Stability::*;
        match (self, other) {
            (a, b) if a == b => Some(Ordering::Equal),
            (Bottom, _) => Some(Ordering::Less),
            (_, Bottom) => Some(Ordering::Greater),
            (_, Unknown) => Some(Ordering::Less),
            (Unknown, _) => Some(Ordering::Greater),
            // Stable ⊑ Versioned ⊑ VersionedTop (behaviour-set inclusion:
            // "never changes" ⊂ "changes only at sets").
            (Stable, Versioned(_) | VersionedTop) => Some(Ordering::Less),
            (Versioned(_) | VersionedTop, Stable) => Some(Ordering::Greater),
            (Versioned(s), Versioned(t)) => {
                if s.is_subset(t) {
                    Some(Ordering::Less) // s ≠ t here (equal case above)
                } else if t.is_subset(s) {
                    Some(Ordering::Greater)
                } else {
                    None
                }
            }
            (Versioned(_), VersionedTop) => Some(Ordering::Less),
            (VersionedTop, Versioned(_)) => Some(Ordering::Greater),
            // PerRender vs Stable/Versioned/VersionedTop: incomparable
            // (different bounds — may vs must).
            _ => None,
        }
    }
}
```
(`src/domains/impls/stability.rs:72-101`)

```rust
    /// Least upper bound (⊔).
    pub fn join(&self, other: &Self) -> Self {
        use Stability::*;
        match (self, other) {
            (a, b) if a == b => a.clone(),
            (Bottom, x) | (x, Bottom) => x.clone(),
            (Unknown, _) | (_, Unknown) => Unknown,
            (Stable, v @ (Versioned(_) | VersionedTop))
            | (v @ (Versioned(_) | VersionedTop), Stable) => v.clone(),
            (Versioned(s), Versioned(t)) => Stability::versioned(s.union(t).cloned().collect()),
            (VersionedTop, Versioned(_)) | (Versioned(_), VersionedTop) => VersionedTop,
            // {Stable, Versioned, VersionedTop} ⊔ PerRender = Unknown
            _ => Unknown,
        }
    }

    /// Greatest lower bound (⊓).
    pub fn meet(&self, other: &Self) -> Self {
        use Stability::*;
        match (self, other) {
            (a, b) if a == b => a.clone(),
            (Unknown, x) | (x, Unknown) => x.clone(),
            (Bottom, _) | (_, Bottom) => Bottom,
            (Stable, Versioned(_) | VersionedTop) | (Versioned(_) | VersionedTop, Stable) => Stable,
            (Versioned(s), Versioned(t)) => {
                let inter: BTreeSet<_> = s.intersection(t).cloned().collect();
                Stability::versioned(inter) // ∅ canonicalises to Stable
            }
            (VersionedTop, v @ Versioned(_)) | (v @ Versioned(_), VersionedTop) => v.clone(),
            // {Stable, Versioned, VersionedTop} ⊓ PerRender = Bottom
            _ => Bottom,
        }
    }

    /// Widening: join, whose `Versioned` union is already threshold-bounded —
    /// chains have height ≤ threshold + 4.
    pub fn widen(&self, other: &Self) -> Self {
        self.join(other)
    }
```
(`src/domains/impls/stability.rs:108-146`)

Remarques :

- `join`, `meet`, `widen` sont des méthodes **inhérentes** ; l'impl
  `AbstractDomain for Stability` (`stability.rs:149-168`) délègue. La
  résolution de méthode préfère l'inhérente (`&self`) : pas de récursion.
- `meet(Versioned(S), PerRender) = Bottom` est la borne inférieure *dans
  l'ordre abstrait*, mais les γ s'intersectent (une trace où chaque rendu suit
  un set de S appartient aux deux). `meet` n'est pas utilisé en production
  (§3.1), donc sans effet ; à ne pas utiliser pour raffiner.
- Hauteur : ⊥ < Stable < V(1) < V(2) < V(3) < V(4) < VersionedTop < Unknown
  (8 éléments) — `widen = join` termine.
- Tests (20, `stability.rs:170-367`) : lois de treillis vérifiées sur
  `all_points()` (7 points) : `join_idempotent_commutative`,
  `join_is_upper_bound`, `meet_is_lower_bound`, `bottom_least_unknown_greatest`,
  `widen_equals_join`, `perrender_incomparable_with_stable_and_versioned`,
  `versioned_empty_is_stable`, `meet_disjoint_versioned_is_stable`,
  `join_versioned_over_threshold_widens_to_top`.

Diagramme de Hasse complet (ASCII) :

```
                       Unknown (⊤)
                      /            \
              VersionedTop          \
                   |                 \
         Versioned({a,b,c,d}) …       \
                   |                   \
          Versioned({a,b})  Versioned({a,c}) …
               \      /          |       PerRender
            Versioned({a})  Versioned({b}) …    |
                     \       /                  |
                      Stable                    |
                          \                    /
                           ─────  Bottom (⊥) ──
```

`Stability` n'est plus `Copy` (possède un `BTreeSet`) : ADR-017 l'accepte
(« les erreurs de compilation forcent un audit exhaustif des consommateurs »).

### 3.7 `StateValue` — le produit ponctuel (ADR-015)

```rust
/// Abstract JS value: pointwise product over the disjoint JS kinds (ADR-015).
///
/// JS primitive kinds are mutually exclusive (a value is never a number AND a
/// string), so the disjunctive completion of the kind sum degenerates into a
/// product: one independent slot per kind, each ⊥ when that kind is impossible.
/// `join`/`meet`/`widen` are pointwise — a cross-kind join keeps BOTH kinds
/// (`number | null`, `number | string`) instead of collapsing to ⊤, which is
/// what enables infinite-loop detection through nullable states without a
/// TypeScript hint.
#[derive(Clone, PartialEq)]
pub struct StateValue {
    /// Numeric kind — interval [lo, hi]; ⊥ = cannot be a number.
    pub num: Interval,
    /// Boolean kind.
    pub boolean: BoolVal,
    /// String kind — finite constant set, threshold-widened to ⊤.
    pub str: StrConst,
    /// Object/array/function kind — reference stability; ⊥ = not a reference.
    pub reference: Stability,
    /// `null` possible?
    pub null: bool,
    /// `undefined` possible?
    pub undef: bool,
    /// Cross-component useState setter (flat lattice with identity payload).
    pub setter: SetterVal,
    /// Residual ⊤ — kinds not modelled (symbol, bigint, …). `true` means the
    /// value may be something outside every other slot.
    pub other: bool,
}
```
(`src/domains/impls/state_value.rs:16-44`)

#### Concrétisation

Les sortes JS étant disjointes, la concrétisation est l'**union disjointe**
des concrétisations par emplacement :

```
γ(v) =   γ_num(v.num)                       (nombres)
       ∪ γ_bool(v.boolean)                  (booléens)
       ∪ γ_str(v.str)                       (chaînes)
       ∪ { objets/tableaux/fonctions dont la trace de changement ∈ γ_stab(v.reference) }
       ∪ ({null}      si v.null)
       ∪ ({undefined} si v.undef)
       ∪ γ_setter(v.setter)                 (fonctions setState)
       ∪ (tout le reste — symbol, bigint, et implicitement NaN — si v.other)
```

Attention : l'emplacement `reference` mélange deux natures. Pour les autres
emplacements γ porte sur des *valeurs* ; pour `reference` il porte sur la
*stabilité* (trace de changement) des références possibles, sans rien dire de
leur contenu. Le domaine ne modélise pas le contenu des objets : c'est le
travail du tas (`src/domains/stores/heap.rs`, hors périmètre).

`NaN` : aucune sorte n'en parle explicitement ; toute opération susceptible de
le produire rend `StateValue::top()` (où `other = true`), donc `NaN` n'est
couvert que par ⊤ — interprétation déduite du code, **à vérifier** avec
l'auteur (§8).

#### ⊥ et ⊤

`⊥` = tous les emplacements ⊥ (`bottom_value`, `state_value.rs:190-206`,
`const fn` pour pouvoir servir en syntaxe de mise à jour de struct) ; `⊤` =
tous les emplacements ⊤, **y compris `other`** :

```rust
    fn top() -> Self {
        StateValue {
            num: Interval::top(),
            boolean: BoolVal::Top,
            str: StrConst::Top,
            reference: Stability::Unknown,
            null: true,
            undef: true,
            setter: SetterVal::Top,
            other: true,
        }
    }
```
(`src/domains/impls/state_value.rs:512-523`)

#### `KindMask` : l'unique énumération des sortes

```rust
/// Bitset of which kind slots of a [`StateValue`] are non-⊥.
///
/// The product's kind enumeration lives in exactly one place —
/// [`StateValue::populated_kinds`], whose destructuring has no `..`. Adding a
/// kind field is therefore a compile error there, not a silent omission that
/// would quietly break every "is this ⊥ / only a setter / how many kinds"
/// predicate downstream (a latent false negative). Each such predicate derives
/// from this mask instead of re-listing the eight slots.
#[derive(Clone, Copy, PartialEq, Eq)]
struct KindMask(u8);
```
(`src/domains/impls/state_value.rs:46-55`)

```rust
    fn populated_kinds(&self) -> KindMask {
        let StateValue {
            num,
            boolean,
            str,
            reference,
            null,
            undef,
            setter,
            other,
        } = self;
        let mut m = 0u8;
        if !num.is_bottom() {
            m |= KindMask::NUM;
        }
        if *boolean != BoolVal::Bottom {
            m |= KindMask::BOOL;
        }
```
(`src/domains/impls/state_value.rs:84-101` ; les six autres bits suivent,
`:102-120`, et `KindMask(m)` est rendu ligne 120)

Les huit bits et les trois requêtes du masque (toutes privées) :

```rust
impl KindMask {
    const NUM: u8 = 1 << 0;
    const BOOL: u8 = 1 << 1;
    const STR: u8 = 1 << 2;
    const REF: u8 = 1 << 3;
    const NULL: u8 = 1 << 4;
    const UNDEF: u8 = 1 << 5;
    const SETTER: u8 = 1 << 6;
    const OTHER: u8 = 1 << 7;

    fn is_empty(self) -> bool {
        self.0 == 0
    }
    /// Number of populated (non-⊥) kinds.
    fn count(self) -> u32 {
        self.0.count_ones()
    }
    /// Every populated kind is within `{bit}` (holds when nothing is populated).
    fn only(self, bit: u8) -> bool {
        self.0 & !bit == 0
    }
}
```
(`src/domains/impls/state_value.rs:57-78`)

Consommateurs : `is_bottom_value` (`is_empty`), `typeof_name` (motif « un
seul bit »), `as_setter` et `is_unstable_reference_only` (`only`),
`to_stability` (`count() >= 2`). Noter que `only(bit)` est **vrai sur ⊥**
(aucun bit peuplé) : chaque consommateur qui l'utilise doit donc écarter ⊥
lui-même s'il le faut (c'est le cas de `is_unstable_reference_only`, qui
exige en plus `reference == PerRender`).

C'est une **protection à la compilation contre un futur faux négatif**
(ADR-020, D4, commit `c887c52`) : la déstructuration sans `..` fait échouer la
compilation si l'on ajoute un neuvième emplacement. `is_top_value`
(`state_value.rs:215-238`) utilise la même astuce indépendamment (c'est un
test de saturation ⊤, pas de population).

**Attention** : les fonctions de transfert `as_arith`, `as_str_only`,
`eval_unary(Not)` (`src/domains/transfer/state_value.rs:714-750`, `:947-965`)
**ré-énumèrent** les emplacements à la main (sans `KindMask`) : un emplacement
ajouté ne les ferait pas échouer à la compilation. C'est une limite de la
protection D4 (constat de lecture).

#### Constructeurs par sorte

`number`, `boolean`, `str_set` (passe par `StrConst::from_set`, donc seuil),
`str_singleton`, `str_top`, `reference`, `null`, `undefined`,
`component_setter` (`state_value.rs:125-188`), tous de la forme :

```rust
    pub fn number(i: Interval) -> Self {
        StateValue {
            num: i,
            ..Self::bottom_value()
        }
    }
```
(`src/domains/impls/state_value.rs:125-130`)

Exceptions à cette forme : `str_set(set)` passe par `StrConst::from_set`
(un ensemble vide donne donc ⊥, plus de 4 éléments donnent `str: Top`),
`null()`/`undefined()` posent un booléen, `component_setter(c, l)` pose
`SetterVal::One(c, l)` (`:183-188`).

#### `from_init` : valeur abstraite d'un initialiseur syntaxique

```rust
    /// Derive the best initial abstract value from a `useState(init)` expression.
    pub fn from_init(init: &Expr) -> Self {
        match init {
            Expr::Lit(Prim::Int(n)) => StateValue::number(Interval::point(*n as f64)),
            Expr::Lit(Prim::Float(f)) => StateValue::number(Interval::point(*f)),
            Expr::Lit(Prim::Bool(b)) => {
                StateValue::boolean(if *b { BoolVal::True } else { BoolVal::False })
            }
            Expr::Lit(Prim::String(s)) => StateValue::str_singleton(s.to_string()),
            Expr::Lit(Prim::Null) => StateValue::null(),
            Expr::Lit(Prim::Unit) => StateValue::undefined(),
            Expr::ObjectLit { .. }
            | Expr::ArrayLit { .. }
            | Expr::FnLit { .. }
            | Expr::New { .. } => StateValue::reference(Stability::PerRender),
            _ => StateValue::top(),
        }
    }
```
(`src/domains/impls/state_value.rs:289-306` ; `Expr::New` ajouté par
`e67b10a`, #158)

**Attention, malgré son nom et sa doc**, `from_init` n'est **pas** la
fonction qui amorce les slots `useState` : l'amorçage passe par
`transfer.eval_expr(init, &init_env, &mut ac)` dans le moteur, avec un cas
spécial pour l'initialiseur paresseux `useState(() => expr)`, dont le corps
est exécuté (`interp::exec_body`) au lieu d'abstraire la fermeture
(`src/engine/fixpoint.rs:314-353`, commentaire « Abstracting the FnLit
(reference(Unstable)) made every lazy-init state slot an "unstable dep"
(corpus FP, TODO.md F2) »). Le seul appelant hors tests est
`src/engine/written.rs:117` (`Terminator::Return(e) =>
Some(StateValue::from_init(e.peel_ts()))`, dans `returns_value`,
`written.rs:108-121`), qui calcule « sans env » la valeur que stocke un
updater fonctionnel `setX(prev => …)` (join des `Return` du corps, ⊤ si
aucun) pour les preuves qui lisent l'écriture d'un autre site. Pour ce
usage purement syntaxique, un `FnLit` rendu ⇒ `PerRender` est correct (un
updater qui rend une fonction la stocke). Tests :
`from_init_int_gives_point_interval`, `from_init_null_gives_null`,
`from_init_object_gives_unstable_reference`,
`from_init_string_gives_singleton` (`state_value.rs:822-856`).

#### Affichage (`Debug`)

`Debug` est écrit à la main (`state_value.rs:422-475`) : `⊥`, `⊤`, sinon
l'union des sortes jointes par `|` : `number[1, 1]|null`, `string{"a", "b"}`,
`ref(Versioned({…}))`, `setter(#idx#label)`, `boolean`, `number` (intervalle
⊤). Test `debug_renders_kind_union` (`state_value.rs:1255-1261`). Les règles
ne doivent **jamais** l'imprimer dans un message (`describe_value`,
`src/rules/helpers/mod.rs:36-52`).

### 3.8 Les contextes (`context.rs`)

```rust
/// Function pointer for inlining a child component's analysis. Breaks the
/// circular dep between `domains::transfer` and `engine::fixpoint`.
pub type AnalyzeChildFn = fn(
    &ComponentIR,
    ComponentId,
    AbstractEnv<StateValue>,
    Heap,
    &InterCtx<'_>,
) -> AnalysisResult<StateValue>;

// ── QueryContext trait ────────────────────────────────────────────────────────

/// Cross-domain context passed to every Transfer method for abstract-state queries.
pub trait QueryContext {
    /// Body CFG of a `useCallback` hook, if the context knows it. Lets the
    /// interpreter execute calls through a callback-bound variable
    /// (`const cb = useCallback(...); ...; cb()`): the rewrite to
    /// `CallbackVal(label)` moved the body out of the expression tree, so it
    /// is not reachable through the heap like a plain FnLit.
    fn callback_body(
        &self,
        _label: crate::ir::types::HookLabel,
    ) -> Option<std::sync::Arc<crate::ir::cfg::CFG>> {
        None
    }
}

// ── NullCtx ───────────────────────────────────────────────────────────────────

/// No-op context: returns `Top` for every query. Used in tests and as recursion base.
pub struct NullCtx;

impl QueryContext for NullCtx {}
```
(`src/domains/context.rs:17-49`)

- `QueryContext` n'a plus qu'**une** requête (`callback_body`) ; ADR-007
  décrit `state_value_of(&self, expr)` et un troisième contexte
  `AnalysisQueryCtx` qui n'existent plus. Le doc de `NullCtx` (« returns
  `Top` ») est lui aussi daté : il rend `None`.
- La valeur par défaut `None` est *sound* : l'interpréteur ne peut alors pas
  exécuter le corps et retombe sur une valeur conservatrice (hors périmètre,
  **à vérifier** dans `interp/interpreter.rs:579`).

```rust
pub struct InterCtx<'a> {
    pub registry: &'a ComponentRegistry,
    pub cache: &'a RefCell<ComponentCache>,
    pub shared_state: &'a RefCell<SharedStateStore>,
    pub call_graph: &'a RefCell<ComponentCallGraph>,
    pub stats: &'a RefCell<AnalysisStats>,
    /// All analysis results accumulated across the entire program analysis.
    pub results: &'a RefCell<std::collections::HashMap<ComponentId, AnalysisResult<StateValue>>>,
    /// Components currently being analyzed (for recursion detection).
    pub call_stack: RefCell<Vec<ComponentId>>,
    /// The component being analyzed at this level.
    pub component: ComponentId,
    /// Analysis config (widen_threshold etc.).
    pub config: &'a Config,
    /// Callback provided by `engine::fixpoint` to inline a child component's analysis.
    pub analyze_child: AnalyzeChildFn,
    /// User-defined custom hook registry for inlining (None = no inlining).
    pub hook_registry: Option<&'a HookRegistry>,
}
```
(`src/domains/context.rs:59-77`)

Tout l'état partagé passe par `RefCell` pour qu'`InterCtx` circule en `&`
(pas de `&mut` imbriqués). `child` clone la pile d'appel et y empile le
composant courant ; `is_recursive` détecte la récursion de composants :

```rust
impl<'a> InterCtx<'a> {
    /// Create a child context for inlining a nested component.
    /// Shares all RefCell state; new call_stack with parent pushed.
    pub fn child(&self, child: ComponentId) -> InterCtx<'a> {
        let mut new_stack = self.call_stack.borrow().clone();
        new_stack.push(self.component);
        InterCtx {
            registry: self.registry,
            cache: self.cache,
            shared_state: self.shared_state,
            call_graph: self.call_graph,
            stats: self.stats,
            results: self.results,
            call_stack: RefCell::new(new_stack),
            component: child,
            config: self.config,
            analyze_child: self.analyze_child,
            hook_registry: self.hook_registry,
        }
    }

    pub fn is_recursive(&self, id: ComponentId) -> bool {
        self.call_stack.borrow().contains(&id) || self.component == id
    }
}
```
(`src/domains/context.rs:79-103`)

Invariant : la pile `call_stack` d'un contexte contient les **ancêtres**
du composant courant, pas le courant lui-même (il est poussé seulement
quand on descend, dans `child`) — d'où le second disjoint
`self.component == id` de `is_recursive`. Toutes les références `&'a`
(registres, caches, résultats, pointeur `analyze_child`) sont partagées par
copie ; seule la pile est neuve. Usage : `eval_comp_app` appelle
`inter.is_recursive(child)` (`transfer/state_value.rs:521`) avant
`inter.child(child)` (`:578`). Les contextes racines sont construits par
`analyze_program` avec `call_stack: RefCell::new(vec![])`
(`src/engine/fixpoint.rs:729-741`).

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

Le champ `component` est ce qui permet de fabriquer les étiquettes
`Versioned({(component, label)})` et `SetterVal::One(component, label)` à la
lecture d'un slot (§4.6). `AnalysisCtx::null(component, state, memo, heap)`
branche un `NullCtx` statique et `inter: None` :

```rust
impl<'a, D: AbstractDomain> AnalysisCtx<'a, D> {
    /// Construct with `NullCtx` as the query context (tests, simple impls).
    pub fn null(
        component: ComponentId,
        state: &'a mut StateStore<D>,
        memo: &'a mut MemoStore<D>,
        heap: &'a mut Heap,
    ) -> Self {
        static NULL: NullCtx = NullCtx;
        AnalysisCtx {
            component,
            state,
            memo,
            heap,
            query: &NULL,
            inter: None,
        }
    }
}
```
(`src/domains/context.rs:127-145`)

Le `static NULL` donne une référence `&'static dyn QueryContext` sans
allocation (`NullCtx` est une structure unitaire). Appelants dans le moteur :
évaluation des constantes de module (`src/engine/fixpoint.rs:168`), valeurs
de retour des arguments de hooks personnalisés (`:245`), amorçage des slots
`useState` (`:327-332`) — tous avec des stores « graines » vides.

```rust
pub struct FixpointCtx<'a> {
    pub state: &'a StateStore<StateValue>,
    pub memo: &'a MemoStore<StateValue>,
    /// `useCallback` body CFGs by hook label (see `QueryContext::callback_body`).
    pub callbacks: &'a std::collections::HashMap<
        crate::ir::types::HookLabel,
        std::sync::Arc<crate::ir::cfg::CFG>,
    >,
}
```
(`src/domains/context.rs:154-162`)

```rust
impl QueryContext for FixpointCtx<'_> {
    fn callback_body(
        &self,
        label: crate::ir::types::HookLabel,
    ) -> Option<std::sync::Arc<crate::ir::cfg::CFG>> {
        self.callbacks.get(&label).cloned()
    }
}
```
(`src/domains/context.rs:164-171`)

Seul `callbacks` est lu par `callback_body` (le clonage est celui d'un
`Arc`, O(1)) ; les champs
`state` et `memo` ne sont lus par aucune méthode du trait (la doc « Local
variables return `Bottom` » décrit un comportement disparu).

### 3.9 Catalogue des tests unitaires du périmètre

Relevé exhaustif des 103 `#[test]` (noms verbatim, `grep -A1 "#\[test\]"`),
groupés par ce qu'ils épinglent :

- **`setter_val.rs` (4, `:50-72`)** : `join_same_is_identity`,
  `join_different_is_top`, `meet_different_is_bottom`, `partial_ord_flat`.
- **`str_const.rs` (5, `:115-153`)** : `singleton_is_not_top_or_bottom`,
  `join_two_singletons_gives_pair`, `join_beyond_threshold_widens_to_top`,
  `meet_gives_intersection`, `partial_ord_subset`.
- **`interval.rs` (20, `:390-614`)** : bases (`interval_point_is_stable`,
  `interval_hull`, `interval_add`, `interval_partial_ord`) ; `%`/`**`
  (`rem_follows_the_dividend_sign_and_stays_below_the_divisor`,
  `rem_by_a_possible_zero_is_unrepresentable`,
  `pow_takes_the_corners_of_the_non_negative_box`,
  `pow_outside_the_non_negative_box_is_unrepresentable`) ; widening
  (`interval_widen_grows_hi`, `interval_widen_shrinks_lo`) ; widening à seuils
  (`widen_to_jumps_to_threshold_not_infinity`,
  `widen_to_goes_infinity_when_no_threshold_encloses`,
  `widen_to_picks_tightest_enclosing_threshold`,
  `widen_to_lower_bound_threshold`, `widen_to_empty_thresholds_equals_plain_widen`,
  `widen_to_stable_bound_untouched`, `widen_to_is_sound_superset_of_hull`) ;
  bit `is_int` (`narrow_lt_float_keeps_boundary_value`,
  `narrow_lt_integer_still_tightens`, `integrality_lost_through_float_arithmetic`).
- **`stability.rs` (20, `:199-367`)** : join (`join_stable_perrender_is_unknown`,
  `join_versioned_perrender_is_unknown`, `join_stable_versioned_keeps_versioned`,
  `join_versioned_is_label_union`, `join_versioned_over_threshold_widens_to_top`,
  `join_with_bottom_is_identity`, `join_with_unknown_is_unknown`,
  `join_idempotent_commutative`, `join_is_upper_bound`) ; canonisation et meet
  (`versioned_empty_is_stable`, `meet_disjoint_versioned_is_stable`,
  `meet_stable_perrender_is_bottom`, `meet_versioned_is_label_intersection`,
  `meet_with_unknown_is_identity`, `meet_is_lower_bound`) ; ordre
  (`bottom_least_unknown_greatest`, `stable_below_versioned_below_versioned_top`,
  `perrender_incomparable_with_stable_and_versioned`,
  `incomparable_versioned_sets`) ; `widen_equals_join`. Les tests de lois
  quantifient sur `all_points()` (7 points : ⊥, Stable, V({A0}), V({A0,B1}),
  VersionedTop, PerRender, Unknown — `stability.rs:185-195`).
- **`state_value.rs` (54, `:697-1261`)** : ordre (`bottom_is_least`,
  `top_is_greatest`, `mixed_direction_slots_are_incomparable`,
  `join_is_upper_bound`) ; produit inter-sortes (`number_join_is_hull`,
  `cross_kind_join_keeps_both_slots`, `null_number_join_keeps_interval`,
  `number_string_join_keeps_both`, `number_widen_grows_bound`) ;
  `to_stability` (`to_stability_point_is_stable`,
  `to_stability_wide_interval_is_unstable`, `to_stability_bottom_is_bottom`,
  `to_stability_mixed_stable_kinds_is_unknown`,
  `nullable_widened_number_is_unstable`) ; `is_unbounded_requires_active_num_slot` ;
  `from_init_*` (4) ; raffinements (`interval_narrow_lt_caps_hi`,
  `interval_narrow_geq_lifts_lo`, `interval_narrow_eq_in_range_gives_point`,
  `interval_narrow_eq_out_of_range_gives_bottom`,
  `state_value_narrow_lt_on_number`, `state_value_narrow_keeps_other_slots`,
  `nullability_narrowing_drop_null`, `nullability_narrowing_keep_nullish_only`,
  `nullability_narrowing_nullish`, `truthiness_narrowing_excludes_falsy_values`,
  `falsy_narrowing_keeps_only_falsy_values`) ; chaînes (`str_singleton_is_stable`,
  `str_multi_is_not_stable`, `str_join_same_singleton_idempotent`,
  `str_join_two_singletons_gives_pair`, `str_join_with_str_top_gives_str`,
  `str_join_beyond_threshold_widens_to_str`, `str_partial_ord_subset`,
  `str_meet_gives_intersection`) ; setters (`component_setter_is_stable`,
  `component_setter_to_stability_is_stable`,
  `component_setter_join_same_is_identity`,
  `component_setter_join_different_loses_identity_stays_stable`,
  `component_setter_join_with_ref_keeps_both_slots`,
  `component_setter_join_with_unstable_ref_is_unstable`,
  `component_setter_join_with_top_gives_top`,
  `component_setter_join_with_bottom_gives_self`,
  `component_setter_as_setter_extracts_payload`,
  `different_component_setters_incomparable`,
  `component_setter_meet_same_is_identity`,
  `component_setter_meet_different_is_bottom`,
  `component_setter_widen_same_is_identity`,
  `component_setter_as_state_value_roundtrip`) ;
  `unstable_reference_only_detection` ; `debug_renders_kind_union`.

Deux tests qui fixent la sémantique *motion-wins* et l'union de sortes, à
citer tels quels dans le chapitre :

```rust
    #[test]
    fn component_setter_join_with_ref_keeps_both_slots() {
        let j = cs("Foo", 0).join(&StateValue::reference(Stability::Stable));
        assert_eq!(j.setter, SetterVal::One(crate::test_support::cid(0), 0));
        assert_eq!(j.reference, Stability::Stable);
        // Mixed with a reference → no longer an exact setter.
        assert!(j.as_setter().is_none());
        // Two populated kinds = two distinct possible values → not
        // definitely stable (cross-kind union rule).
        assert_eq!(j.to_stability(), Stability::Unknown);
    }

    #[test]
    fn component_setter_join_with_unstable_ref_is_unstable() {
        // Motion-wins: the unstable reference slot dominates the stable setter.
        let j = cs("Foo", 0).join(&StateValue::reference(Stability::PerRender));
        assert_eq!(j.to_stability(), Stability::PerRender);
    }
```
(`src/domains/impls/state_value.rs:1158-1175`)

Aucun test ne couvre `Interval::mul` avec un coin `NaN` (§4.2), ni le ⊥ non
canonique face à `partial_cmp` (§3.5), ni `!⊥` (§4.2).

---

## 4. Algorithmes clefs

### 4.1 Opérations ponctuelles du produit

```rust
/// Combine two per-slot orderings; `None` when slots disagree in direction.
fn merge_ord(acc: Option<Ordering>, next: Option<Ordering>) -> Option<Ordering> {
    match (acc?, next?) {
        (Ordering::Equal, x) | (x, Ordering::Equal) => Some(x),
        (Ordering::Less, Ordering::Less) => Some(Ordering::Less),
        (Ordering::Greater, Ordering::Greater) => Some(Ordering::Greater),
        _ => None,
    }
}

impl PartialOrd for StateValue {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        let mut ord = self.num.partial_cmp(&other.num);
        ord = merge_ord(ord, self.boolean.partial_cmp(&other.boolean));
        ord = merge_ord(ord, self.str.partial_cmp(&other.str));
        ord = merge_ord(ord, self.reference.partial_cmp(&other.reference));
        ord = merge_ord(ord, self.null.partial_cmp(&other.null));
        ord = merge_ord(ord, self.undef.partial_cmp(&other.undef));
        ord = merge_ord(ord, self.setter.partial_cmp(&other.setter));
        ord = merge_ord(ord, self.other.partial_cmp(&other.other));
        ord
    }
}
```
(`src/domains/impls/state_value.rs:479-501`)

Ordre produit : `a ⊑ b` ssi chaque emplacement est `⊑`. Les emplacements
booléens (`null`, `undef`, `other`) utilisent l'ordre de `bool`
(`false < true`), qui est bien le treillis {⊥ = false, ⊤ = true}. Un seul
emplacement incomparable, ou deux emplacements de sens opposés, rend le tout
incomparable (test `mixed_direction_slots_are_incomparable`,
`state_value.rs:1099-1109`).

```rust
    fn join(&self, other: &Self) -> Self {
        StateValue {
            num: self.num.hull(&other.num),
            boolean: self.boolean.join(&other.boolean),
            str: self.str.join(&other.str),
            reference: self.reference.join(&other.reference),
            null: self.null || other.null,
            undef: self.undef || other.undef,
            setter: self.setter.join(&other.setter),
            other: self.other || other.other,
        }
    }

    fn meet(&self, other: &Self) -> Self {
        StateValue {
            num: AbstractDomain::meet(&self.num, &other.num),
            boolean: self.boolean.meet(&other.boolean),
            str: AbstractDomain::meet(&self.str, &other.str),
            reference: self.reference.meet(&other.reference),
            null: self.null && other.null,
            undef: self.undef && other.undef,
            setter: AbstractDomain::meet(&self.setter, &other.setter),
            other: self.other && other.other,
        }
    }

    fn widen(&self, other: &Self) -> Self {
        StateValue {
            num: self.num.widen(&other.num),
            boolean: self.boolean.join(&other.boolean),
            str: self.str.widen(&other.str),
            reference: self.reference.join(&other.reference),
            null: self.null || other.null,
            undef: self.undef || other.undef,
            setter: self.setter.join(&other.setter),
            other: self.other || other.other,
        }
    }

    fn widen_to(&self, other: &Self, thresholds: &[f64]) -> Self {
        StateValue {
            num: self.num.widen_to(&other.num, thresholds),
            // Non-numeric slots have no notion of a threshold → plain widen.
            ..self.widen(other)
        }
    }
```
(`src/domains/impls/state_value.rs:631-676`)

Point clef d'ADR-015 : le produit **n'a pas d'opérateur de réduction**. Les
emplacements décrivent des sortes disjointes ; il n'y a aucune information
croisée à propager (« No reduction operator and no query pool »). La
terminaison du produit découle de celle de chaque emplacement (tous de hauteur
finie sauf `num`, qui a son widening).

Complexité : chaque opération est O(1) sauf `str` (O(k log k), k ≤ 4 après
seuil, mais l'union intermédiaire peut contenir |a|+|b| ≤ 8 éléments) et
`reference` (union/intersection de `BTreeSet` ≤ 4+4 éléments). Les `Arc` de
`StrConst::Set` rendent le clonage O(1).

### 4.2 Arithmétique d'intervalles et soundness

```rust
    pub fn add(&self, other: &Self) -> Self {
        if self.is_bottom() || other.is_bottom() {
            return Interval::bottom();
        }
        Interval {
            lo: self.lo + other.lo,
            hi: self.hi + other.hi,
            is_int: self.is_int && other.is_int,
        }
    }

    pub fn sub(&self, other: &Self) -> Self {
        if self.is_bottom() || other.is_bottom() {
            return Interval::bottom();
        }
        Interval {
            lo: self.lo - other.hi,
            hi: self.hi - other.lo,
            is_int: self.is_int && other.is_int,
        }
    }

    pub fn mul(&self, other: &Self) -> Self {
        if self.is_bottom() || other.is_bottom() {
            return Interval::bottom();
        }
        let products = [
            self.lo * other.lo,
            self.lo * other.hi,
            self.hi * other.lo,
            self.hi * other.hi,
        ];
        let lo = products.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = products.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        Interval {
            lo,
            hi,
            is_int: self.is_int && other.is_int,
        }
    }
```
(`src/domains/impls/interval.rs:148-187`)

Soundness : ce sont les formules classiques de l'arithmétique d'intervalles,
correctes sur ℝ. En `f64`, l'arrondi au plus proche est **monotone** : les
bornes calculées avec la même opération arrondie que JavaScript encadrent
toujours les résultats concrets (argument de lecture, non documenté dans le
code). ⊥ est absorbant (un chemin mort reste mort).

**Défaut observé (sonde, §6.0)** : `mul` perd `NaN` dans le repli.
`f64::min(x, NaN) = x`, donc un produit de coins valant `NaN` (`0 × ±∞`) est
ignoré. Pour `[0,0] × [−∞, +∞]`, les quatre coins valent `NaN`, le repli rend
`lo = +∞, hi = −∞` : **⊥**. Or pour tout `x` fini, `0 × x = 0` : le résultat
concret `{0}` est perdu (sous-approximation, direction interdite). Sortie de la
sonde :

```
[0,0] * [-inf,+inf] = Interval { lo: inf, hi: -inf, is_int: false } is_bottom=true
[0,0] * [0,+inf] = Interval { lo: 0.0, hi: 0.0, is_int: true }
```

Effet observable de bout en bout en §6.7. Correctif naturel (non appliqué) :
traiter `0 × ±∞` comme `0`, ou rendre ⊤ quand un coin est `NaN`.

Opérateurs du commit `548f922` (#73) :

```rust
    /// `self % other` with JS semantics: the result takes the dividend's sign
    /// and is strictly smaller in magnitude than both the divisor and the
    /// dividend. `None` when the divisor may be zero — the result may then be
    /// `NaN`, which no interval holds.
    pub fn rem(&self, other: &Self) -> Option<Self> {
        if self.is_bottom() || other.is_bottom() {
            return Some(Interval::bottom());
        }
        if other.lo <= 0.0 && other.hi >= 0.0 {
            return None;
        }
        let is_int = self.is_int && other.is_int;
        let magnitude = other.lo.abs().max(other.hi.abs());
        // Strictly below the divisor's magnitude: one less on integers, the
        // closed bound otherwise.
        let bound = if is_int { magnitude - 1.0 } else { magnitude };
        Some(Interval {
            lo: if self.lo >= 0.0 {
                0.0
            } else {
                self.lo.max(-bound)
            },
            hi: if self.hi <= 0.0 {
                0.0
            } else {
                self.hi.min(bound)
            },
            is_int,
        })
    }

    /// `self ** other`. On the non-negative quadrant the function is monotone
    /// in each argument, so the extremes sit at the corners of the operand
    /// box. `None` outside it: a negative base with a fractional exponent is
    /// `NaN`, and a negative exponent of zero is infinite.
    pub fn pow(&self, other: &Self) -> Option<Self> {
        if self.is_bottom() || other.is_bottom() {
            return Some(Interval::bottom());
        }
        if self.lo < 0.0 || other.lo < 0.0 {
            return None;
        }
        let powers = [
            self.lo.powf(other.lo),
            self.lo.powf(other.hi),
            self.hi.powf(other.lo),
            self.hi.powf(other.hi),
        ];
        Some(Interval {
            lo: powers.iter().cloned().fold(f64::INFINITY, f64::min),
            hi: powers.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            is_int: self.is_int && other.is_int,
        })
    }
```
(`src/domains/impls/interval.rs:200-253`)

Argument de soundness de `rem` : en JS, `x % m` a le signe de `x` et
`|x % m| < |m|` et `|x % m| ≤ |x|`. Sur des entiers, « strictement inférieur à
|m| » donne `|m| − 1` ; sur des réels on garde la borne fermée `|m|`
(sur-approximation). Si le diviseur peut être nul, le résultat peut être `NaN`
→ `None` → ⊤ au niveau `StateValue`. Le choix de `magnitude = max(|lo|, |hi|)`
du diviseur est sûr (la borne la plus lâche). Tests :
`rem_follows_the_dividend_sign_and_stays_below_the_divisor`,
`rem_by_a_possible_zero_is_unrepresentable` (`interval.rs:422-458`).

Argument de `pow` : sur `x ≥ 0, y ≥ 0`, `x^y` est croissante en `x` et monotone
en `y` (croissante si `x > 1`, décroissante si `x < 1`) ; pour chaque `y` fixé
l'extremum en `x` est à une extrémité, puis l'extremum en `y` de
`lo^y`/`hi^y` est à une extrémité : les quatre coins suffisent. `0**0 = 1`
en JS comme en Rust (`powf`). Tests : `pow_takes_the_corners_of_the_non_negative_box`,
`pow_outside_the_non_negative_box_is_unrepresentable` (`interval.rs:460-486`).

Au niveau `StateValue`, la table des opérateurs binaires
(`src/domains/transfer/state_value.rs:752-811`) :

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

La coercition `ToNumber` est centralisée dans `as_arith` :

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

Lecture de soundness, cas par cas :

- `+` : numérique si les deux opérandes sont ⊆ {num, null} ; concaténation
  exacte (produit cartésien, puis seuil de 4 via `str_set`) si les deux sont
  des chaînes pures ; sinon ⊤. Un mélange `string + number` rend ⊤, sûr mais
  imprécis (la concaténation JS donnerait une chaîne).
- `/` : toujours ⊤ (`x/0` = `±Infinity` ou `NaN`).
- `%`, `**` : intervalle exact ou ⊤ (commit `548f922`).
- `&&`, `||` : ⊤ — la valeur n'est presque jamais utilisée : le lowering
  les transforme en diamant de CFG (ADR-020 non-changement n°1).
- comparaisons, `in`, `instanceof` : `BoolVal::Top` pur (ni `num` ni `str`) ;
  même `1 < 2` n'est **pas** évalué à `True` — imprécis mais sûr ; le
  raffinement des branches est fait à part par `narrow_env_for_branch` (§4.4).
- bit à bit : `eval_bitwise` (`:846-929`) garantit toujours un nombre dans la
  plage int32 (uint32 pour `>>>`), resserrée par un masque constant ou un
  décalage constant ; un opérande ⊥ rend ⊥ (`:855-859`).

Opérateurs unaires (`eval_unary`, `:941-1004`) : `-x` via `as_arith`/`neg` ;
`!x` inversé exactement si l'opérande est un booléen pur, sinon ⊤ ;
`typeof x` rend une **chaîne singleton exacte** quand `typeof_name` répond,
sinon `str_top()` (jamais ⊤ complet) ; `~x` dans int32 ; `+x` via
`as_arith` puis `coerce_to_number` (booléen → [0,0]/[1,1]/[0,1], ensemble de
chaînes toutes parsables → enveloppe, sinon ⊤) ; `UnaryOp::Unknown` → ⊤.
Deux détails vérifiés dans le code : `typeof` et `~` rendent ⊥ sur un
opérande ⊥ (chemin mort, `:970-972`, `:983-985`), mais `!⊥` rend
`boolean(Top)` et non ⊥ (le test « booléen pur » est satisfait par ⊥, puis
le `match` tombe dans `_ => BoolVal::Top`, `:948-962`) — imprécis, pas
unsound.

`typeof_name` :

```rust
    pub fn typeof_name(&self) -> Option<&'static str> {
        let kinds = self.populated_kinds();
        if kinds.count() != 1 {
            return None;
        }
        for (bit, name) in [
            (KindMask::NUM, "number"),
            (KindMask::BOOL, "boolean"),
            (KindMask::STR, "string"),
            (KindMask::NULL, "object"),
            (KindMask::UNDEF, "undefined"),
            (KindMask::SETTER, "function"),
        ] {
            if kinds.only(bit) {
                return Some(name);
            }
        }
        None
    }
```
(`src/domains/impls/state_value.rs:250-268`)

La sorte `reference` est volontairement absente (objet *ou* fonction) ; `other`
aussi. `typeof null === "object"` : c'est JavaScript (vérifié : la sonde rend
`typeof null = Some("object")`).

### 4.3 Widening et widening à seuils (ADR-014)

```rust
    /// Widening: if other grows the bound beyond self, jump to ±∞.
    pub fn widen(&self, other: &Self) -> Self {
        if self.is_bottom() {
            return *other;
        }
        if other.is_bottom() {
            return *self;
        }
        Interval {
            lo: if other.lo < self.lo {
                f64::NEG_INFINITY
            } else {
                self.lo
            },
            hi: if other.hi > self.hi {
                f64::INFINITY
            } else {
                self.hi
            },
            is_int: self.is_int && other.is_int,
        }
    }

    /// Threshold widening ("widening up to"). A bound that grows jumps to the
    /// tightest enclosing threshold instead of ±∞; ±∞ is used only when no finite
    /// threshold encloses the grown bound. `thresholds` need not be sorted.
    ///
    /// Sound: result ⊒ self.hull(other) (bounds only ever loosen), and the set is
    /// finite so the ascending chain still stabilises.
    pub fn widen_to(&self, other: &Self, thresholds: &[f64]) -> Self {
        if self.is_bottom() {
            return *other;
        }
        if other.is_bottom() {
            return *self;
        }
        let lo = if other.lo < self.lo {
            // Largest threshold ≤ other.lo, else -∞.
            thresholds
                .iter()
                .copied()
                .filter(|&t| t <= other.lo)
                .fold(f64::NEG_INFINITY, f64::max)
        } else {
            self.lo
        };
        let hi = if other.hi > self.hi {
            // Smallest threshold ≥ other.hi, else +∞.
            thresholds
                .iter()
                .copied()
                .filter(|&t| t >= other.hi)
                .fold(f64::INFINITY, f64::min)
        } else {
            self.hi
        };
        Interval {
            lo,
            hi,
            is_int: self.is_int && other.is_int,
        }
    }
```
(`src/domains/impls/interval.rs:85-146`)

Formule (ADR-014) :

```
[a,b] ∇_T [c,d] = [ c < a ? max{t ∈ T ∪ {−∞} | t ≤ c} : a ,
                    d > b ? min{t ∈ T ∪ {+∞} | t ≥ d} : b ]
```

Remarques :

- Hypothèse d'usage : `other ⊒ self` (le moteur élargit l'ancien état par le
  nouveau, qui l'inclut déjà car les stores accumulent par `join`). Si `other`
  n'inclut pas `self`, `widen` garde `self.lo`/`self.hi` et reste une borne
  supérieure du *hull* seulement pour la borne qui n'a pas grandi : correct.
- Soundness : `widen_to(a, b, T) ⊒ a ⊔ b` (test
  `widen_to_is_sound_superset_of_hull`, `interval.rs:568-576`).
- Terminaison : chaque borne ne peut prendre que des valeurs de `T ∪ {±∞}`
  après son premier élargissement ; `T` est fini, donc au plus `|T| + 1` sauts
  par borne.
- `is_int` : `widen([0,0],[0,1]) = [0, +∞]` avec `is_int = true` (sonde) — la
  borne ∞ ne rend pas l'intervalle non entier ; c'est cohérent avec
  « prouvé entier ».

Récolte des seuils (moteur, hors périmètre mais indispensable) :

```rust
/// Harvest the finite threshold set for "widening up-to" (see ADR-014).
///
/// Collects numeric literals from the render CFG, all hook bodies, and useState
/// init expressions — the constants against which guarded state growth is
/// bounded. Over-collecting is harmless: the set stays finite (termination) and
/// extra thresholds only add candidate bounds (precision, never unsoundness).
fn collect_thresholds(render_cfg: &CFG, hooks: &[HookEntry]) -> Vec<f64> {
    let mut out: Vec<f64> = Vec::new();
    collect_lits_cfg(render_cfg, &mut out);
    for hook in hooks {
        match hook {
            HookEntry::State { init, .. } => collect_lits_expr(init, &mut out),
            HookEntry::Effect { body_cfg, .. } | HookEntry::Handler { body_cfg, .. } => {
                collect_lits_cfg(body_cfg, &mut out)
            }
            _ => {}
        }
    }
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out.dedup();
    out
}
```
(`src/engine/fixpoint.rs:1185-1206`)

Note : ADR-014 prévoyait de récolter seulement les littéraux des gardes et des
inits ; le code récolte **tous** les littéraux numériques des CFG (y compris
dans les `FnLit`, `fixpoint.rs:1217-1219`). C'est plus large, donc sûr.

Deux points d'application :

1. Boucle externe (render → mémos → effets → handlers) : après
   `config.widen_threshold` itérations (défaut **3**, `fixpoint.rs:51-59` —
   ADR-002 disait 2), l'état est élargi par `state.widen_to(&new_state,
   &thresholds)` (`fixpoint.rs:514-529`) ; au-delà de 100 itérations, widening
   forcé sans seuils (`:500-512`) — `state = state.widen(&new_state);` suivi
   d'un `break` **immédiat**, sans nouveau test `leq` : l'état retenu n'est
   donc pas vérifié post-point-fixe à ce tour (le moteur rafraîchit ensuite la
   passe de rendu, `:532` « Post-convergence: refresh the render pass » ;
   que ce soit suffisant pour la soundness sur une entrée pathologique est
   **à vérifier** dans le dossier moteur). Le test de convergence est
   `new_state.leq(&state)` (`:495`), qui passe par `PartialOrd` et ignore
   `is_int`.
2. Arcs retour des CFG (`analyze_cfg`) :

```rust
            let new_entry = match entry_envs.get(&succ) {
                None => outgoing_env,
                Some(existing) => {
                    if is_back {
                        let cnt = back_edge_counts.entry(succ).or_insert(0);
                        *cnt += 1;
                        if *cnt >= widen_threshold {
                            existing.widen_to(&outgoing_env, thresholds)
                        } else {
                            existing.join(&outgoing_env)
                        }
                    } else {
                        existing.join(&outgoing_env)
                    }
                }
            };
```
(`src/engine/cfg_analyzer.rs:97-112`)

`StateStore::widen_to` est ponctuel sur les labels
(`src/domains/stores/state_store.rs:58-62`), `StateStore::update` est un
*weak update* monotone `self[label] = self[label] ⊔ val` (`:29-33`).

### 4.4 Raffinement sur gardes (« narrowing » de branches)

Côté intervalle :

```rust
    // Narrowing: restrict interval to satisfy a comparison against a literal `v`.
    //
    // For `<`/`>` the sound bound over reals is `v` itself (the excluded
    // endpoint is kept — a float state `x = 1.7` under `x < 2` would lose 1.7 if
    // we used `v ∓ 1`, an unsound value FN). Integer tightening (`x < 5 ⟹ x ≤ 4`)
    // is applied only when `is_int` proves every value is an integer; that is
    // what keeps ADR-014 threshold widening able to bound `i < N; i++` loops.
    pub fn narrow_lt(&self, v: f64) -> Self {
        let bound = if self.is_int { v.ceil() - 1.0 } else { v };
        Interval {
            lo: self.lo,
            hi: self.hi.min(bound),
            is_int: self.is_int,
        }
    }
    pub fn narrow_leq(&self, v: f64) -> Self {
        let bound = if self.is_int { v.floor() } else { v };
        Interval {
            lo: self.lo,
            hi: self.hi.min(bound),
            is_int: self.is_int,
        }
    }
    pub fn narrow_gt(&self, v: f64) -> Self {
        let bound = if self.is_int { v.floor() + 1.0 } else { v };
        Interval {
            lo: self.lo.max(bound),
            hi: self.hi,
            is_int: self.is_int,
        }
    }
    pub fn narrow_geq(&self, v: f64) -> Self {
        let bound = if self.is_int { v.ceil() } else { v };
        Interval {
            lo: self.lo.max(bound),
            hi: self.hi,
            is_int: self.is_int,
        }
    }
    pub fn narrow_eq(&self, v: f64) -> Self {
        if self.lo <= v && v <= self.hi {
            Interval::point(v)
        } else {
            Interval::bottom()
        }
    }
    /// Conservative: can't split an interval at an interior point; return self.
    /// Exception: a point interval equal to `v` is exactly excluded → ⊥.
    pub fn narrow_neq(&self, v: f64) -> Self {
        if self.is_point() && self.lo == v {
            Interval::bottom()
        } else {
            *self
        }
    }
```
(`src/domains/impls/interval.rs:255-309`)

Vérification des arrondis entiers : `x < 4.5` ⇒ `x ≤ ⌈4.5⌉−1 = 4` ;
`x < 5` ⇒ `x ≤ 4` ; `x > 4.5` ⇒ `x ≥ ⌊4.5⌋+1 = 5` ; `x > 5` ⇒ `x ≥ 6` ;
`x ≤ 4.5` ⇒ `x ≤ 4` ; `x ≥ 4.5` ⇒ `x ≥ 5`. Tous exacts sur ℤ. Sur des réels
(`is_int = false`) on garde la borne `v` fermée (sur-approximation d'une
borne ouverte). Sonde : `1.7 narrow_lt 2 = [1.7, 1.7]`, `[0,9] int narrow_lt
4.5 = [0, 4]`. Le bit `is_int` a été introduit par le commit `29a6709`
(2026-07-22, « Wave-0 soundness FNs ») précisément pour corriger le FN
« `x = 1.7` sous `x < 2` ».

Côté `StateValue` : les comparaisons numériques ne touchent **que** `num` et
gardent les autres emplacements (« `null < 5` est vrai en JS »,
`state_value.rs:604-605`). Les gardes de nullité et de véracité :

```rust
    /// Taken `x == null` guard: only null/undefined survive (`other` may hide
    /// either, so it conservatively re-enables both).
    fn narrow_keep_nullish_only(self) -> Self {
        StateValue {
            null: self.null || self.other,
            undef: self.undef || self.other,
            ..Self::bottom_value()
        }
    }

    /// Taken truthiness guard `if (x)`: excludes every falsy JS value —
    /// null, undefined, 0, "" and false. References/setters are always
    /// truthy → unchanged; `other` may hide truthy kinds → kept.
    /// (NaN is falsy too but intervals never claim to contain it.)
    fn narrow_truthy(mut self) -> Self {
        self.null = false;
        self.undef = false;
        self.num = self.num.narrow_neq(0.0);
        self.boolean = match self.boolean {
            BoolVal::False => BoolVal::Bottom,
            BoolVal::Top => BoolVal::True,
            b => b,
        };
        if let StrConst::Set(set) = &self.str
            && set.contains("")
        {
            let filtered: BTreeSet<String> =
                set.iter().filter(|s| !s.is_empty()).cloned().collect();
            self.str = StrConst::from_set(filtered); // empty set → ⊥
        }
        self
    }

    /// Falsy branch (`else` of `if (x)`, taken `if (!x)`): only falsy values
    /// survive — null, undefined, 0, "" and false. References and setters
    /// are always truthy → ⊥.
    fn narrow_falsy(mut self) -> Self {
        self.num = self.num.narrow_eq(0.0);
        self.boolean = match self.boolean {
            BoolVal::True => BoolVal::Bottom,
            BoolVal::Top => BoolVal::False,
            b => b,
        };
        self.str = match &self.str {
            StrConst::Set(set) if set.contains("") => StrConst::singleton(String::new()),
            StrConst::Top => StrConst::singleton(String::new()),
            _ => StrConst::Bottom,
        };
        self.reference = Stability::Bottom;
        self.setter = SetterVal::Bottom;
        self
    }
```
(`src/domains/impls/state_value.rs:551-602`)

Soundness, point par point :

- `narrow_keep_nullish_only` : `other` peut cacher null/undefined → les deux
  sont réactivés ; tout le reste meurt. Sûr pour `==` comme pour `===` (l'IR
  confond les deux en `Eq`).
- `narrow_truthy` : on ne peut pas couper `[0,5]` en `(0,5]` → gardé entier ;
  seul le point `[0,0]` meurt (`-0` aussi : `-0.0 == 0.0` en Rust). `other`
  est gardé (il peut cacher des valeurs vraies).
- `narrow_falsy` : `other` est **gardé** (il peut cacher `NaN`, `0n`) ; les
  références et setters meurent (toujours vrais).

Le moteur choisit l'opérateur selon la forme syntaxique de la condition :

```rust
    match cond {
        Expr::BinOp { op, lhs, rhs } => {
            let Expr::Var(x) = lhs.as_ref() else {
                return env.clone();
            };
            match rhs.as_ref() {
                Expr::Lit(Prim::Int(_) | Prim::Float(_)) => {
                    let v = match rhs.as_ref() {
                        Expr::Lit(Prim::Int(n)) => *n as f64,
                        Expr::Lit(Prim::Float(f)) => *f,
                        _ => unreachable!(),
                    };
                    let cur = env.lookup(x);
                    let refined = match (op, taken) {
                        (BinOp::Lt, true) => cur.narrow_lt(v),
                        (BinOp::Lt, false) => cur.narrow_geq(v),
                        (BinOp::Leq, true) => cur.narrow_leq(v),
                        (BinOp::Leq, false) => cur.narrow_gt(v),
                        (BinOp::Gt, true) => cur.narrow_gt(v),
                        (BinOp::Gt, false) => cur.narrow_leq(v),
                        (BinOp::Geq, true) => cur.narrow_geq(v),
                        (BinOp::Geq, false) => cur.narrow_lt(v),
                        (BinOp::Eq, true) => cur.narrow_eq(v),
                        (BinOp::Eq, false) => cur.narrow_neq(v),
                        (BinOp::Neq, true) => cur.narrow_neq(v),
                        (BinOp::Neq, false) => cur.narrow_eq(v),
                        _ => cur,
                    };
                    with_refined(x, refined)
                }
```
(`src/engine/cfg_analyzer.rs:212-241` ; suivent les cas `Lit(Null)`,
`Lit(Unit)` → nullité, `:242-259`, puis `Var(x)` → véracité et
`!Var(x)` → véracité inversée, `:263-291`)

Motifs **non** reconnus (l'env est cloné tel quel — sûr, imprécis) : littéral
à gauche (`10 > count`), comparaison entre deux variables, comparaison à une
chaîne (`mode === "a"`), `typeof x === "string"` (malgré le commentaire
d'`eval_unary`, `transfer/state_value.rs:966-968`, qui présente l'exactitude de
`typeof` comme ce qui rend la garde « narrowable » : aucun consommateur ne
raffine sur ce motif — **à vérifier** s'il est prévu ailleurs), conditions
composées `&&`/`||` (diamants, ADR-020 n°1).

**Subtilité de soundness** : un raffinement sur `(BinOp::Lt, false)` utilise
`narrow_geq`, c'est-à-dire la négation *numérique*. Pour une valeur non
numérique (`undefined < 5` est `false`), la branche `else` est prise alors que
`num` est raffiné vers `≥ 5` : sûr, parce que `narrow_geq` ne touche que
`num` et garde `undef`/`other`.

### 4.5 `to_stability` : projection « motion-wins »

```rust
    pub fn to_stability(&self) -> Stability {
        if self.other {
            return Stability::Unknown;
        }
        let mut acc = Stability::Bottom;
        let mut in_motion = false;
        if !self.num.is_bottom() {
            if self.num.is_point() {
                acc = acc.join(&Stability::Stable);
            } else {
                in_motion = true;
            }
        }
        match self.boolean {
            BoolVal::Bottom => {}
            BoolVal::True | BoolVal::False => acc = acc.join(&Stability::Stable),
            BoolVal::Top => acc = acc.join(&Stability::Unknown),
        }
        match &self.str {
            StrConst::Bottom => {}
            StrConst::Set(set) if set.len() == 1 => acc = acc.join(&Stability::Stable),
            _ => acc = acc.join(&Stability::Unknown),
        }
        match &self.reference {
            Stability::PerRender => in_motion = true,
            s => acc = acc.join(s),
        }
        if self.null || self.undef {
            acc = acc.join(&Stability::Stable);
        }
        if self.setter != SetterVal::Bottom {
            // React guarantees setter identity across renders.
            acc = acc.join(&Stability::Stable);
        }
        // Each populated kind slot is individually "stable" when it holds a
        // single point, but two populated kinds are two DISTINCT concrete
        // values (`Object.is` never equates across kinds): the value can
        // transition between them (`useState<string>()` + `setX("data")` in
        // a handler → {undefined, "data"}). A cross-kind union is never
        // definitely stable.
        // `other` is already handled by the early return above, so it is ⊥ here
        // and does not inflate the count.
        if self.populated_kinds().count() >= 2 {
            acc = acc.join(&Stability::Unknown);
        }
        if in_motion {
            return Stability::PerRender;
        }
        acc
    }
```
(`src/domains/impls/state_value.rs:320-369`)

Algorithme :

1. `other` ⇒ `Unknown` (une valeur opaque ne revendique jamais une
   (in)stabilité définie).
2. Chaque emplacement contribue : point numérique/booléen exact/chaîne
   singleton/null/undefined/setter ⇒ `Stable` ; booléen ⊤, ensemble de ≥ 2
   chaînes, chaîne ⊤ ⇒ `Unknown` ; référence ⇒ sa propre stabilité, sauf
   `PerRender` qui met `in_motion`.
3. Deux sortes peuplées ⇒ `Unknown` (on peut passer de l'une à l'autre).
4. **Motion-wins** : un intervalle non ponctuel ou une référence `PerRender`
   rend `PerRender` *même si* un autre emplacement dit `Unknown`.

**Piège majeur** : `PerRender` n'a pas le même sens selon sa provenance. Dans
l'emplacement `reference`, c'est une **borne must** (« nouvelle référence à
chaque rendu, garanti »). Renvoyé par `to_stability` pour un intervalle non
ponctuel, c'est seulement « **peut** changer à chaque rendu » (ADR-017
§Consumer rewiring : « the name reads oddly for a number but means "may change
every render", kind-agnostic »). Sonde : `boolean ⊔ [0,5]` → `PerRender`,
alors que ni l'un ni l'autre n'est « must-fresh ». Les consommateurs qui ont
besoin du fait must lisent l'emplacement directement :

```rust
    pub fn is_unstable_reference_only(&self) -> bool {
        self.reference == Stability::PerRender && self.populated_kinds().only(KindMask::REF)
    }
```
(`src/domains/impls/state_value.rs:285-287`)

Côté règles, `StabilityVerdict` (`src/rules/api/query.rs:143-179`) documente
la même mise en garde et replie `Bottom` et `Unknown` sur le côté may :

```rust
    pub fn of(stability: Stability) -> Self {
        match stability {
            Stability::Stable => StabilityVerdict::Stable,
            Stability::Versioned(slots) => StabilityVerdict::Versioned(slots),
            Stability::VersionedTop => StabilityVerdict::Versioned(BTreeSet::new()),
            Stability::PerRender => StabilityVerdict::PerRender,
            Stability::Bottom | Stability::Unknown => StabilityVerdict::Unknown,
        }
    }
```
(`src/rules/api/query.rs:164-172`)

ADR-020 non-changement n°4 interdit de supprimer `to_stability` (projection
documentée, testée, sans couplage lossy démontré).

### 4.6 Conversion côté lecture : `Versioned` (ADR-017 §2)

```rust
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
```
(`src/domains/transfer/state_value.rs:122-135`)

C'est **l'unique** point de conversion. Il fonde la **double vue** de l'état :

- le **store** (`StateStore`) contient le join des valeurs *écrites* — vue
  « événement ». `redundant-set-state` le lit directement ;
- l'**évaluation** de `StateVal(l)` rend la vue « inter-rendus », ce que
  `Object.is` compare entre deux rendus. Les règles de deps lisent celle-ci.

Exemple observé (sonde, §6.4) : `useState({ locale: "en" })` a pour store
`ref(PerRender)` (valeur écrite = littéral objet frais), mais toute lecture
donne `ref(Versioned({(C, 0)}))`.

Hypothèse de soundness n°1 d'ADR-017 : les sets ont lieu hors rendu ; un set
pendant le rendu avec une référence fraîche fait changer le slot à chaque
rendu (ce que `Versioned` sous-estime) — violation couverte par
`setter-in-render` (Error). Hypothèse n°2 : `Versioned` n'est accepté comme
« garde » dans `all_deps_*` que **couplé** au bras churn d'`infinite-loop`
(sinon on rouvre le FN `ObjChurn`).

La conservation des étiquettes passe par `versioned_reference` :

```rust
    pub fn versioned_reference(&self) -> Option<Stability> {
        match &self.reference {
            Stability::Versioned(_) | Stability::VersionedTop => Some(self.reference.clone()),
            _ => None,
        }
    }
```
(`src/domains/impls/state_value.rs:407-412`)

utilisée par la lecture de champ d'un objet versionné
(`transfer/state_value.rs:617-627` : `reference: obj_val.versioned_reference()
.unwrap_or(Stability::Unknown), ..StateValue::top()`) et par
`recompute_memo` :

```rust
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
```
(`src/domains/transfer/state_value.rs:67-100`)

La valeur d'un mémo est **uniquement** une stabilité logée dans l'emplacement
`reference` : `useMemo(() => 1, [])` vaut `ref(Stable)`, pas le nombre 1
(sonde §6.4). Conséquence « gratuite » (ADR-017) : `join(Versioned({X}),
Stable) = Versioned({X})` propage les étiquettes le long des chaînes de mémos.
Conséquence moins heureuse : §6.6 (FP observé).

### 4.7 Prédicats consommés par les règles

```rust
    pub fn is_unbounded(&self) -> bool {
        (!self.num.is_bottom() && (self.num.lo.is_infinite() || self.num.hi.is_infinite()))
            || self.reference == Stability::PerRender
            || self.other
    }

    /// True if the value ranges over a few primitives only: booleans,
    /// `null`/`undefined`, a finite set of string constants, a narrow integer
    /// range. A state holding such a value re-renders only when a write
    /// *changes* it (React bails out on `Object.is`-equal writes), so however
    /// often it is written, it re-renders at the rate of its transitions.
    pub fn is_finitely_valued(&self) -> bool {
        const NARROW: f64 = 16.0;
        let num_ok = self.num.is_bottom()
            || (self.num.is_int
                && self.num.lo.is_finite()
                && self.num.hi.is_finite()
                && self.num.hi - self.num.lo <= NARROW);
        num_ok
            && matches!(self.str, StrConst::Bottom | StrConst::Set(_))
            && self.reference == Stability::Bottom
            && self.setter == SetterVal::Bottom
            && !self.other
    }
```
(`src/domains/impls/state_value.rs:378-401`)

- `is_unbounded` : garde n°2 du bras point fixe d'`infinite-loop`
  (`src/rules/impls/infinite_loop.rs:130-136`, `:202`). Le test
  `is_unbounded_requires_active_num_slot` protège le piège des bornes
  sentinelles de ⊥ (`[+∞, −∞]` a des bornes infinies !).
- `is_finitely_valued` : utilisé par `wasted-subtree-render`
  (`src/rules/impls/wasted_subtree_render.rs:167`). **C'est le seul prédicat
  qui lit `is_int`** hors narrowing.
- `as_setter` (`state_value.rs:270-279`) : identité exacte seulement si le
  setter est la seule sorte peuplée.
- `is_stable` = `to_stability() == Stable` (`:414-417`).

Carte des consommateurs (recherche `grep` hors `src/domains/impls/`) :
`is_unstable_reference_only` → `always_unstable_deps.rs:164`,
`unstable_context_value.rs:26`, `helpers/jsx.rs:293`, `api/query.rs:153,214`,
`engine/written.rs:188`, `engine/fixpoint.rs:1176` ; `StateValue::is_stable` →
`missing_deps.rs:88,156,188`, `redundant_set_state.rs:269`,
`unnecessary_rerender.rs:129,141` (attention : `api/query.rs:226` et
`helpers/mod.rs:233` appellent `StabilityVerdict::is_stable`,
`query.rs:176`, pas celui de `StateValue`) ; `to_stability` →
`api/query.rs:186`, `api/query.rs:886`, `helpers/mod.rs:44`,
`transfer/state_value.rs:97` (dans `recompute_memo`) — les mentions de
`lowering/hook_extractor.rs:756` et `ir/expr.rs:309,318` ne sont que des
commentaires ; `VersionedTop` → `stale_closure.rs:163`,
`frozen_initial_state.rs:38` (doc), `api/query.rs:168,883` ;
`as_setter` → `transfer/state_value.rs:280,340,440,444,457`,
`interp/interpreter.rs:375`, `engine/setters.rs:302,328` ; `typeof_name` →
`transfer/state_value.rs:973` (`typeof`) et `:1013` (`coerce_to_number`) ;
`versioned_reference` → `transfer/state_value.rs:96,625` ; `is_top_value`
→ `engine/written.rs:377` (plus deux assertions de tests,
`cfg_analyzer.rs:752`, `fixpoint.rs:2006`). Relevé fait par
`grep -rn` sur `src/` au commit `e67b10a`.

`value_freshness` (`src/engine/written.rs:185-200`) classe une valeur écrite
pour le graphe de churn : `PerRender` pur ⇒ `Fresh`, `PerRender` mêlé ⇒
`Maybe`, `Unknown` ⇒ `Maybe`, `Versioned` par le slot cible seul ⇒ `Not`,
résidu `other` ⇒ `Maybe`, sinon `Not`. Commentaire décisif :
« Churn is about the REFERENCE kind only: a widened numeric value
(`count + 1`) changes but never fails `Object.is` freshly » (`:173-175`).

---

## 5. Décisions de conception

### 5.1 ADR-002 — *Abstract domains — stability lattice + 3 stores*

- **Statut** : Accepted (2026-05-29). Largement **dépassé** dans les faits par
  ADR-015 et ADR-017, sans marque `Superseded` dans le fichier.
- **Décide** : le domaine central est la stabilité référentielle (`Object.is`),
  treillis à 4 points `⊥ < {Stable, Unstable} < Unknown` ; table de transfert
  statique (littéral primitif ⇒ Stable ; `{}`/`[]`/`() => {}` ⇒ Unstable ;
  setter ⇒ Stable ; `useRef()` ⇒ Stable ; appel non-hook ⇒ Unstable ;
  `obj.prop` ⇒ Unknown…) ; trois stores séparés (`StateStore` soumis au point
  fixe, `MemoStore` fonction des deps, `RefStore` trivial) car les trois
  familles de hooks ont des sémantiques différentes vis-à-vis du cycle de
  rendu ; widening après un seuil configurable (défaut 2).
- **Alternative refusée** : un store unifié (forcerait tous les hooks dans le
  même point fixe).
- **Ce qui a changé** : `Unstable` est devenu `PerRender` et le treillis a
  gagné `Versioned`/`VersionedTop` (ADR-017) ; il n'y a pas de `RefStore`
  (`useRef` rend `reference(Stable)` via `MarkerVal::StableRef`,
  `transfer/state_value.rs:144-147`) ; `src/domains/stability.rs` et
  `src/domains/product.rs` n'existent pas (la stabilité est dans
  `impls/stability.rs`, le produit est `StateValue`) ; le seuil par défaut est
  3 (`fixpoint.rs:54`) ; « appel non-hook ⇒ Unstable » est devenu ⊤ sauf les
  allocateurs connus (`returns_fresh_reference`, `transfer/state_value.rs:224-259`).

### 5.2 ADR-008 — *Value domain for the SCC fixpoint — StateValue enum + TypedStateStore*

- **Statut** : **Superseded by ADR-015** (2026-07-14).
- **Décidait** : `StateValue` enum plat (`Bottom | Null | Undefined |
  Number(Interval) | Boolean(BoolVal) | StrConst(Arc<…>) | Str |
  Reference(Stability) | Top`) — un seul genre par valeur ; tout join
  inter-genres ⇒ `Top` ; compensé par un `TypedStateStore` à cinq sous-stores
  dispatché par un `StateType` inféré de l'init et par l'indication
  TypeScript `useState<number>(null)`.
- **Ce qui survit** : `Interval`, `BoolVal`, `StrConst` (seuil 4), la
  machinerie widening/narrowing, le signal « le store s'élargit ⇒ boucle
  potentielle ».
- **Problème résiduel qui a motivé ADR-015** : `useState(null)` sans
  annotation ⇒ `join(Null, Number) = Top` ⇒ FN documenté.
- **Obsolète dans ADR-008** : la ligne du tableau « Boolean ✓ oscillation
  true↔false » ne correspond pas au comportement actuel (§6.3).

### 5.3 ADR-014 — *Widening up-to (thresholds) + narrowing*

- **Statut** : Accepted ; Partie 1 implémentée ; **Partie 2 superseded**
  (révision 2026-06-28).
- **Partie 1 (implémentée)** : widening à seuils ASTRÉE sur un ensemble fini
  de constantes récoltées une fois par composant ; appliqué à la boucle
  externe **et** aux arcs retour internes d'`analyze_cfg`. La signature de
  `AbstractDomain::widen` n'est pas changée (bruit), on ajoute `widen_to`
  avec défaut.
- **Partie 2 (non implémentée)** : un opérateur `narrow` et une phase
  descendante bornée. Refusée parce que, *empiriquement*, le narrowing sur
  gardes pendant la montée plus les seuils suffisent (compteur gardé ⇒
  `[0,10]`, boucle locale ⇒ `[0,5]`) ; les deux techniques ne récupèrent que
  des **bornes littérales** ; et une phase descendante ne pourrait de toute
  façon pas faire redescendre le store d'état (les écritures s'accumulent par
  `join` monotone). `narrow` reste une infrastructure différée, justifiée
  seulement si un jour des bornes symboliques sont nécessaires.
- **Argument de soundness** retenu : `widen_to(a,b,T) ⊒ a ⊔ b` et `T` fini.
- Parties d'ADR-014 non réalisées : `WidenOutcome { ToThreshold, ToTop }`,
  suppression de `effect_setter_writes` (le champ existe toujours,
  `src/engine/analysis_result.rs:218`) — **à vérifier** avec le dossier
  moteur.

### 5.4 ADR-015 — *Product value domain over disjoint JS kinds*

- **Statut** : Implemented (2026-07-14), **supersedes ADR-008**.
- **Observation clef** : pour une somme de sortes **disjointes**, la
  complétion disjonctive dégénère en **produit ponctuel**. D'où un emplacement
  par sorte, ⊥ quand la sorte est impossible ; `join`/`meet`/`widen`/
  `widen_to`/`partial_cmp` ponctuels.
- **Supprime** `TypedStateStore`, `StateType`, `infer_state_type`,
  `HookEntry::State.type_hint` et la surcharge `null` ⇒ `Number([0,0])`.
  `useState<T>(…)` devient décoratif.
- **`ComponentSetter` devient un emplacement** (`SetterVal`), `as_setter`
  répond seulement si c'est la seule sorte peuplée.
- **Choix sémantiques** : `to_stability` motion-wins ; arithmétique avec
  `ToNumber(null) = 0` (d'où la détection de `useState(null)` + `setN(n+1)`) ;
  `undefined` (NaN) conservateur ; raffinement de nullité (enveloppe de `==`
  et `===`) ; véracité complète.
- **Refus explicite** : pas d'opérateur de réduction ni de *query pool*
  MOPSA à l'intérieur du produit (rien à partager entre sortes disjointes) ; un
  futur produit réduit se fera **au niveau analyse** (`Value × Relational`),
  avec `QueryContext` comme canal de lecture ; pas de pool n-aire (pas de
  GADT en Rust, ADR-007).
- Tests épinglés : `tests/narrowing.rs::null_init_without_hint_unbounded_is_flagged`,
  `nullable_fetch_pattern_no_false_positive`.

### 5.5 ADR-017 — *Versioned reference stability — may/must change bounds*

- **Statut** : Implemented (2026-07-15), raffine ADR-015.
- **Contexte** : 5 FP `always-unstable-deps` (corpus memos, famille F5) sur
  des fournisseurs de contexte à état objet (`useState({locale:"en"})` en dep)
  — le domaine confondait fraîcheur à l'allocation et fraîcheur par rendu. Mais
  un correctif naïf ouvrait un FN réel : `ObjChurn` (effet qui fait
  `setObj({...obj})` avec `[obj]`) n'était détecté *que* par ce même warning.
- **Décide** : (1) le treillis à 6 points (§3.6) ; (2) conversion côté
  lecture en un seul point (`StateVal`) ; (3) un bras churn dans
  `infinite-loop` (Error si dep = slot exact ∧ set sur tous les chemins ∧
  valeur `PerRender` ; Warning si dep `Versioned(S)` avec X ∈ S ∧ set
  atteignable ∧ valeur fraîche ou `Unknown`) ; (4) recâblage des
  consommateurs (`always_unstable_deps` ne tire plus que sur `PerRender`,
  `Versioned` garde dans `all_deps_unstable`, `missing_deps` inchangé).
- **Alternatives refusées** : encoder un compteur d'événements dans le store
  pour faire tirer le widening (« non-standard hack ») ; un nouveau nom de
  diagnostic (même classe de panne) ; garder les points (OnSet, OnSet) et
  (Every, OnSet) dans le treillis (un seul consommateur prospectif, qui a
  besoin d'une analyse d'atteignabilité).
- **Limites notées** : cycles multi-effets (résolu par ADR-018) ; raffinement
  « jamais écrit » ⇒ `Stable` abandonné (issue #41) ; `FieldAccess` sur objet
  versionné.
- **Obsolète dans ADR-017** : `Versioned(BTreeSet<(Symbol, HookLabel)>)` —
  c'est `QualifiedSlot = (ComponentId, HookLabel)` depuis #7.

### 5.6 ADR-020 — décisions de non-changement touchant le périmètre

- D5 : macro `flat_lattice!` (réalisée). D4 : `KindMask`/`populated_kinds`
  (réalisée).
- Non-changement n°4 : **ne pas supprimer `to_stability`**.
- Non-changement n°5 : **ne pas extraire `BoundedPowerset<T,N>`**.
- Non-changement n°3 : `may_written_slots` reste syntaxique (un bit
  « observé » pourrait sous-compter sur un chemin élagué ⇒ FN) — lié à #41.

### 5.7 ADR-007 et ADR-001

- ADR-007 (*Cross-domain queries — QueryContext trait (B3 implemented)*,
  statut « Implemented (B3 active, B1 groundwork in place) », 2026-06-02) :
  « Decision A » = le trait `QueryContext` (solution B3) ; « Decision B » =
  migration future vers un gestionnaire générique (B1, types marqueurs +
  bornes `where`) si le nombre de domaines dépasse ~5 — non réalisée. `dyn QueryContext`
  pour garder `Transfer` object-safe ; MOPSA-`ask` impossible en Rust stable
  (pas de GADT, spécialisation instable). Le trait réel a une seule méthode
  (`callback_body`) — la description d'ADR-007 est datée (§3.8).
- ADR-001 (*React-tRace as reference concrete semantics*) : la sémantique
  concrète de référence est React-tRace (Lee, Ahn, Yi, OOPSLA 2025), étendue
  localement pour deps, `useMemo`, `useCallback`, `useRef`, objets.
  ADR-001 annonce ces extensions dans `docs/semantics.md` : **ce fichier
  n'existe pas** au commit étudié (`ls docs/`), et aucune fonction de
  transfert du périmètre ne cite une règle React-tRace — **à vérifier**.

### 5.8 Commit `548f922` (#73) — `%`, `**`, `in`, `instanceof`

Les quatre opérateurs deviennent des variantes réelles de `BinOp` (`Mod`,
`Pow`, `In`, `InstanceOf`), suivant la décision prise avec #74/#75 : « une
variante par opérateur, jamais un opérateur source porté dans `Unknown` ».
`BinOp::Unknown` n'a plus de producteur et est **supprimé** ; `lower_binop` est
exhaustif (plus de joker `_ =>`), donc un nouvel opérateur du langage ne peut
plus tomber silencieusement dans l'opaque. Arithmétique dans la couche
intervalle (`rem`, `pow`, §4.2) ; `in`/`instanceof` ⇒ `BoolVal::Top`
(`num` et `str` à ⊥, donc une garde sur le résultat reste raffinable) ; `%=` et
`**=` passent par `faithful_compound_binop`. Corpus inchangé (1499 → 1499).
`docs/limitations.md` mis à jour (`NaN` ⇒ ⊤ pour `/`, `%` par diviseur
possiblement nul, `**` à base ou exposant négatifs).

### 5.9 Issues GitHub

Issues fermées `wontfix` (`gh issue list --state closed --label wontfix`) :
#101, #65, #63, #51, #42, #40. **Aucune ne porte directement sur les
domaines** ; la plus proche est #40 (« whole-object read via guard/nullish is
flagged ») : une garde `if (!x)` lit la référence entière, et distinguer
« usage de la valeur » de « test d'existence » demanderait un suivi du type de
consommation — refusé, le warning est sûr et aligné sur eslint.

Issues ouvertes `area/domain` pertinentes : #22 (FN, `slice`/`concat` ne
sont pas prouvés frais — le corps dit « Closed as a recorded decision » mais
l'issue est **OPEN** ; lever la limite demande la *sorte du receveur* dans le
produit), #34 (FP, constante de module `const X = f()` : le produit n'a pas
d'encodage « sorte inconnue, identité stable »), #41 (FP, slot jamais écrit
⇒ `Versioned` au lieu de `Stable`), #76 (FN/précision, spreads et clés
calculées), #157 (soundness : une écriture d'une valeur dérivée du slot écrit
— un mémo sur lui — est lue « non fraîche » ; le correctif demande un point
`Slot(q)` **sous** `Versioned({q})` dans le treillis de stabilité), #159 (FP,
setter prop typé primitif), #144 (Error jamais atteint sur la divergence de
valeur, et remarque que « `n + 1` égale `n` à partir de 2^53 et pour
`±Infinity` »), #28 (`useContext` non modélisé ⇒ ⊤).

### 5.10 Principes de CLAUDE.md qui s'appliquent

- **Soundness** (FP tolérés, FN interdits) : chaque défaut par défaut du trait
  est l'identité ou ⊤ ; `from_state_value` par défaut ⊥ est l'exception
  (§3.1).
- **Pas de workarounds / correction au niveau central** : la conversion
  `Versioned` est faite *une fois* dans `eval_state_value` ; `recompute_memo`
  insiste (« Not a store workaround — a genuine memo-side projection »).
- **Modulaire et général** : `flat_lattice!`, `KindMask`, `as_arith`
  centralisent ce que plusieurs règles utiliseraient.
- **Paragraphe unique** : ADR-015 et ADR-017 s'appuient chacun sur *une*
  observation (sortes disjointes ⇒ produit ; un slot ne change qu'à ses sets).

### 5.11 Historique (`git log --oneline -- src/domains/impls/ src/domains/mod.rs src/domains/context.rs`)

41 commits. Jalons :

| Commit | Date | Effet |
|---|---|---|
| `ebed38a`, `daa8d67` | 2026-06-01 | premiers domaines, stores, trait `Transfer` |
| `cfc27bc` | 2026-06-02 | `TypedStateStore`, contexte inter-domaines (ADR-008) |
| `be0e97b` | 2026-06-03 | découpage de `state_value.rs` (domaine vs transfert) |
| `4c106d7` | 2026-06-03 | tout l'état dans un objet de contexte (`AnalysisCtx`) |
| `bcffcf7` | 2026-06-04 | analyse inter-composants (ADR-012) → `InterCtx` |
| `f937160` | 2026-06-28 | widening à seuils (ADR-014) |
| `c32e1b0` | 2026-07-14 | produit ponctuel (ADR-015) |
| `f31acae` | 2026-07-15 | stabilité versionnée + churn objet (ADR-017) |
| `29a6709` | 2026-07-22 | bit `is_int` (FN `1.7 < 2`) |
| `61d7a0b`, `c887c52` | 2026-07-22/23 | `flat_lattice!`, `KindMask` (ADR-020) |
| `806d114` | 2026-09-05 | `ComponentId` interné (#7) dans `Versioned` et `SetterVal` |
| `548f922` | 2026-09-27 | `rem`, `pow`, `In`, `InstanceOf` (#73) |
| `e67b10a` | 2026-09-27 | `Expr::New` ⇒ référence fraîche dans `from_init` (#158) |

---

## 6. Exemples concrets

### 6.0 Protocole

- Fichiers d'exemple dans `/tmp/dom04/ex*.tsx`, analysés par
  `cargo run -q -- check /tmp/dom04/exN.tsx --no-color --show-clean [--trace] [--info]`.
- **Sonde** : un crate temporaire `/tmp/dom04/probe` dépendant du dépôt par
  chemin (`reactant = { path = "…", default-features = false }`, cible
  `CARGO_TARGET_DIR=/tmp/dom04/target`). Sans argument, il exécute des
  opérations de domaine ; avec un fichier, il fait `lower_program` +
  `analyze_component(comp, &StateValueTransfer, &Config::default())` (la même
  chaîne que `tests/widening_e2e.rs`) et imprime `state_store.get(l)` pour
  chaque label, `widen_trace.contains_key(l)`, et `memo_store.get(label)` pour
  chaque hook `Memo`/`Callback`. Les identités apparaissent comme
  `ComponentId(4294967295)` = `ComponentId::SYNTHETIC` (analyse intra isolée).
- Rien n'a été écrit dans le dépôt.

Sorties de la sonde sur les opérations de domaine (verbatim) :

```
== Interval edge cases
[0,0] * [-inf,+inf] = Interval { lo: inf, hi: -inf, is_int: false } is_bottom=true
[0,0] * [0,+inf] = Interval { lo: 0.0, hi: 0.0, is_int: true }
[10,20].narrow_lt(5) = Interval { lo: 10.0, hi: 4.0, is_int: true } is_bottom=true
non-canonical bottom == Interval::bottom() ? false
partial_cmp(non-canonical ⊥, [0,3]) = None
partial_cmp(Interval::bottom(), [0,3]) = Some(Less)
rem [0,+inf] % [3,3] = Some(Interval { lo: 0.0, hi: 2.0, is_int: true })
rem [0,9] % [0,3] = None
pow [2,3] ** [2,2] = Some(Interval { lo: 4.0, hi: 9.0, is_int: true })
widen_to([0,5],[0,6],[10]) = Interval { lo: 0.0, hi: 10.0, is_int: true }
widen([0,0],[0,1]) = Interval { lo: 0.0, hi: inf, is_int: true }
is_int ignored by ==: true
1.7 narrow_lt 2 = Interval { lo: 1.7, hi: 1.7, is_int: false }
[0,9] int narrow_lt 4.5 = Interval { lo: 0.0, hi: 4.0, is_int: true }
== StateValue
null ⊔ 1 = number[1, 1]|null; to_stability = Unknown
widen -> number[1, inf]|null; to_stability = PerRender; is_unbounded=true
10 ⊔ "test" = number[10, 10]|string{"test"}
fresh ref: ref(PerRender) unstable_ref_only=true
top = ⊤; bottom = ⊥
boolean ⊔ [0,5] to_stability = PerRender (motion wins over Unknown)
setter: setter(#4294967295#1) to_stability=Stable typeof=Some("function")
typeof null = Some("object")
Versioned ⊔ Stable = Versioned({(ComponentId(4294967295), 0)})
Versioned ⊔ PerRender = Unknown
top-with-versioned-ref to_stability = Unknown; versioned_reference = Some(Versioned({(ComponentId(4294967295), 0)}))
```

### 6.1 Compteur non gardé vs compteur gardé (intervalles, seuils, narrowing)

```tsx
import { useState, useEffect } from "react";

export function Unbounded() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    setCount(count + 1);
  }, [count]);
  return <div>{count}</div>;
}

export function Guarded() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    if (count < 10) setCount(count + 1);
  }, [count]);
  return <div>{count}</div>;
}
```
(`/tmp/dom04/ex1.tsx`, repris de `tests/fixtures/widening.tsx`
`UnboundedCounter`/`GuardedCounter`)

Ce que fait le domaine :

- Amorçage : `count ↦ number[0, 0]` (`Interval::point(0.0)`, `is_int = true`).
- `Unbounded` : l'effet écrit `count + 1` ⇒ `as_arith` ⇒ `[0,0] + [1,1] =
  [1,1]`, puis `[0,1]`, `[0,2]`… À l'itération 3 (`widen_threshold = 3`),
  `widen_to` avec seuils {0, 1} : la borne haute dépasse tous les seuils ⇒
  `+∞`. `is_unbounded` vrai.
- `Guarded` : la garde `count < 10` (Var à gauche, littéral à droite) fait
  `narrow_lt(10)` avec `is_int` ⇒ `hi ≤ 9`, l'écriture est bornée à `[1,10]` ;
  le widening saute au seuil **10** au lieu de `+∞` ⇒ `[0,10]`.

Sonde :

```
-- Unbounded
   state[0] = number[0, inf]   widened=true
-- Guarded
   state[0] = number[0, 10]   widened=true
```

CLI (`--show-clean`, puis `--trace --info` pour la ligne `widening-info`) :

```
  Guarded  (2 hooks)  /tmp/dom04/ex1.tsx  ✓
  Unbounded  (2 hooks)  /tmp/dom04/ex1.tsx
    warn   infinite-loop  [hook:0]  (line 5:2)  this effect keeps pushing state `count` (its deps do not provably gate it, so the effect can re-run every render) to new values on every run. Potential infinite render loop
       (2 trace step(s), rerun with --trace)

⚠  1 warning(s) across 1 file(s).
```

```
  Guarded  (2 hooks)  /tmp/dom04/ex1.tsx
    info   widening-info  (line 13:2)  state `count` kept changing during analysis and was approximated to converge, so findings that depend on it may be imprecise
       → state `count` is written here [hook:1] (line 13:2)
       → the abstract value of state `count` kept growing and was widened at iteration 3
```

La valeur `[0,10]` est aussi épinglée par
`tests/widening_e2e.rs::guarded_counter_converges_bounded`. Le widening à
seuils (ADR-014) a eu lieu (`widened=true`) mais à une borne finie ;
`infinite-loop` ne tire pas parce que la garde n°2 (`is_unbounded`) l'arrête.

### 6.2 Produit : `null ∪ number`, véracité

```tsx
export function NullCounter() {
  const [n, setN] = useState(null);
  useEffect(() => {
    setN(n + 1);
  }, [n]);
  return <div>{n}</div>;
}

export function TruthyFromZero() {
  const [n, setN] = useState(0);
  useEffect(() => {
    if (n) setN(n + 1);
  }, [n]);
  return <div>{n}</div>;
}

export function TruthyFromOne() {
  const [n, setN] = useState(1);
  useEffect(() => {
    if (n) setN(n + 1);
  }, [n]);
  return <div>{n}</div>;
}

export function FetchOnce() {
  const [user, setUser] = useState(null);
  useEffect(() => {
    if (!user) setUser({ name: "guest" });
  }, [user]);
  return <div>{user ? "x" : "y"}</div>;
}
```
(`/tmp/dom04/ex2.tsx`, en-tête `import { useState, useEffect } from "react";` ;
cas repris de `tests/narrowing.rs`)

Sonde :

```
-- NullCounter
   state[0] = number[1, inf]|null   widened=true
-- TruthyFromZero
   state[0] = number[0, 0]   widened=false
-- TruthyFromOne
   state[0] = number[1, inf]   widened=true
-- FetchOnce
   state[0] = ref(PerRender)|null   widened=false
```

Lecture :

- `NullCounter` : `n = {null}` ; `n + 1` ⇒ `as_arith` voit {null} ⇒
  `[0,0]` (ToNumber(null) = 0) ⇒ écrit `[1,1]` ; le store devient
  `number[1,1]|null` puis s'élargit sur `num` seul ⇒ `number[1, inf]|null`. Le
  FN d'ADR-008 est inversé en détection (test
  `null_init_without_hint_unbounded_is_flagged`).
- `TruthyFromZero` : `narrow_truthy` sur `[0,0]` ⇒ `narrow_neq(0)` ⇒ ⊥ ;
  `n + 1` avec un opérande ⊥ ⇒ `add` rend ⊥ ⇒ l'écriture est un no-op. Pas de
  boucle (test `truthy_guarded_increment_from_zero_never_starts`).
- `TruthyFromOne` : `[1,1]` survit à `narrow_truthy` ⇒ croissance ⇒ widening.
- `FetchOnce` : au 2e tour `user = ref(Versioned)|null` ; la branche `if
  (!user)` applique `narrow_falsy` qui tue `reference` ⇒ seul `null` reste ⇒
  l'écriture `{ name: "guest" }` se fait une fois ; le store se stabilise sur
  `ref(PerRender)|null` (valeurs *écrites*). Test
  `negated_truthy_fetch_pattern_no_false_positive`.

CLI :

```
  FetchOnce  (2 hooks)  /tmp/dom04/ex2.tsx  ✓
  NullCounter  (2 hooks)  /tmp/dom04/ex2.tsx
    warn   infinite-loop  [hook:0]  (line 5:2)  this effect keeps pushing state `n` (its deps do not provably gate it, so the effect can re-run every render) to new values on every run. Potential infinite render loop
       → state `n` is written here [hook:1] (line 5:2)
       → the abstract value of state `n` kept growing and was widened at iteration 3
  TruthyFromOne  (2 hooks)  /tmp/dom04/ex2.tsx
    warn   infinite-loop  [hook:0]  (line 21:2)  this effect keeps pushing state `n` (its deps do not provably gate it, so the effect can re-run every render) to new values on every run. Potential infinite render loop
       → state `n` is written here [hook:1] (line 21:2)
       → the abstract value of state `n` kept growing and was widened at iteration 3
  TruthyFromZero  (2 hooks)  /tmp/dom04/ex2.tsx
    warn   redundant-set-state  [hook:0]  (line 13:2)  state `n` is set to a stable value it already holds, so the update is redundant
       → the value written to state `n` is the value it already holds [hook:0] (line 14:11)

⚠  3 warning(s) across 1 file(s).
```

**Surprise observée** sur `TruthyFromZero` : `redundant-set-state` tire (Warning)
sur une écriture **morte**. Cause (lecture de
`src/rules/impls/redundant_set_state.rs:262-269`) : la règle ré-évalue
l'argument `n + 1` dans l'env et le store convergés **sans** le raffinement de
la branche, obtient `[1,1]`, et teste `arg_val.is_stable() &&
current_val.is_stable()` — deux *points* (`[1,1]` et `[0,0]`) sont « stables »
au sens de `to_stability`, alors que les valeurs diffèrent. « Stable » signifie
« ne change pas d'un rendu à l'autre », pas « égal à la valeur courante ».
FP (Warning, tolérée par l'invariant) hors du périmètre strict mais qui
illustre un piège de lecture de `is_stable` (§8).

### 6.3 Treillis finis : chaînes, modulo, booléens — et un faux négatif

```tsx
export function ModCycle() {
  const [i, setI] = useState(0);
  useEffect(() => {
    setI((i + 1) % 3);
  }, [i]);
  return <div>{i}</div>;
}

export function Toggle() {
  const [on, setOn] = useState(false);
  useEffect(() => {
    setOn(!on);
  }, [on]);
  return <div>{on ? 1 : 0}</div>;
}

export function StrToggle() {
  const [mode, setMode] = useState("a");
  useEffect(() => {
    setMode(mode === "a" ? "b" : "a");
  }, [mode]);
  return <div>{mode}</div>;
}

export function ModByLength({ items }: { items: string[] }) {
  const [i, setI] = useState(0);
  useEffect(() => {
    setI((i + 1) % items.length);
  }, [i]);
  return <div>{i}</div>;
}
```
(`/tmp/dom04/ex5.tsx`)

Sonde :

```
-- ModCycle
   state[0] = number[0, 2]   widened=false
-- Toggle
   state[0] = boolean   widened=false
-- StrToggle
   state[0] = string{"a", "b"}   widened=false
-- ModByLength
   state[0] = ⊤   widened=false
```

Lecture domaine : `rem([1,1] ∪ …, [3,3])` borne `i` à `[0,2]` (§4.2) ;
`!on` inverse `False` puis la jointure donne `BoolVal::Top` ; les deux
chaînes forment `{"a","b"}` (sous le seuil 4) ; `i % items.length` a un
diviseur ⊤ ⇒ `as_arith` échoue ⇒ ⊤. Les trois premiers convergent **sans
widening** : leurs treillis sont de hauteur finie.

CLI (`--show-clean --trace --info`, lignes `verified` élaguées sauf une) :

```
  ModByLength  (2 hooks)  /tmp/dom04/ex5.tsx
    warn   infinite-loop  [hook:0]  (line 29:2)  this effect may store a fresh reference into state `i` which its deps react to: possible infinite render loop
       → a fresh value is written to state `i` here [hook:1] (line 30:4)
    warn   missing-deps  [hook:1]  var:items  (line 29:2)  `items` is used in this effect but not in its deps array, and its value may change between renders
       → `items` is read here [hook:1] (line 29:2)
  ModCycle  (2 hooks)  /tmp/dom04/ex5.tsx  ✓
  StrToggle  (2 hooks)  /tmp/dom04/ex5.tsx  ✓
  Toggle  (2 hooks)  /tmp/dom04/ex5.tsx  ✓
```

et, pour `ModCycle` comme pour `StrToggle` et `Toggle` :

```
    verified  infinite-loop  no effect diverges into an infinite render loop
```

**Faux négatif observé (à confirmer par le mainteneur)** : `ModCycle`
(0→1→2→0…), `Toggle` (false→true→false…) et `StrToggle` ("a"→"b"→"a"…) sont
de vraies boucles de rendu infinies (chaque écriture change la valeur au sens
d'`Object.is`, la dep change, l'effet se ré-exécute), et l'analyseur les
déclare « verified ». Cause : le bras point fixe d'`infinite-loop` exige
`widen_trace` (le slot a été élargi) puis `is_unbounded`
(`src/rules/impls/infinite_loop.rs:130-136`), et le graphe de churn ne
s'intéresse qu'à la sorte référence (`src/engine/written.rs:173-175`). Or le
domaine abstrait des **ensembles de valeurs atteignables** : un cycle à travers
un nombre fini de valeurs a un point fixe abstrait fini, et l'information
« la valeur change à chaque exécution » (une propriété de *transition*) n'y est
pas représentable. `ModByLength` n'est signalé que grâce à l'imprécision (⊤ ⇒
référence `Unknown` ⇒ « Maybe fresh »). Aucune issue trouvée
(`gh issue list --state all --search "oscillat"` / `"toggle"` : vide) ;
`docs/limitations.md` ne le mentionne pas ; ADR-008 affirmait au contraire
« Boolean ✓ oscillation true↔false ». À vérifier : pas de test du dépôt
n'épingle ces motifs (recherche dans `tests/`).

Complément, seuil des chaînes (`/tmp/dom04/ex7.tsx`, `ManyStrings`, cinq
constantes "a"…"e" écrites par des handlers) : la sonde donne
`state[0] = string` (i.e. `StrConst::Top`), le seuil `|S| > 4` a joué.

### 6.4 Stabilité référentielle : littéral, `useMemo`, `useCallback`, état objet

```tsx
import { useState, useEffect, useMemo, useCallback } from "react";

export function InlineObjectDep() {
  const opts = { a: 1 };
  useEffect(() => { console.log(opts); }, [opts]);
  return <div />;
}

export function MemoizedDep() {
  const opts = useMemo(() => ({ a: 1 }), []);
  useEffect(() => { console.log(opts); }, [opts]);
  return <div />;
}

export function CallbackDep({ id }: { id: string }) {
  const [q, setQ] = useState({ id: "x" });
  const onLoad = useCallback(() => console.log(q), [q]);
  useEffect(() => { onLoad(); }, [onLoad]);
  return <button onClick={() => setQ({ id })}>go</button>;
}

export function ObjectStateDep() {
  const [ctx, setCtx] = useState({ locale: "en" });
  useEffect(() => { console.log(ctx); }, [ctx]);
  return <button onClick={() => setCtx({ locale: "fr" })}>fr</button>;
}
```
(`/tmp/dom04/ex3.tsx`, premières composantes)

Sonde :

```
-- InlineObjectDep
-- MemoizedDep
   memo[0] (Memo) = ref(Stable)
-- CallbackDep
   state[0] = ref(PerRender)   widened=false
   memo[1] (Callback) = ref(Versioned({(ComponentId(4294967295), 0)}))
-- ObjectStateDep
   state[0] = ref(PerRender)   widened=false
```

Lecture :

- `InlineObjectDep` : `{ a: 1 }` ⇒ `reference(PerRender)` (fait **must**) ;
  `opts` en dep ⇒ `is_unstable_reference_only` ⇒ `always-unstable-deps`.
- `MemoizedDep` : deps `[]` (`Arity::Exact(0)`) ⇒ `recompute_memo` rend
  `ref(Stable)` ; silence.
- `CallbackDep` : store de `q` = `ref(PerRender)` (vue événement : on n'y
  écrit que des littéraux frais) ; la lecture `q` dans les deps du
  `useCallback` passe par la conversion `Versioned({(C,0)})` ; le
  `useCallback` vaut donc `ref(Versioned({(C,0)}))` ⇒ dep de l'effet
  « change seulement quand `q` est set » ⇒ silence (c'est précisément ADR-017).
- `ObjectStateDep` : le FP F5 d'ADR-017 ; la lecture de `ctx` est
  `Versioned`, donc `always-unstable-deps` se tait.

CLI :

```
  CallbackDep  (4 hooks)  /tmp/dom04/ex3.tsx  ✓
  InlineObjectDep  (1 hooks)  /tmp/dom04/ex3.tsx
    warn   always-unstable-deps  [hook:0]  (line 5:2)  this effect depends on `opts`, a new reference every render, so `Object.is` always differs and the effect re-runs on every render regardless of the other deps
       → the value flows through `opts`, bound here (line 4:8)
  MemoizedDep  (2 hooks)  /tmp/dom04/ex3.tsx  ✓
  …
  ObjectStateDep  (3 hooks)  /tmp/dom04/ex3.tsx  ✓
```

(Ligne `ObjChurn` élaguée ici, voir §6.5.)

### 6.5 Churn d'objet : l'Error certifiée d'ADR-017

```tsx
export function ObjChurn() {
  const [obj, setObj] = useState({ a: 1 });
  useEffect(() => { setObj({ ...obj, b: 2 }); }, [obj]);
  return <div />;
}
```
(`/tmp/dom04/ex3.tsx:28-32`, exemple d'ADR-017)

Domaine : store `ref(PerRender)` ; lecture `obj` ⇒ `Versioned({(C,0)})` ; la
valeur écrite `{ ...obj, b: 2 }` est un `ObjectLit` ⇒ `PerRender` ⇒
`value_freshness = Fresh`. Rien ne s'élargit (`join(PerRender, PerRender) =
PerRender`, sonde : `widened=false`) : le bras point fixe ne peut rien voir, et
c'est le bras churn (dep = slot exact ∧ set sur tous les chemins ∧ valeur
must-fresh) qui certifie l'**Error** :

```
  ObjChurn  (2 hooks)  /tmp/dom04/ex3.tsx
    error  infinite-loop  [hook:0]  (line 30:2)  this effect recreates object state `obj` it depends on. Every run stores a fresh reference (`Object.is` always fails) and re-triggers itself: infinite render loop
       → a fresh value is written to state `obj` here [hook:1] (line 30:20)
```

Contraste avec §6.3 : pour une référence, le domaine *conserve* un fait de
transition (« fraîche à chaque écriture », `PerRender`), ce qu'il ne fait pas
pour un booléen ou un petit intervalle.

### 6.6 Mémo sur un état numérique : un FP de projection

```tsx
import { useEffect, useMemo, useState } from "react";

export function MemoOverNumState() {
  const [n, setN] = useState(0);
  const doubled = useMemo(() => n * 2, [n]);
  useEffect(() => { console.log(doubled); }, [doubled]);
  return <button onClick={() => setN(5)}>{doubled}</button>;
}

export function MemoOverObjState() {
  const [o, setO] = useState({ a: 1 });
  const a = useMemo(() => ({ v: o.a }), [o]);
  useEffect(() => { console.log(a); }, [a]);
  return <button onClick={() => setO({ a: 2 })}>{a.v}</button>;
}
```
(`/tmp/dom04/ex8.tsx`)

Sonde :

```
-- MemoOverNumState
   state[0] = number[0, 5]   widened=false
   memo[1] (Memo) = ref(PerRender)
-- MemoOverObjState
   state[0] = ref(PerRender)   widened=false
   memo[1] (Memo) = ref(Versioned({(ComponentId(4294967295), 0)}))
```

CLI :

```
  MemoOverNumState  (4 hooks)  /tmp/dom04/ex8.tsx
    warn   always-unstable-deps  [hook:2]  (line 6:2)  this effect depends on `doubled`, a new reference every render, so `Object.is` always differs and the effect re-runs on every render regardless of the other deps
       → the value flows through `doubled`, bound here (line 5:8)
  MemoOverObjState  (4 hooks)  /tmp/dom04/ex8.tsx  ✓
```

**FP observé** (Warning, message faux : `doubled` est un nombre recalculé
seulement quand `n` change). Chaîne causale, lue dans le code :
`recompute_memo` ne projette `Versioned({l})` que si la dep **est
syntaxiquement** `Expr::StateVal(l)` (`transfer/state_value.rs:86-88`) ; ici
la dep est la variable `n` (l'IR la lit comme `Var`), donc elle est évaluée
normalement : `number[0,5]`, emplacement `reference` à ⊥ ⇒
`versioned_reference() = None` ⇒ `to_stability()` ⇒ intervalle non ponctuel ⇒
`PerRender` (au sens *may* de motion-wins) ⇒ la valeur du mémo, logée dans
l'emplacement `reference`, devient `ref(PerRender)`, que
`is_unstable_reference_only` lit comme un fait **must**. Le glissement de sens
de `PerRender` (§4.5) passe ainsi d'une projection *may* à un fait *must*.
Avec un état objet (`MemoOverObjState`), la lecture de `o` porte déjà
`Versioned` dans son emplacement référence : aucun problème. Même phénomène
pour `useMemo(() => n * 2, [n])` avec un handler `setN(n + 1)`
(`/tmp/dom04/ex7.tsx`, `MemoOverState` : `memo[1] = ref(PerRender)`,
warning identique). Aucune issue correspondante trouvée — à signaler.

### 6.7 Cas limite `0 × [−∞, +∞]` : un ⊥ inattendu

```tsx
export function ZeroTimesUnbounded() {
  const [n, setN] = useState(0);
  const [k, setK] = useState(0);
  const z = n * 0;
  useEffect(() => {
    setK(k + 1 + z);
  }, [k]);
  return (
    <div>
      <button onClick={() => setN(n + 1)}>+</button>
      <button onClick={() => setN(n - 1)}>-</button>
      {k}
    </div>
  );
}
```
(`/tmp/dom04/ex6.tsx` ; le témoin `ZeroTimesControl` est identique avec
`const z = 0;`)

Sonde : `n` s'élargit dans les deux sens (`state[0] = number`, c.-à-d.
`[−∞, +∞]`) sous l'effet des deux handlers. Alors `z = n * 0` est calculé par
`Interval::mul` : quatre coins `NaN` ⇒ **⊥** (§4.2). CLI :

```
  ZeroTimesUnbounded  (5 hooks)  /tmp/dom04/ex6.tsx
    warn   infinite-loop  [hook:1]  (line 7:2)  this effect keeps pushing state `k` (its deps do not provably gate it, so the effect can re-run every render) to new values on every run. Potential infinite render loop
       → state `k` is written here [hook:2] (line 7:2)
       → the abstract value of state `k` kept growing and was widened at iteration 3
    warn   missing-deps  [hook:2]  var:z  (line 7:2)  `z` is used in this effect but not in its deps array, and its value never changes between renders
       → `z` is read here [hook:2] (line 7:2)
```

Le témoin `ZeroTimesControl` n'a que l'`infinite-loop` (pas de
`missing-deps`). La ligne `missing-deps` sur `z`, au libellé contradictoire
(« never changes » est la formulation commune de `describe_value` pour
`Stability::Bottom | Stability::Stable`, `src/rules/helpers/mod.rs:45` ;
la règle, elle, se tait sur `is_stable()`, c'est-à-dire
`to_stability() == Stable` — un ⊥ ne la fait pas taire, mais s'affiche
comme un `Stable`), trahit que `z` est évalué ⊥. Ici la conséquence est un
FP ; mais ⊥ signifie « inatteignable », et un ⊥ issu d'une valeur réellement
atteinte (`z = 0`) est une sous-approximation : toute écriture dont la valeur
dépend de `z` devient un no-op abstrait (dans cet exemple, la boucle sur `k`
reste détectée parce que `k` s'élargit dès les premières itérations, avant que
`n` n'atteigne `[−∞, +∞]` — explication plausible, **à vérifier**).

Détail de la sonde, rejoué le 2026-09-28 :

```
-- ZeroTimesUnbounded
   state[0] = number   widened=false  num=Interval { lo: -inf, hi: inf, is_int: true }
   state[1] = number[0, inf]   widened=true  num=Interval { lo: 0.0, hi: inf, is_int: true }
-- ZeroTimesControl
   state[0] = number   widened=false  num=Interval { lo: -inf, hi: inf, is_int: true }
   state[1] = number[0, inf]   widened=true  num=Interval { lo: 0.0, hi: inf, is_int: true }
```

Noter `widened=false` pour `n` alors que sa valeur est `[−∞, +∞]` : les
écritures des **handlers** s'élargissent sans entrer dans `widen_trace`
(commentaire du moteur « render+effects only handler widening is not a
bug », `src/engine/fixpoint.rs:515` ; le mécanisme exact du point fixe des
handlers est **à vérifier** dans le dossier moteur). Même constat pour
`MemoOverState` (§6.6 : `state[0] = number[0, inf] widened=false`).

### 6.8 Croissance non bornée d'une chaîne, compteur masqué : deux FN du prédicat `is_unbounded`

Fichier `/tmp/dom04v/ex10.tsx` (créé pour cette vérification) :

```tsx
import { useState, useEffect } from "react";

export function StrGrow() {
  const [s, setS] = useState("a");
  useEffect(() => {
    setS(s + "x");
  }, [s]);
  return <div>{s}</div>;
}

export function BitCounter() {
  const [f, setF] = useState(0);
  useEffect(() => {
    setF((f + 1) & 7);
  }, [f]);
  return <div>{f}</div>;
}

export function TypeofGuard() {
  const [v, setV] = useState(null);
  useEffect(() => {
    if (typeof v !== "number") setV(1);
  }, [v]);
  return <div>{v}</div>;
}
```

Sonde :

```
-- StrGrow
   state[0] = string   widened=true  num=Interval { lo: inf, hi: -inf, is_int: true }
-- BitCounter
   state[0] = number[0, 7]   widened=false  num=Interval { lo: 0.0, hi: 7.0, is_int: true }
-- TypeofGuard
   state[0] = number[1, 1]|null   widened=false  num=Interval { lo: 1.0, hi: 1.0, is_int: true }
```

CLI (`--show-clean --trace --info`, lignes `verified` élaguées sauf
`infinite-loop` de `StrGrow`) :

```
  BitCounter  (2 hooks)  ex10.tsx  ✓
  StrGrow  (2 hooks)  ex10.tsx
    info   widening-info  (line 5:2)  state `s` kept changing during analysis and was approximated to converge, so findings that depend on it may be imprecise
       → state `s` is written here [hook:1] (line 5:2)
       → the abstract value of state `s` kept growing and was widened at iteration 3
    verified  infinite-loop  no effect diverges into an infinite render loop
  TypeofGuard  (2 hooks)  ex10.tsx  ✓
```

Lecture :

- `StrGrow` : `s + "x"` sur des chaînes pures fait le produit de
  concaténation (`eval_binop`, §4.2) : `{"a"}`, `{"a","ax"}`, … jusqu'à
  dépasser 4 constantes ⇒ `StrConst::Top`. Le slot **est** dans
  `widen_trace` (`widened=true`), mais `is_unbounded` (§4.7) ne regarde que
  `num` (borne infinie), `reference == PerRender` et `other` : un
  `str: Top` n'est pas « non borné ». La garde n°2 du bras point fixe
  (`infinite_loop.rs:134`) coupe donc, et le churn ne concerne que les
  références. Or la boucle concrète est réelle et infinie (`"a"`, `"ax"`,
  `"axx"`, … : chaque écriture change la valeur au sens d'`Object.is`).
  **Faux négatif observé** (un « verified » sur une vraie boucle), non
  documenté dans `docs/limitations.md` ni dans une issue trouvée
  (`gh issue list --state all --search "string unbounded"` : rien de
  pertinent) — **à confirmer par le mainteneur**. La cause est dans le
  périmètre : `is_unbounded` ne traite pas l'emplacement `str`.
- `BitCounter` : `(f + 1) & 7` est borné par le masque constant
  (`eval_bitwise`, `[0, 7]`) ; la convergence est finie, sans widening —
  même famille que `ModCycle` (§6.3, cycle fini non vu).
- `TypeofGuard` : silence **correct** (la boucle concrète s'arrête :
  `v = 1` puis la garde est fausse). Il illustre que `typeof v !== "number"`
  n'est pas raffiné par `narrow_env_for_branch` (§4.4) : l'écriture `1` a lieu
  sur une branche non raffinée, sans conséquence ici.

---

## 7. Contexte React nécessaire

Pour comprendre ce sous-système, le lecteur doit connaître :

1. **Rendu et commit.** Un composant est une fonction appelée à chaque rendu ;
   les effets (`useEffect`) s'exécutent après le commit. La trace de
   changement de `Stability` est indexée par ces rendus successifs r₀, r₁, …
   (le moteur itère render → mémos → effets → handlers).
2. **`useState` et le bail-out `Object.is`.** Un setter appelé avec une valeur
   `Object.is`-égale à la valeur courante n'entraîne pas de nouveau rendu.
   C'est pourquoi un **primitif** n'est pas « frais » (il est comparé par
   valeur) alors qu'un littéral objet l'est ; et c'est l'hypothèse de
   `is_finitely_valued` (« re-renders at the rate of its transitions »).
3. **Identité du setter** : React garantit que la fonction `setX` est la même
   à chaque rendu ⇒ `SetterVal` non-⊥ ⇒ `Stable`. De même `useRef()` rend le
   même conteneur ⇒ `reference(Stable)` (`MarkerVal::StableRef`).
4. **Un slot d'état ne change que par son setter** (le fait qui fonde
   ADR-017) ; en dehors du rendu (sinon `setter-in-render`).
5. **Tableaux de deps** : `useEffect`/`useMemo`/`useCallback` comparent
   chaque dep à la précédente avec `Object.is` et se ré-exécutent si **au
   moins une** diffère (sémantique OU — d'où « one stable dep gates
   nothing », `infinite_loop.rs:101-105`) ; `[]` ⇒ une seule fois ; pas de
   tableau ⇒ à chaque rendu. D'où le traitement de `recompute_memo` :
   `[]` connu ⇒ `Stable`, tableau illisible ⇒ `Unknown`.
6. **Stabilité référentielle** : `{}`, `[]`, `() => {}`, `new X()`,
   `arr.map(…)` créent une nouvelle référence à chaque évaluation ;
   `useMemo`/`useCallback` la stabilisent jusqu'au changement des deps.
7. **Sémantique JavaScript** utilisée par les transferts : disjonction des
   sortes (`typeof`), `ToNumber(null) = 0`, `ToNumber(undefined) = NaN`,
   `typeof null === "object"`, valeurs fausses (`null`, `undefined`, `0`,
   `-0`, `NaN`, `""`, `false`, `0n`), `%` du signe du dividende, `x/0`,
   `0**0 = 1`, coercition int32/uint32 des opérateurs bit à bit.
8. **Batching / updaters fonctionnels** : non modélisés dans ce périmètre ;
   le store accumule toutes les écritures par `join` (weak update), ce qui
   sur-approxime tout ordre de batching. Les updaters `setX(prev => …)` sont
   traités par l'interpréteur (hors périmètre ; `written.rs::returns_value`
   utilise `from_init` sur les `Return` de l'updater).
9. **Strict Mode, Server Components, Context** : sans effet direct sur les
   domaines. `useContext` n'est pas modélisé (⊤, #28).
10. **`Object.is` précisément** : comme `===` sauf `Object.is(NaN, NaN) =
    true` et `Object.is(0, -0) = false`. Le domaine n'a ni `NaN` (§3.5) ni
    distinction `0`/`-0` (`-0.0 == 0.0` en Rust, donc `narrow_neq(0)` tue
    aussi `-0`) : ce sont des pertes de précision *dans les deux sens* qui
    n'ont pas d'effet sur les règles actuelles (la stabilité d'un nombre est
    lue sur la *forme* de l'intervalle, point ou non, §4.5).
11. **Initialiseur paresseux** `useState(() => expr)` : React exécute la
    fonction une seule fois au montage et stocke son *résultat* ; le moteur
    exécute donc le corps (`interp::exec_body`) au lieu d'abstraire la
    fermeture en référence fraîche (`fixpoint.rs:336-347`, FP corpus « F2 »).
12. **Limite de profondeur de React** : React lève « Maximum update depth
    exceeded » après ~50 mises à jour imbriquées synchrones (rappelé par
    #144 : « React throws after 50 nested updates anyway »). Une « boucle
    infinie » au sens du diagnostic est donc, concrètement, un plantage ou
    une boucle de rendus/effets qui ne s'arrête pas ; le domaine n'a pas à
    modéliser ce plafond.
13. **Une valeur primitive écrite deux fois à l'identique ne re-rend pas** :
    c'est pourquoi `redundant-set-state` et `is_finitely_valued` raisonnent
    sur les *valeurs*, et pourquoi le churn (`value_freshness`) ne regarde
    que la sorte référence (`written.rs:173-175`).

Sémantique concrète de référence : **ADR-001**, React-tRace (règles
`StepInit → StepEffect → StepCheck`, `SttReBind`, `CheckEffect`,
`CheckNoEffect`) ; les extensions (deps, mémos, refs, objets) devaient être
spécifiées dans `docs/semantics.md`, **absent** (§5.7).

---

## 8. Subtilités, pièges, limites

1. **`PerRender` a deux sens.** Must dans l'emplacement `reference`
   (allocation fraîche garantie) ; may dans le résultat de `to_stability`
   (« peut changer à chaque rendu », y compris pour un nombre). Mélanger les
   deux produit le FP de §6.6. Utiliser `is_unstable_reference_only` pour le
   fait must, `StabilityVerdict`/`may_change_of` pour le fait may.
2. **`NaN` n'est dans aucun intervalle** : `/` ⇒ ⊤, `%` par diviseur
   pouvant être nul ⇒ ⊤, `**` hors du quadrant positif ⇒ ⊤ (limitations.md,
   #73). Mais `mul` perd `NaN` dans son repli `f64::min`/`max` : `[0,0] ×
   [−∞,+∞] = ⊥` (sonde, §6.7) — sous-approximation.
3. **`±Infinity` comme valeur concrète** : `+∞` **peut** être une valeur.
   Vérifié le 2026-09-28 (`/tmp/dom04v/ex11.tsx`, `useState(1e999)` puis
   `setN(n % 3)` dans un effet `[n]`) : la sonde donne
   `state[0] = number[0, inf] … is_int: false`, c'est-à-dire que l'init est
   bien `point(inf)` et que `rem([inf,inf], [3,3])` a rendu `[0,3]`, alors
   que `Infinity % 3` est `NaN` en JS : la valeur concrète `NaN` n'est pas
   couverte (sous-approximation de valeur ; ici sans effet sur les
   diagnostics — concrètement `NaN` est `Object.is`-égal à lui-même et la
   boucle s'arrête, et l'analyseur ne dit rien). De même
   `add([−∞,−∞],[+∞,+∞])` produirait `NaN` dans les bornes. Le code traite
   de fait `±∞` comme « non borné », pas comme une valeur ; #144 note que
   `n + 1 = n` pour `±Infinity`.
4. **⊥ non canonique d'`Interval`** : `[10, 4]` est ⊥ mais `!=
   Interval::bottom()` et incomparable avec `[0,3]` (sonde). Tous les
   combinateurs testent `is_bottom()` ; seules les comparaisons brutes
   (`==`, `partial_cmp`, donc `StateStore::leq`/`changed_labels`) sont
   exposées. Impact **à vérifier**.
5. **`is_int` ignoré par l'égalité et l'ordre.** Voulu (convergence
   indépendante du bit). Conséquence théorique : si un tour n'ajoute qu'un
   non-entier *à l'intérieur* des bornes, `leq` conclut à la convergence et
   le moteur garde l'ancien état (`is_int = true`) ; un narrowing ultérieur
   `x < 2.6` resserrerait alors à tort à `≤ 2`. Tentative de reproduction
   (`/tmp/dom04/ex9.tsx`, `HalfStep`) : **non reproduit** (la sonde montre
   `is_int: false` à la convergence et l'`infinite-loop` attendu est émis) ;
   reste **à vérifier** formellement. Sortie de la sonde rejouée le
   2026-09-28 :

   ```
   -- HalfStep
      state[0] = number[0, 10]   widened=false  num=Interval { lo: 0.0, hi: 10.0, is_int: false }
      state[1] = number[0, inf]   widened=true  num=Interval { lo: 0.0, hi: inf, is_int: true }
   -- HalfStepControl
      state[0] = number[9.5, 10]   widened=false  num=Interval { lo: 9.5, hi: 10.0, is_int: false }
      state[1] = number[0, inf]   widened=true  num=Interval { lo: 0.0, hi: inf, is_int: true }
   ```

   (Observation hors périmètre : la chaîne de témoins CLI de `HalfStep` cite
   « state `y` is written here » aux hooks 2 et 3, qui écrivent `x` et non
   `y` — relevant du dossier témoins/relations, **à vérifier**.)
6. **Bornes sentinelles de ⊥.** `Interval::bottom()` a des bornes infinies :
   tout prédicat qui lit `lo`/`hi` doit d'abord tester `is_bottom()`
   (`is_unbounded` le fait ; test `is_unbounded_requires_active_num_slot`).
7. **`is_stable` ≠ « égal ».** Deux points différents sont tous deux
   « stables » ; `redundant-set-state` en tire à tort « the value it already
   holds » sur une écriture morte (§6.2).
8. **Pas d'information de transition pour les primitifs.** Un cycle fini
   (booléen qui bascule, `(i+1) % k`, deux chaînes) converge abstraitement
   sans widening et n'est vu ni par le bras point fixe (`widen_trace` +
   `is_unbounded`) ni par le churn (références seulement) : FN observé
   (§6.3), non documenté.
9. **Valeur d'un mémo = stabilité seulement.** `MemoStore` range
   `reference(stabilité des deps)` ; la sorte réelle du mémo (nombre, objet)
   est perdue ; `useMemo(() => 1, [{}])` vaut `ref(PerRender)` et une dep sur
   lui tire `always-unstable-deps` (sonde et CLI, `/tmp/dom04/ex7.tsx`,
   `MemoPrimitiveOverFreshDep`).
10. **La projection `Versioned` de `recompute_memo` est syntaxique** (dep ==
    `Expr::StateVal`) ; une dep `Var` liée à un état numérique ne la reçoit
    pas (§6.6).
11. **Comparaisons jamais évaluées** : `Eq/Lt/…` rendent toujours
    `BoolVal::Top`, même sur deux constantes ; la précision vient uniquement
    de `narrow_env_for_branch`, qui ne reconnaît que `Var op Lit(num|null|undefined)`,
    `Var` et `!Var`.
12. **Mélanges de sortes ⇒ ⊤ en arithmétique** : `number + string` ⇒ ⊤ (et
    non une chaîne) ; `number | undefined` ⇒ ⊤ ; `boolean + 1` ⇒ ⊤
    (`as_arith` refuse les booléens ; seul `+x` unaire les convertit via
    `coerce_to_number`).
13. **`to_stability` d'une union de sortes ⇒ `Unknown`** même si chaque
    emplacement est un point (`null ⊔ 1`, sonde) ; motion-wins peut
    cependant « écraser » un `Unknown` en `PerRender` (`boolean ⊔ [0,5]`).
14. **`meet` inutilisé** et, pour `Stability`, plus petit que
    l'intersection des γ (`Versioned ⊓ PerRender = ⊥`). Ne pas s'en servir
    pour raffiner sans revoir ce point.
15. **Dépendance d'hypothèses d'ADR-017** : la lecture `Versioned` suppose
    qu'aucun set n'a lieu pendant le rendu (couvert par `setter-in-render`) et
    que `all_deps_*` n'accepte `Versioned` comme garde que tant que le bras
    churn existe. Et #157 : un `Versioned(S)` écrit dans un slot de S est lu
    « non frais » (FN connu).
16. **`useState(CONST)` jamais écrit** lit `Versioned`, pas `Stable` (#41,
    limitations.md) — la dep ne peut pas être omise.
17. **Constantes de module** `const X = f()` ⇒ ⊤ (#34) : pas d'encodage
    « sorte inconnue, identité stable » dans le produit.
18. **Documentation datée** : commentaire `Clone + Copy` du trait (`mod.rs:18-19`) ;
    doc de `NullCtx` (« returns `Top` ») et de `FixpointCtx` (« Local variables
    return `Bottom` ») ; ADR-007 (`state_value_of`, `AnalysisQueryCtx`) ;
    ADR-015/017 (`Symbol` au lieu de `ComponentId`) ; ADR-002 (fichiers
    inexistants, seuil 2) ; ADR-008 (« Boolean ✓ oscillation »).
19. **Protection `KindMask` partielle** : `as_arith`, `as_str_only`,
    `eval_unary(Not)` ré-énumèrent les emplacements sans `KindMask`.
20. **`from_state_value` par défaut ⊥** : correct seulement parce que
    `StateValue` est l'unique carrier (§3.1).
21. **Seuils récoltés largement** : tous les littéraux numériques des CFG
    (rendu, effets, handlers, init, `FnLit`) deviennent des seuils ; sûr,
    mais un littéral sans rapport peut « arrêter » un widening à une borne
    finie qui n'est pas la vraie (précision seulement, pas soundness : la
    borne sera dépassée au tour suivant et sautera au seuil suivant ou à ∞).
22. **Limites transverses** (docs/limitations.md) : `useContext` ⊤ (#28) ;
    spreads et clés calculées non modélisés (#76) ; `slice`/`concat` non
    prouvés frais faute de sorte du receveur (#22) ; setter prop typé primitif
    lu comme référence possiblement fraîche (#159).
23. **`is_unbounded` ignore l'emplacement `str`.** Une chaîne qui grandit
    (`setS(s + "x")`) passe à `StrConst::Top` et au `widen_trace`, mais
    `is_unbounded` est faux : `infinite-loop` déclare « verified » une vraie
    boucle infinie (§6.8, FN observé, **à confirmer**). Parmi les
    emplacements que `is_unbounded` ne lit pas (`boolean`, `str`, `null`,
    `undef`, `setter`), seul `str` a un ensemble concret infini : c'est le
    seul qui puisse grandir sans borne.
24. **Cycles finis masqués par les opérateurs bornants** : `& 7`, `% 3`
    bornent l'intervalle et le point fixe converge sans widening (§6.3,
    §6.8 `BitCounter`) — même famille que le point 8.
25. **`!⊥ = boolean(Top)`** (et non ⊥) dans `eval_unary(Not)` : un chemin
    mort redevient vivant pour la suite de l'évaluation (imprécis, sûr).
26. **Garde-fou des 100 itérations** : `state.widen(&new_state)` puis `break`
    sans nouveau test de post-point-fixe (§4.3) — **à vérifier** côté moteur.
27. **`from_init` n'amorce pas les slots** malgré son nom : l'amorçage passe
    par `eval_expr` (et `exec_body` pour un initialiseur paresseux) ;
    `from_init` ne sert qu'à `written.rs::returns_value` (§3.7).
28. **`SetterVal` et `StrConst` hors du ré-export `crate::domains`** : il
    faut les importer par `crate::domains::impls::…` (§2).

---

## 9. Glossaire

| Terme | Définition | Où |
|---|---|---|
| **domaine abstrait** | Treillis de valeurs abstraites muni de ⊥, ⊤, `join`, `meet`, `widen`, `widen_to` et des `narrow_*` | `trait AbstractDomain`, `src/domains/mod.rs:23-101` |
| **transfert** | Évaluation abstraite d'une expression / exécution d'une instruction | `trait Transfer`, `src/domains/mod.rs:114-162` ; `StateValueTransfer` |
| **emplacement (slot de produit)** | Un champ de `StateValue`, une par sorte JS | `state_value.rs:26-44` |
| **slot (d'état)** | Un `useState`/`useReducer` identifié par son `HookLabel` | `StateStore`, `src/domains/stores/state_store.rs:7-11` |
| **QualifiedSlot** | `(ComponentId, HookLabel)` : slot qualifié par son composant | `src/ir/types.rs:6-9` |
| **sorte (kind)** | Catégorie JS disjointe (nombre, booléen, chaîne, référence, null, undefined, setter, autre) | `KindMask`, `state_value.rs:46-78` |
| **produit ponctuel** | Opérations de treillis appliquées emplacement par emplacement, sans réduction | ADR-015 ; `state_value.rs:631-676` |
| **⊥ / bottom** | Aucune valeur possible (chemin inatteignable) | `bottom()` de chaque domaine |
| **⊤ / top** | Toute valeur possible ; pour `StateValue`, tous les emplacements ⊤ y compris `other` | `state_value.rs:512-523` |
| **résidu `other`** | Emplacement « tout le reste » (symbol, bigint, et de fait NaN) ; force `to_stability = Unknown` | `state_value.rs:41-43`, `:321-323` |
| **treillis plat** | ⊥ < éléments incomparables < ⊤ | `flat_lattice!`, `impls/mod.rs:1-53` |
| **k-ensemble / seuil de chaînes** | Ensemble de ≤ 4 constantes, au-delà ⊤ | `STR_WIDEN_THRESHOLD`, `str_const.rs:8` |
| **hull** | Plus petit intervalle contenant les deux (join des intervalles) | `interval.rs:69-83` |
| **widening** | Opérateur d'accélération : une borne qui grandit saute à ±∞ | `Interval::widen`, `interval.rs:85-106` |
| **widening à seuils / up-to** | La borne saute au plus proche seuil englobant, ±∞ sinon | `Interval::widen_to`, `interval.rs:108-146` ; ADR-014 |
| **seuils (thresholds)** | Littéraux numériques récoltés une fois par composant | `collect_thresholds`, `src/engine/fixpoint.rs:1185-1206` |
| **narrowing (ici)** | Raffinement de l'env sur une branche selon la garde (pas l'opérateur descendant) | `narrow_*` ; `narrow_env_for_branch`, `src/engine/cfg_analyzer.rs:201-294` |
| **garde (guard)** | Condition de branche qui raffine une variable | idem |
| **is_int** | Annotation « tous entiers prouvés » d'un intervalle, hors égalité/ordre | `interval.rs:5-21` |
| **stabilité** | Borne sur la trace de changement d'une valeur au fil des rendus | `enum Stability`, `stability.rs:13-52` |
| **trace de changement** | Ensemble des rendus où `Object.is(vᵢ, vᵢ₋₁)` échoue | doc `stability.rs:13-14`, ADR-017 |
| **may / must** | Borne sur-approximée (sert à se taire) / sous-approximée (sert à tirer) | `stability.rs:16-18` ; `May<T>`, `src/rules/api/query.rs:131-135` |
| **Stable** | Même référence à chaque rendu | `Stability::Stable` |
| **Versioned(S)** | Ne change qu'aux sets des slots de S (may) | `Stability::Versioned` ; `versioned`, `versioned_by` |
| **VersionedTop** | Versionné par des slots inconnus (seuil de 4 dépassé) | `Stability::VersionedTop` |
| **PerRender** | Nouvelle référence à chaque rendu (must) ; via `to_stability`, « peut changer à chaque rendu » (may) | `Stability::PerRender` |
| **Unknown** | ⊤ de la stabilité : aucune borne | `Stability::Unknown` |
| **canonisation** | `Versioned(∅) ≡ Stable`, `|S| > 4 ⇒ VersionedTop` | `Stability::versioned`, `stability.rs:55-64` |
| **conversion côté lecture** | À la lecture d'un slot, la référence devient `Versioned({slot})` | `transfer/state_value.rs:122-134` |
| **vue événement / vue inter-rendus** | Store = join des valeurs écrites ; lecture = ce que `Object.is` compare | ADR-017 §2 |
| **motion-wins** | Dans `to_stability`, un emplacement en mouvement impose `PerRender` | `state_value.rs:308-369` |
| **fraîcheur (freshness)** | `Fresh`/`Maybe`/`Not` d'une valeur écrite, pour le churn | `value_freshness`, `src/engine/written.rs:185-200` |
| **churn** | Boucle de rendu entretenue par l'écriture de références fraîches | ADR-017 §3, ADR-018 ; `written.rs:173-175` |
| **weak update** | Mise à jour par join (`old ⊔ new`) plutôt que par remplacement | `StateStore::update`, `state_store.rs:29-33` |
| **as_arith** | Vue numérique d'un opérande selon `ToNumber` (null ⇒ 0) | `transfer/state_value.rs:708-732` |
| **KindMask / populated_kinds** | Masque des sortes peuplées, énumération unique protégée par déstructuration | `state_value.rs:46-121` |
| **setter (SetterVal)** | Fonction `setState` d'un slot identifié, stable par garantie React | `setter_val.rs:15-23` |
| **AnalysisCtx** | Paquet d'état mutable (store, mémo, tas, requêtes, contexte inter) passé aux transferts | `context.rs:112-125` |
| **QueryContext** | Canal de requête inter-domaines (une requête : `callback_body`) | `context.rs:30-42` |
| **InterCtx** | Contexte d'inlining inter-composants top-down (registres, caches, pile d'appel) | `context.rs:59-103` |
| **AnalyzeChildFn** | Pointeur de fonction qui casse le cycle `transfer → fixpoint` | `context.rs:17-25` |
| **SYNTHETIC** | `ComponentId(u32::MAX)` des IR construites à la main / analyses isolées | `src/ir/component_id.rs:38` |
| **widen_trace** | Labels élargis pendant le point fixe (et itération) | `src/engine/analysis_result.rs:208` |
| **concrétisation γ** | Fonction qui associe à une valeur abstraite l'ensemble des valeurs (ou traces) concrètes qu'elle représente ; correction = `γ(a) ∪ γ(b) ⊆ γ(a ⊔ b)` | §3.5, §3.6, §3.7 |
| **hauteur (d'un treillis)** | Longueur maximale d'une chaîne strictement croissante ; finie ⇒ `widen = join` suffit | `StrConst` (6), `Stability` (8), `BoolVal`/`SetterVal` (3) ; intervalles : infinie |
| **point (intervalle ponctuel)** | `[v, v]` ; `is_point()` ; seul cas numérique « stable » pour `to_stability` | `interval.rs:61-63` |
| **⊥ non canonique** | Intervalle `lo > hi` différent de `[+∞, −∞]`, produit par un narrowing | §3.5, §8 point 4 |
| **ToNumber** | Coercition JS vers nombre ; ici `null ⇒ 0`, `undefined ⇒ NaN` (refusé), booléen/chaîne via `coerce_to_number` pour `+x` | `as_arith`, `transfer/state_value.rs:708-732`, `:1009` |
| **Object.is** | Égalité utilisée par React pour les deps et le bail-out de `setState` | §7 |
| **is_unbounded** | Prédicat « peut croître sans borne » : borne numérique infinie, référence `PerRender` ou résidu `other` | `state_value.rs:378-382` |
| **is_finitely_valued** | Prédicat « quelques primitifs seulement » (intervalle entier de largeur ≤ 16, chaînes finies, pas de référence) | `state_value.rs:389-401` |
| **StabilityVerdict** | Projection de `Stability` côté règles (`Bottom`/`Unknown` repliés sur `Unknown`) | `src/rules/api/query.rs:143-179` |
| **NullCtx** | `QueryContext` sans réponse (`callback_body = None`) | `context.rs:44-49` |
| **FixpointCtx** | `QueryContext` du point fixe : sert les corps de `useCallback` | `context.rs:147-171` |
| **from_init** | Valeur syntaxique d'un initialiseur ; sert aux updaters, pas à l'amorçage des slots | `state_value.rs:289-306` |
| **amorçage (seeding)** | Valeur initiale d'un slot `useState` = `eval_expr(init)` (ou exécution de l'initialiseur paresseux) | `src/engine/fixpoint.rs:314-353` |
| **DepsArg / Arity::Exact(0)** | Argument deps d'un hook tel que lu par l'IR ; `Exact(0)` = `[]` connu | `recompute_memo`, `transfer/state_value.rs:67-100` |
| **site, witness, anchor, reviver, seed** | Termes des relations/règles (hors de ce périmètre) | voir dossiers 06, 07, 10, 11 |

---

## 10. Plan pédagogique suggéré

### 10.1 Ordre d'exposition

1. **Motivation par l'exemple** (§6.1) : le compteur qui boucle vs le compteur
   gardé. Question : « comment prouver qu'une valeur ne grandit pas sans
   borne ? » ⇒ intervalles.
2. **Rappels de treillis** : ordre partiel, ⊥/⊤, join/meet, concrétisation γ,
   correction (`γ(a) ∪ γ(b) ⊆ γ(a ⊔ b)`), hauteur, widening. Le trait
   `AbstractDomain` comme traduction Rust (`PartialOrd` = ordre).
3. **Le treillis plat** (`BoolVal`, `SetterVal`) et la macro `flat_lattice!`
   (le plus simple, γ exact pour les booléens).
4. **Les k-ensembles** (`StrConst`) : hauteur finie par seuil.
5. **Les intervalles** : ordre, hull, arithmétique, `NaN`, `is_int`, widening
   puis widening à seuils (ADR-014) ; raffinement sur gardes.
6. **Le produit `StateValue`** (ADR-008 → ADR-015) : pourquoi un enum plat
   perd `null ∪ number`, l'observation « sortes disjointes ⇒ produit »,
   `KindMask`, `as_arith` et `ToNumber(null) = 0` (§6.2).
7. **La stabilité** : d'abord le treillis naïf d'ADR-002 (Stable/Unstable),
   puis le FP F5 et le FN `ObjChurn` qui le rendent intenable, puis les traces
   de changement, may/must, et le treillis d'ADR-017 ; conversion côté
   lecture, `useMemo`/`useCallback` (§6.4, §6.5).
8. **Projections vers les règles** : `to_stability` (motion-wins, les deux
   sens de `PerRender`), `is_unbounded`, `is_unstable_reference_only`,
   `StabilityVerdict`.
9. **Le contexte** : `AnalysisCtx`, `QueryContext`, `InterCtx`,
   `AnalyzeChildFn` — en fin de chapitre, comme passerelle vers le moteur.
10. **Limites** (§8) : les FP/FN observés comme exercices de réflexion.

Prérequis : chapitre IR (`Expr`, `Prim`, `BinOp`, `HookEntry`, CFG) et
lowering (comment `x < 10` devient `BinOp{Lt, Var, Lit}`, pourquoi `&&` est un
diamant). À lire après : moteur de point fixe (dossier 06), relations
(dossier 07), règles d'état et d'effets (dossiers 10, 11).

### 10.2 Schémas à dessiner

- Diagrammes de Hasse : `BoolVal` ; `SetterVal` ; `StrConst` (tronqué) ;
  intervalles (quelques niveaux) ; `Stability` complet (§3.6) ; le produit
  (quelques emplacements, vue « un treillis par sorte »).
- Le carré may/must d'ADR-017 (6 points, 4 retenus) avec les exemples React.
- Chronologie d'un compteur : itérations 0,1,2,3 (`[0,0]`, `[0,1]`, `[0,2]`,
  saut au seuil) pour `Unbounded` et `Guarded`.
- Diagramme « double vue » : store (valeurs écrites, `PerRender`) → lecture
  `StateVal` (`Versioned`) → deps → règles.
- Flot de `recompute_memo` : deps → stabilités → join → `ref(…)`.
- Tableau de `narrow_env_for_branch` (motif × branche → opérateur).

### 10.3 Exercices

1. Vérifier à la main que `flat_lattice!` produit un treillis (ordre, join =
   borne sup) ; montrer que `(⊥, ⊤)` tombe bien sur `Less`.
2. Calculer `widen_to([0,5], [0,12], [10,20,100])` puis
   `widen_to([0,20],[0,25],[10,20,100])` ; justifier la terminaison.
3. Prouver la correction de `narrow_gt` avec `is_int` (`x > v ⇒ x ≥ ⌊v⌋+1`).
4. Pourquoi `narrow_truthy` ne peut-il pas transformer `[0,5]` en `[1,5]` ?
   Et avec `is_int = true`, serait-ce sûr ? (Oui sur ℤ : discuter pourquoi le
   code ne le fait pas — précision non implémentée.)
5. Donner γ de `number[1,inf]|null` et suivre `setN(n + 1)` sur trois
   itérations.
6. Montrer un programme où `to_stability` rend `PerRender` alors que la valeur
   ne change jamais concrètement (indice : un intervalle non ponctuel dû à des
   écritures sur des branches exclusives).
7. Construire le γ de `Versioned({a})` et `PerRender` en traces ; montrer que
   leurs γ s'intersectent alors que `meet` rend ⊥.
8. Corriger (sur papier) `Interval::mul` pour `0 × ±∞` ; prouver la
   correction.
9. Proposer un domaine qui détecterait `Toggle` (§6.3) : quelle information
   faut-il ajouter (fait de transition « l'écriture diffère toujours de la
   valeur lue ») ? Pourquoi un domaine d'ensembles de valeurs ne suffit pas ?
10. Expliquer pourquoi `MemoOverObjState` est silencieux et
    `MemoOverNumState` ne l'est pas (§6.6), et où une correction centrale
    devrait se placer (principe « pas de workaround »).
11. `StrGrow` (§6.8) : écrire la version corrigée de `is_unbounded` qui
    fermerait ce FN, et discuter son effet sur `StrToggle` (§6.3) et sur
    `ManyStrings` (cinq constantes écrites par des handlers, `str: Top`).

---

## Vérification

Passe de relecture-vérification du 2026-09-28, sur `main` au commit
`e67b10a` (binaire `target/debug/reactant` reconstruit par `cargo build`,
à jour ; sonde `/tmp/dom04/probe` recompilée et rejouée).

**Méthode.**

- Tous les blocs de code suivis d'une référence `chemin:A-B` ont été
  comparés **mécaniquement** à `sed -n 'A,Bp'` (script de comparaison ligne
  à ligne, lecture seule) : 58 blocs référencés après ajouts, tous
  identiques au source après correction (les autres blocs sont des
  diagrammes, des formules ou des sorties CLI/sonde, vérifiés par rejeu).
- Les références en ligne (`fichier:ligne` sans extrait, ~200) ont été
  relues une à une en affichant la première ligne citée.
- Tous les exemples de §6 ont été rejoués (`reactant check … --no-color
  --show-clean [--trace] [--info]`) et la sonde relancée sur chaque fichier
  `/tmp/dom04/ex{1,2,3,5,6,7,8,9}.tsx` : sorties CLI et valeurs abstraites
  **identiques** à celles citées.
- `cargo test -q --lib domains::impls` → `103 passed` ; décomptes des
  `#[test]` par fichier et des tests d'intégration cités (`tests/narrowing.rs`
  22, `tests/widening_e2e.rs` 4) confirmés ; noms de tests cités vérifiés.
- Items publics du périmètre listés par
  `grep -n "pub fn\|pub struct\|pub enum\|pub trait\|pub type\|pub const"` :
  tous mentionnés dans le dossier.

**Corrections apportées.**

1. `SetterVal` : référence `setter_val.rs:1-37` → `:3-37` (la ligne 1 est un
   `use`, l'extrait commençait au commentaire de la ligne 3).
2. Extraits ajoutés et recalés : `KindMask` (`:57-78`, décalage d'une
   ligne corrigé), test cité `:1158-1175`, `widen_to` de `StateValue`
   `:670-676`, `returns_value` `written.rs:108-121`.
3. Carte des consommateurs (§4.7) : `api/query.rs:226` et
   `helpers/mod.rs:233` appellent `StabilityVerdict::is_stable`, pas
   `StateValue::is_stable` ; `lowering/hook_extractor.rs:756` n'est qu'un
   commentaire ; ajout de `api/query.rs:886` et `transfer/state_value.rs:97`
   pour `to_stability` ; lignes précises pour `missing_deps`,
   `unnecessary_rerender`, `stale_closure`.
4. §6.7 : « never changes » est la formulation de `describe_value` pour
   `Bottom | Stable` (et non pour `Bottom` seul).

**Ajouts.**

- §2 : graphe des ré-exports (`crate::domains` ne ré-exporte ni `SetterVal`
  ni `StrConst` ; `BoolVal`/`Interval` passent par `state_value`).
- §3.1 : tableau « qui redéfinit quoi » dans `AbstractDomain`
  (`widen_to`, ponts `StateValue`, `narrow_*`) ; doc datée du trait
  `Transfer` (`&NullCtx`).
- §3.4 : extrait de `StrConst::singleton`, liste complète des 5 tests,
  remarque sur l'égalité par contenu des `Arc`.
- §3.5 : extraits de `hull` et `neg`, `point(∞)` non entier.
- §3.7 : extrait des huit bits et des requêtes de `KindMask` (`only` vrai
  sur ⊥) ; extrait de `from_init` et **correction de fond** : ce n'est pas
  la fonction d'amorçage des slots (`eval_expr` + `exec_body` pour
  l'initialiseur paresseux, `fixpoint.rs:314-353`) ; son seul appelant est
  `written.rs::returns_value`.
- §3.8 : extraits de `InterCtx::child`/`is_recursive` (invariant « la pile
  contient les ancêtres »), `AnalysisCtx::null` (`static NULL`, trois
  appelants moteur `fixpoint.rs:168,245,327`), `impl QueryContext for
  FixpointCtx`.
- §3.9 : catalogue complet des 103 tests unitaires, avec deux tests
  *motion-wins* cités verbatim.
- §4.2 : `typeof`/`~` rendent ⊥ sur ⊥, mais `!⊥ = boolean(Top)` ;
  `UnaryOp::Unknown` ⇒ ⊤.
- §4.3 : le garde-fou des 100 itérations fait `widen` puis `break` sans
  re-test.
- §6.7 : sortie détaillée de la sonde et constat « les handlers élargissent
  sans `widen_trace` ».
- §6.8 (nouveau, `/tmp/dom04v/ex10.tsx`) : `StrGrow` — **FN observé** :
  une chaîne qui grandit sans borne passe à `str: Top` et au `widen_trace`
  mais `is_unbounded` l'ignore ⇒ `infinite-loop` « verified » ; `BitCounter`
  (`& 7`, cycle fini non vu) ; `TypeofGuard` (silence correct).
- §7 : points 10 à 13 (`Object.is` et `NaN`/`-0`, initialiseur paresseux,
  limite « Maximum update depth », primitifs non frais).
- §8 : points 23 à 28 ; point 3 (`±Infinity`) passé de « à vérifier » à
  « vérifié » par `/tmp/dom04v/ex11.tsx` ; point 5 complété par la sortie de
  la sonde sur `HalfStep`.
- §9 : 14 entrées de glossaire (γ, hauteur, point, ⊥ non canonique,
  ToNumber, Object.is, `is_unbounded`, `is_finitely_valued`,
  `StabilityVerdict`, `NullCtx`, `FixpointCtx`, `from_init`, amorçage,
  `DepsArg`).
- §5.7 : statut et décisions A/B d'ADR-007.
- §10.3 : exercice 11.

**Ce qui reste incertain (à vérifier).**

- FN `StrGrow` (§6.8) et cycles finis `ModCycle`/`Toggle`/`StrToggle`/
  `BitCounter` (§6.3, §6.8) : observés, non documentés, aucune issue
  trouvée ; à confirmer par le mainteneur (le second relève d'une limite de
  conception — pas de fait de transition pour les primitifs — le premier
  d'un oubli dans `is_unbounded`).
- `Interval::mul` et `0 × ±∞` ⇒ ⊥ (§4.2, §6.7) : sous-approximation
  confirmée par la sonde ; son effet sur la détection de bout en bout
  n'a été montré que sur un FP de `missing-deps`.
- Effet des ⊥ non canoniques d'`Interval` sur `StateStore::leq` et
  `changed_labels` (§3.5, §8.4).
- `is_int` hors égalité : scénario de resserrement erroné non reproduit
  (§8.5).
- Garde-fou des 100 itérations : soundness de l'état retenu après le
  `break` (dossier moteur).
- Mécanisme d'élargissement des handlers hors `widen_trace` (dossier
  moteur).
- `±Infinity` comme valeur concrète : `1e999` ⇒ `point(inf)` est
  **confirmé** (§8.3, `/tmp/dom04v/ex11.tsx`) ainsi que la perte de `NaN`
  par `rem` ; reste à décider (mainteneur) si c'est une limite assumée —
  `docs/limitations.md` ne parle que de `/`, `%` par diviseur nul et `**`.
- Chaîne de témoins de `HalfStep` citant des hooks qui n'écrivent pas `y`
  (§8.5, hors périmètre).
- `docs/semantics.md` annoncé par ADR-001 : absent (§5.7).
