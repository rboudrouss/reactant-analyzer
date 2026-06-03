pub mod context;
pub mod impls;
pub mod interp;
pub mod product;
pub mod query;
pub mod stores;
pub mod transfer;

pub use context::{AnalysisCtx, AnalysisQueryCtx, FixpointCtx, NullCtx, QueryContext};
pub use impls::{BoolVal, Interval, Stability, StateValue};
pub use product::{ProductDomain, ProductTransfer};
pub use query::{DomainQuery, Queryable};
pub use stores::{AbstractEnv, EnvVal, Heap, HeapValue, MemoStore, StateStore};
pub use transfer::StateValueTransfer;

use crate::ir::{Expr, Stmt};

// ── AbstractDomain ────────────────────────────────────────────────────────────

/// Core abstract domain trait.
///
/// Supertrait bounds:
/// - `Clone + Copy`  — values are small, freely copyable
/// - `PartialEq`     — needed for convergence checks
/// - `PartialOrd`    — lattice order (a ≤ b = a ⊑ b)
/// - `Debug`         — required for diagnostics and derive macros on generic containers
pub trait AbstractDomain: Clone + PartialEq + PartialOrd + std::fmt::Debug {
    fn bottom() -> Self;
    fn top() -> Self;
    fn is_bottom(&self) -> bool;
    fn join(&self, other: &Self) -> Self;
    fn meet(&self, other: &Self) -> Self;
    fn widen(&self, other: &Self) -> Self;

    // Branch narrowing: default = identity (sound, imprecise).
    // Override for numeric domains to refine interval bounds on branch conditions.
    fn narrow_lt(self, _v: f64) -> Self {
        self
    }
    fn narrow_leq(self, _v: f64) -> Self {
        self
    }
    fn narrow_gt(self, _v: f64) -> Self {
        self
    }
    fn narrow_geq(self, _v: f64) -> Self {
        self
    }
    fn narrow_eq(self, _v: f64) -> Self {
        self
    }
    fn narrow_neq(self, _v: f64) -> Self {
        self
    }
}

// ── Transfer trait ────────────────────────────────────────────────────────────

/// Domain-specific transfer functions for a single abstract analysis pass.
///
/// Each implementation binds one domain (`type Domain`) and defines how
/// expressions are evaluated and how statements update the abstract state.
/// Adding a new domain = new struct + `impl Transfer`.
///
/// The `ctx` parameter lets a Transfer query other domains during analysis
/// (cross-domain ask pattern — ADR-007 B3). Pass `&NullCtx` when no
/// cross-domain queries are needed (tests, simple impls).
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

    /// Compute the abstract value for a memoized hook from its dependency list.
    /// Called by the engine after each render-pass to refresh the memo store.
    fn recompute_memo(
        &self,
        deps: &[Expr],
        env: &AbstractEnv<Self::Domain>,
        ctx: &dyn QueryContext,
    ) -> Self::Domain;
}
