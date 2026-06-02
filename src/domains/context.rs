use crate::{
    domains::{
        StateValue, StateValueTransfer, Transfer,
        stores::{AbstractEnv, MemoStore, StateStore},
    },
    engine::AnalysisResult,
    ir::expr::Expr,
};

// ── QueryContext trait ────────────────────────────────────────────────────────

/// Cross-domain context passed to every Transfer method.
///
/// Allows a Transfer to query the abstract state of other domains during
/// `eval_expr` / `exec_stmt`. Implemented by:
/// - `NullCtx`          — no-op (returns Top); used in tests and as the recursion base.
/// - `FixpointCtx`      — mid-fixpoint queries using the current iteration's state.
/// - `AnalysisQueryCtx` — post-fixpoint queries using the converged `AnalysisResult`.
pub trait QueryContext {
    fn state_value_of(&self, expr: &Expr) -> StateValue;
}

// ── NullCtx ───────────────────────────────────────────────────────────────────

/// No-op context: conservatively returns `Top` for every query.
///
/// Used in tests and as the recursive base case inside `FixpointCtx` and
/// `AnalysisQueryCtx` (preventing infinite dispatch when `StateValueTransfer`
/// is called with a ctx that itself calls `StateValueTransfer`).
pub struct NullCtx;

impl QueryContext for NullCtx {
    fn state_value_of(&self, _expr: &Expr) -> StateValue {
        StateValue::Top
    }
}

// ── FixpointCtx ───────────────────────────────────────────────────────────────

/// Mid-fixpoint context: evaluates `expr` against the current iteration's state.
///
/// Used by `analyze_cfg` to give Transfer methods read access to the fixpoint
/// state accumulated so far. Local variables return `Bottom` (no per-block env
/// threaded here — conservative but sound).
pub struct FixpointCtx<'a> {
    pub state: &'a StateStore<StateValue>,
    pub memo: &'a MemoStore<StateValue>,
}

impl QueryContext for FixpointCtx<'_> {
    fn state_value_of(&self, expr: &Expr) -> StateValue {
        StateValueTransfer.eval_expr(
            expr,
            &AbstractEnv::bottom(),
            self.state,
            self.memo,
            &NullCtx,
        )
    }
}

// ── AnalysisQueryCtx ──────────────────────────────────────────────────────────

/// Post-fixpoint context: evaluates `expr` against the fully converged `AnalysisResult`.
pub struct AnalysisQueryCtx<'a> {
    pub result: &'a AnalysisResult<StateValue>,
}

impl QueryContext for AnalysisQueryCtx<'_> {
    fn state_value_of(&self, expr: &Expr) -> StateValue {
        StateValueTransfer.eval_expr(
            expr,
            &self.result.exit_env(),
            &self.result.state_store,
            &self.result.memo_store,
            &NullCtx,
        )
    }
}
