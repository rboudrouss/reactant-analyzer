pub mod impls;
pub mod stores;

pub use impls::{Stability, StabilityTransfer};
pub use stores::{AbstractEnv, MemoStore, StateStore};

use crate::ir::{Expr, Stmt};

/// Core abstract domain trait.
///
/// Supertrait bounds:
/// - `Clone + Copy`  — values are small, freely copyable
/// - `PartialEq`     — needed for convergence checks
/// - `PartialOrd`    — lattice order (a ≤ b = a ⊑ b)
/// - `Debug`         — required for diagnostics and derive macros on generic containers
pub trait AbstractDomain: Clone + Copy + PartialEq + PartialOrd + std::fmt::Debug {
    fn bottom() -> Self;
    fn top() -> Self;
    fn is_bottom(&self) -> bool;
    fn join(&self, other: &Self) -> Self;
    fn meet(&self, other: &Self) -> Self;
    fn widen(&self, other: &Self) -> Self;
}

// ── Transfer trait ────────────────────────────────────────────────────────────

/// Domain-specific transfer functions for a single abstract analysis pass.
///
/// Each implementation binds one domain (`type Domain`) and defines how
/// expressions are evaluated and how statements update the abstract state.
/// Adding a new domain = new struct + `impl Transfer`.
///
/// The engine (stage 5) will be parameterised over `T: Transfer`, enabling
/// per-analysis-run domain selection and composition.
pub trait Transfer {
    type Domain: AbstractDomain;

    /// Abstract evaluation of an expression in the current abstract state.
    fn eval_expr(
        &self,
        expr: &Expr,
        env: &AbstractEnv<Self::Domain>,
        state: &StateStore<Self::Domain>,
        memo: &MemoStore<Self::Domain>,
    ) -> Self::Domain;

    /// Execute a statement, updating `env`, `state`, and `memo` in place.
    fn exec_stmt(
        &self,
        stmt: &Stmt,
        env: &mut AbstractEnv<Self::Domain>,
        state: &mut StateStore<Self::Domain>,
        memo: &mut MemoStore<Self::Domain>,
    );

    /// Compute the abstract value for a memoized hook from its dependency list.
    /// Called by the engine after each render-pass to refresh the memo store.
    fn recompute_memo(
        &self,
        deps: &[Expr],
        env: &AbstractEnv<Self::Domain>,
    ) -> Self::Domain;
}
