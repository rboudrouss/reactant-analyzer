# ADR-007 : Cross-domain queries — AnalysisCtx now, typed Manager later

## <!> Outdated ?

- **Statut** : Accepté (B3 actif, B1 documenté pour migration future)
- **Date** : 2026-06-02

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

## Decision A : Solution — `AnalysisCtx` struct (B3)

Passer une struct concrète en lecture seule à `exec_stmt` / `eval_expr` :

```rust
pub struct AnalysisCtx<'a> {
    pub stability_env:   &'a AbstractEnv<Stability>,
    pub stability_state: &'a StateStore<Stability>,
    // ajouter un champ par domaine supplémentaire
}
```

Le trait `Transfer` devient :

```rust
pub trait Transfer {
    type Domain: AbstractDomain;

    fn eval_expr(
        &self,
        expr: &Expr,
        env: &AbstractEnv<Self::Domain>,
        state: &StateStore<Self::Domain>,
        memo: &MemoStore<Self::Domain>,
        ctx: &AnalysisCtx<'_>,        // ← nouveau
    ) -> Self::Domain;

    fn exec_stmt(
        &self,
        stmt: &Stmt,
        env: &mut AbstractEnv<Self::Domain>,
        state: &mut StateStore<Self::Domain>,
        memo: &mut MemoStore<Self::Domain>,
        ctx: &AnalysisCtx<'_>,        // ← nouveau
    );
}
```

**Avantages** : pas de type erasure, pas de lifetime hell, aucun downcast, immédiatement lisible.  
**Limite** : chaque nouveau domaine ajoute un champ à `AnalysisCtx`. Acceptable à ≤5 domaines.

---

## Decision B : Solution future — Manager générique (B1)

Quand le nombre de domaines dépasse ~5, migrer vers un Manager typé :

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
        // M1::ask retourne Option<Q::Result>, M2::ask retourne Option<Q::Result>
        // mais on ne peut pas savoir statiquement si M1 ou M2 gère Q
        // → requires specialization (#31844), unstable depuis 2015
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
// Chaque domaine déclare les queries qu'il répond
struct StabilityOf<'a>(pub &'a Expr);
struct SetterEffectOf(pub HookLabel);

// Chaque Transfer implémente Queryable<Q> pour ses queries
trait Queryable<Q: DomainQuery> {
    fn ask(&self, q: &Q, env: &AbstractEnv<Self::Domain>, ...) -> Q::Result
    where Self: Transfer;
}

impl Queryable<StabilityOf<'_>> for StabilityTransfer {
    fn ask(&self, q: &StabilityOf<'_>, env: &AbstractEnv<Stability>, ...) -> Stability {
        eval_stability(q.0, env, ...)
    }
}

// ProductTransfer délègue au premier sous-domaine qui implémente Queryable<Q>
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
**Limite** : si T1 ne gère pas Q mais T2 le fait, il faut un impl séparé `where T2: Queryable<Q>`. Avec N domaines et M queries, ça donne potentiellement N×M impls dans `ProductTransfer`. Un macro `impl_queryable_product!` peut générer ça automatiquement.

### Pattern de migration depuis B3 vers B1

1. Remplacer `&AnalysisCtx<'_>` dans le trait `Transfer` par `ctx: &impl QueryContext`
2. `QueryContext` est un trait avec des méthodes concrètes (pas de GADT) :
   ```rust
   trait QueryContext {
       fn stability_of(&self, expr: &Expr) -> Stability;
       fn setter_effect_of(&self, label: HookLabel) -> SetterEffect;
   }
   ```
3. Implémenter `QueryContext` pour `AnalysisCtx` (migration transparente, même API)
4. Implémenter `QueryContext` pour `ProductManager<T1, T2>` par délégation
5. Supprimer `AnalysisCtx` une fois tous les sites migrés

Cette migration est mécanique et ne casse pas les règles existantes.

---

## Conséquences

- **Maintenant** : `Transfer` prend `ctx: &AnalysisCtx<'_>`. `SetterEffect` lit `ctx.stability_env` directement.
- **Futur (>5 domaines)** : migrer vers `ctx: &impl QueryContext` via le pattern ci-dessus.
- `StabilityTransfer` passe `ctx` sans l'utiliser (`let _ = ctx`) — overhead nul.
- Chaque nouveau domaine ajoute un champ à `AnalysisCtx` ET une méthode à `QueryContext` lors de la migration.
