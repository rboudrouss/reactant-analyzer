# ADR-007: Cross-domain queries — QueryContext trait (B3 implemented)

- **Status**: Implemented (B3 active, B1 groundwork in place)
- **Date**: 2026-06-02
- **Update**: 2026-06-03 — `AnalysisCtx<D>` bundles `(state, memo, heap, query)`; `Transfer` reduced to 3 params.

## Context

When several abstract domains run in product (e.g. `Stability × SetterEffect`), the `SetterEffect` domain needs to read the results of `Stability` to classify the argument of a `setState`:

- `setState({...})` → Stability says `Unstable` → new reference guaranteed → infinite loop
- `setState(count + 1)` → Stability says `Stable`, but the AST contains `StateVal(label)` → infinite loop
- `setState(42)` → Stability says `Stable`, no `StateVal` → not necessarily a loop

Without cross-domain access, `SetterEffect` cannot distinguish these cases.

### Reference: MOPSA

MOPSA (OCaml) solves this problem with a **Manager object** passed to each transfer function. The reduced product splits this manager into `fst_pair_man` / `snd_pair_man`. Cross-domain communication goes through `man.ask(Q_some_query)` — an extensible GADT where each query has its own return type `'r`:

```ocaml
val ask : ('a,'r) query -> ('a, t) man -> 'a flow -> ('a, 'r) cases option

type ('a, _) query +=
  | Q_constant_vars : ('a, var list) query
  | Q_variables_linked_to : expr -> ('a, VarSet.t) query
```

The return type is polymorphic (`'r`) and statically safe thanks to OCaml GADTs. Each domain implements `ask` for the queries it can answer; the others return `None`.

---

## Decision A: Implemented solution — `QueryContext` trait (B3)

The `Transfer` trait takes `ctx: &dyn QueryContext` — `dyn` (not `impl`) to keep `Transfer` object-safe (no generic parameter in methods).

`(state, memo, heap, query)` are now bundled in `AnalysisCtx<D>`, reducing each method from 6 → 3 params. `env` stays separate (mutability incompatible between `eval_expr` `&` and `exec_stmt` `&mut`). `recompute_memo` keeps `ctx: &dyn QueryContext` — it doesn't need state/memo/heap.

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

### Three implementations of `QueryContext`

**`NullCtx`** — returns `Top` for any query. Used in tests and as recursion base in `recompute_memo`.

**`FixpointCtx<'a>`** — used during fixpoint computation. Wraps `&StateStore<StateValue>` and `&MemoStore<StateValue>`. Passed to `analyze_cfg`, scoped at each call to avoid borrow conflicts with `memo_store.set`.

**`AnalysisQueryCtx<'a>`** — used post-fixpoint. Wraps `&AnalysisResult<StateValue>`.

---

## Decision B: Future migration — generic Manager (B1)

### The problem: GADTs don't exist in Rust

OCaml allows `type ('a, 'r) query = ..` where `'r` varies per constructor. Rust has no such mechanism.
The naive version doesn't compile:

```rust
// DOES NOT COMPILE
trait DomainQuery {
    type Result;
}

trait Manager {
    fn ask<Q: DomainQuery>(&self, q: Q) -> Option<Q::Result>;
}

struct ProductManager<M1, M2>(M1, M2);

impl<M1: Manager, M2: Manager> Manager for ProductManager<M1, M2> {
    fn ask<Q: DomainQuery>(&self, q: Q) -> Option<Q::Result> {
        // requires specialization (#31844), unstable since 2015
        self.0.ask(q).or_else(|| self.1.ask(q))
        //                              ^^ q already moved
    }
}
```

**Two Rust blockers**:

1. **`specialization` is unstable** ([tracking issue #31844](https://github.com/rust-lang/rust/issues/31844)). Without it, we cannot implement `Manager::ask` differently depending on whether `M1` knows the type `Q` or not.

2. **Move semantics**: `q` is moved in `self.0.ask(q)` before `self.1.ask(q)`. Workable with `Clone` or by passing `&Q`, but `Q::Result` may not be `Clone`.

### B1 solution viable in stable Rust: marker types + `where` bounds

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

**Advantage**: 100% stable Rust, type-safe, zero runtime overhead.
**Limit**: if T1 doesn't handle Q but T2 does, a separate impl `where T2: Queryable<Q>` is needed. A macro `impl_queryable_product!` can generate that automatically.

The groundwork exists already: `DomainQuery` and `Queryable<Q>` are defined in `query.rs`; `ProductTransfer` delegates via this trait in `product.rs`. No concrete query type is defined yet — that's the remaining work.

---

## Consequences

**Current**:
- `Transfer::eval_expr` / `exec_stmt` take `ctx: &mut AnalysisCtx<D>` (3 params instead of 6).
- `AnalysisCtx<D>` contains `state`, `memo`, `heap`, and `query: &dyn QueryContext`.
- `recompute_memo` still takes `ctx: &dyn QueryContext` directly (doesn't need state/memo/heap).
- `NullCtx` / `FixpointCtx` / `AnalysisQueryCtx` cover the three phases; accessible via `ctx.query`.
- `AnalysisCtx::null(state, memo, heap)` is a convenient constructor for tests and simple impls.
- `dyn QueryContext` ensures `Transfer`'s object-safety (no monomorphization per context).

**Remaining work**:
- Define concrete query types (`StabilityOf`, etc.) for cross-domain requests beyond `state_value_of`.
- Implement `Queryable<Q>` on the affected transfers and extend `QueryContext` or migrate to the B1 pattern if the number of domains exceeds ~5.
