use crate::{
    domains::{StateValue, StateValueTransfer, Transfer},
    engine::AnalysisResult,
    ir::expr::Expr,
};

// ── QueryContext trait ────────────────────────────────────────────────────────

/// Read-only cross-domain context for post-pass analyses (SetterEffect, SCC,
/// etc.). Passed by reference to analyses that need to query the converged
/// abstract state of other domains.
///
/// This is the B1 manager step (ADR-007 §Decision B). Concrete typed methods
/// avoid GADT machinery while remaining extensible — adding a new domain means
/// adding a new method here and updating `AnalysisQueryCtx`.
///
/// When 5+ domains exist and the method list grows unwieldy, migrate to the
/// full `Queryable<Q>` pattern described in ADR-007 §Option B1.
pub trait QueryContext {
    /// Evaluate `expr` using the StateValue domain results from the converged
    /// fixpoint. Used by post-passes that need numeric / reference values.
    fn state_value_of(&self, expr: &Expr) -> StateValue;
}

// ── AnalysisQueryCtx ──────────────────────────────────────────────────────────

/// `QueryContext` implementation wrapping a fully-converged `AnalysisResult`.
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
        )
    }
}
