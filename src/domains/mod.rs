pub mod context;
pub mod impls;
pub mod interp;
pub mod stores;
pub mod transfer;

pub use context::{AnalysisCtx, AnalyzeChildFn, FixpointCtx, InterCtx, NullCtx, QueryContext};
pub use impls::{BoolVal, Interval, Stability, StateValue};
pub use stores::{AbstractEnv, EnvVal, Heap, HeapValue, MemoStore, StateStore};
pub use transfer::StateValueTransfer;

use crate::ir::{Expr, Stmt};

// ── AbstractDomain ────────────────────────────────────────────────────────────

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
/// (cross-domain ask pattern). Pass `&NullCtx` when no
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
    /// `deps` is `None` when the hook declares no readable deps array. That is
    /// not the same as `Some([])`: an empty array pins the memo forever, while
    /// an unreadable one bounds nothing, so the two must not share an answer.
    fn recompute_memo(
        &self,
        component: &crate::ir::types::Symbol,
        deps: Option<&crate::ir::hooks::DepsList>,
        env: &AbstractEnv<Self::Domain>,
        ctx: &mut context::AnalysisCtx<Self::Domain>,
    ) -> Self::Domain;
}
