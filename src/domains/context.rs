use crate::{
    domains::{
        AbstractDomain, StateValue, StateValueTransfer, Transfer,
        stores::{AbstractEnv, Heap, MemoStore, StateStore},
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

// ── AnalysisCtx ───────────────────────────────────────────────────────────────

/// Bundle of mutable analysis state threaded through `Transfer` methods.
///
/// Replaces the five separate `(env, state, memo, heap, ctx)` parameters.
/// `env` is kept as a separate parameter since its mutability and lifetime
/// differ between `eval_expr` (`&`) and `exec_stmt` (`&mut`).
pub struct AnalysisCtx<'a, D: AbstractDomain> {
    pub state: &'a mut StateStore<D>,
    pub memo: &'a mut MemoStore<D>,
    pub heap: &'a mut Heap,
    pub query: &'a dyn QueryContext,
}

impl<'a, D: AbstractDomain> AnalysisCtx<'a, D> {
    /// Construct with `NullCtx` as the query context (tests, simple impls).
    pub fn null(
        state: &'a mut StateStore<D>,
        memo: &'a mut MemoStore<D>,
        heap: &'a mut Heap,
    ) -> Self {
        static NULL: NullCtx = NullCtx;
        AnalysisCtx {
            state,
            memo,
            heap,
            query: &NULL,
        }
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
        let mut state = self.state.clone();
        let mut memo = self.memo.clone();
        let mut heap = Heap::new();
        let mut ctx = AnalysisCtx::null(&mut state, &mut memo, &mut heap);
        StateValueTransfer.eval_expr(expr, &AbstractEnv::bottom(), &mut ctx)
    }
}

// ── AnalysisQueryCtx ──────────────────────────────────────────────────────────

/// Post-fixpoint context: evaluates `expr` against the fully converged `AnalysisResult`.
pub struct AnalysisQueryCtx<'a> {
    pub result: &'a AnalysisResult<StateValue>,
}

impl QueryContext for AnalysisQueryCtx<'_> {
    fn state_value_of(&self, expr: &Expr) -> StateValue {
        let mut state = self.result.state_store.clone();
        let mut memo = self.result.memo_store.clone();
        let mut heap = Heap::new();
        let mut ctx = AnalysisCtx::null(&mut state, &mut memo, &mut heap);
        StateValueTransfer.eval_expr(expr, &self.result.exit_env(), &mut ctx)
    }
}
