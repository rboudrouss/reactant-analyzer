# ADR-007 : Cross-domain queries — QueryContext trait (B3 implémenté)

- **Statut** : Implémenté (B3 actif, B1 groundwork en place)
- **Date** : 2026-06-02
- **Mise à jour** : 2026-06-03 — `AnalysisCtx<D>` bundle `(state, memo, heap, query)` ; `Transfer` réduit à 3 params.

## Contexte

Quand plusieurs domaines abstraits tournent en produit (ex. `Stability × SetterEffect`), le domaine `SetterEffect` a besoin de lire les résultats de `Stability` pour classifier l'argument d'un `setState` :

- `setState({...})` → Stability dit `Unstable` → nouvelle référence garantie → boucle infinie
- `setState(count + 1)` → Stability dit `Stable`, mais l'AST contient `StateVal(label)` → boucle infinie
- `setState(42)` → Stability dit `Stable`, pas de `StateVal` → pas forcément une boucle

Sans accès cross-domaine, `SetterEffect` ne peut pas distinguer ces cas.

### Référence : MOPSA

MOPSA (OCaml) résout ce problème avec un **Manager object** passé à chaque fonction de transfert. Le produit réduit split ce manager en `fst_pair_man` / `snd_pair_man`. La communication cross-domaine passe par `man.ask(Q_some_query)` — un GADT extensible où chaque query a son propre type de retour `'r` :

```ocaml
val ask : ('a,'r) query -> ('a, t) man -> 'a flow -> ('a, 'r) cases option

type ('a, _) query +=
  | Q_constant_vars : ('a, var list) query
  | Q_variables_linked_to : expr -> ('a, VarSet.t) query
```

Le type de retour est polymorphe (`'r`) et statiquement sûr grâce aux GADTs OCaml. Chaque domaine implémente `ask` pour les queries qu'il sait répondre ; les autres retournent `None`.

---

## Decision A : Solution implémentée — `QueryContext` trait (B3)

Le trait `Transfer` prend `ctx: &dyn QueryContext` — `dyn` (pas `impl`) pour garder `Transfer` object-safe (pas de paramètre générique dans les méthodes).

`(state, memo, heap, query)` sont maintenant bundlés dans `AnalysisCtx<D>`, réduisant chaque méthode de 6 → 3 params. `env` reste séparé (mutabilité incompatible entre `eval_expr` `&` et `exec_stmt` `&mut`). `recompute_memo` garde `ctx: &dyn QueryContext` — il n'a pas besoin de state/memo/heap.

```rust
pub struct AnalysisCtx<'a, D: AbstractDomain> {
    pub state: &'a mut StateStore<D>,
    pub memo:  &'a mut MemoStore<D>,
    pub heap:  &'a mut Heap,
    pub query: &'a dyn QueryContext,
}

pub trait QueryContext {
    fn state_value_of(&self, expr: &Expr) -> StateValue;
}

pub trait Transfer {
    type Domain: AbstractDomain;

    fn eval_expr(
        &self,
        expr: &Expr,
        env: &AbstractEnv<Self::Domain>,
        ctx: &mut AnalysisCtx<Self::Domain>,
    ) -> Self::Domain;

    fn exec_stmt(
        &self,
        stmt: &Stmt,
        env: &mut AbstractEnv<Self::Domain>,
        ctx: &mut AnalysisCtx<Self::Domain>,
    );

    fn recompute_memo(
        &self,
        deps: &[Expr],
        env: &AbstractEnv<Self::Domain>,
        ctx: &dyn QueryContext,
    ) -> Self::Domain;
}
```

### Trois implémentations de `QueryContext`

**`NullCtx`** — retourne `Top` pour toute query. Utilisé dans les tests et comme base de récursion dans `recompute_memo`.

**`FixpointCtx<'a>`** — utilisé pendant le calcul du point fixe. Enveloppe `&StateStore<StateValue>` et `&MemoStore<StateValue>`. Passé à `analyze_cfg`, scopé à chaque appel pour éviter les conflits d'emprunt avec `memo_store.set`.

**`AnalysisQueryCtx<'a>`** — utilisé post-point-fixe. Enveloppe `&AnalysisResult<StateValue>`.

---

## Decision B : Migration future — Manager générique (B1)

### Le problème : les GADTs n'existent pas en Rust

OCaml permet `type ('a, 'r) query = ..` où `'r` varie par constructeur. Rust n'a pas ce mécanisme.  
Le naïf ne compile pas :

```rust
// NE COMPILE PAS
trait DomainQuery {
    type Result;
}

trait Manager {
    fn ask<Q: DomainQuery>(&self, q: Q) -> Option<Q::Result>;
}

struct ProductManager<M1, M2>(M1, M2);

impl<M1: Manager, M2: Manager> Manager for ProductManager<M1, M2> {
    fn ask<Q: DomainQuery>(&self, q: Q) -> Option<Q::Result> {
        // requires specialization (#31844), unstable depuis 2015
        self.0.ask(q).or_else(|| self.1.ask(q))
        //                              ^^ q déjà moved
    }
}
```

**Deux blocages Rust** :

1. **`specialization` est unstable** ([tracking issue #31844](https://github.com/rust-lang/rust/issues/31844)). Sans elle, on ne peut pas implémenter `Manager::ask` différemment selon si `M1` connaît le type `Q` ou non.

2. **Move sémantique** : `q` est moved dans `self.0.ask(q)` avant `self.1.ask(q)`. Contournable avec `Clone` ou en passant `&Q`, mais `Q::Result` peut ne pas être `Clone`.

### Solution B1 viable en Rust stable : marker types + `where` bounds

```rust
struct StabilityOf<'a>(pub &'a Expr);

trait Queryable<Q: DomainQuery> {
    fn ask(&self, q: &Q, env: &AbstractEnv<Self::Domain>, ...) -> Q::Result
    where Self: Transfer;
}

impl Queryable<StabilityOf<'_>> for StabilityTransfer {
    fn ask(&self, q: &StabilityOf<'_>, env: &AbstractEnv<Stability>, ...) -> Stability {
        eval_stability(q.0, env, ...)
    }
}

impl<T1, T2, Q> Queryable<Q> for ProductTransfer<T1, T2>
where
    T1: Transfer + Queryable<Q>,
    Q: DomainQuery,
{
    fn ask(&self, q: &Q, ...) -> Q::Result {
        self.t1.ask(q, ...)
    }
}
```

**Avantage** : 100% stable Rust, type-safe, zéro overhead runtime.  
**Limite** : si T1 ne gère pas Q mais T2 le fait, il faut un impl séparé `where T2: Queryable<Q>`. Un macro `impl_queryable_product!` peut générer ça automatiquement.

Le groundwork existe déjà : `DomainQuery` et `Queryable<Q>` sont définis dans `query.rs` ; `ProductTransfer` délègue via ce trait dans `product.rs`. Aucun type de query concret n'est encore défini — c'est le travail restant.

---

## Conséquences

**Actuel** :
- `Transfer::eval_expr` / `exec_stmt` prennent `ctx: &mut AnalysisCtx<D>` (3 params au lieu de 6).
- `AnalysisCtx<D>` contient `state`, `memo`, `heap`, et `query: &dyn QueryContext`.
- `recompute_memo` prend encore `ctx: &dyn QueryContext` directement (pas besoin de state/memo/heap).
- `NullCtx` / `FixpointCtx` / `AnalysisQueryCtx` couvrent les trois phases ; accessible via `ctx.query`.
- `AnalysisCtx::null(state, memo, heap)` est un constructeur pratique pour tests et impls simples.
- `dyn QueryContext` assure l'object-safety de `Transfer` (pas de monomorphisation par contexte).

**Reste à faire** :
- Définir des types de query concrets (`StabilityOf`, etc.) pour les requêtes cross-domaine au-delà de `state_value_of`.
- Implémenter `Queryable<Q>` sur les transfers concernés et étendre `QueryContext` ou migrer vers le pattern B1 si le nombre de domaines dépasse ~5.
