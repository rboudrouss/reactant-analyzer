//! Post-fixpoint evaluation: probe an expression against a component's
//! converged stores.
//!
//! Every relation the engine derives after convergence — the writer row's
//! freshness, the effect triggers, the churn graph — and every rule-side value
//! probe evaluate through this one core (ADR-042 §6). It used to live in
//! `rules/helpers`, which made the relations that need it unable to move below
//! the rules layer.

use crate::{
    domains::{
        AbstractEnv, AnalysisCtx, StateValue, StateValueTransfer, Transfer,
        stores::{Heap, MemoStore, StateStore},
    },
    engine::AnalysisResult,
    ir::{ComponentId, expr::Expr},
};

/// Evaluate `expr` in `env` against *copies* of the given stores and `heap`,
/// through a null [`AnalysisCtx`]. The shared core of every post-fixpoint value
/// probe.
///
/// The state and memo stores are cloned internally and the heap is borrowed
/// mutably (callers pass a throwaway), so the caller's fixpoint result is never
/// disturbed. Each call site passes its OWN store bundle — the component's
/// converged stores (`comp.state_store` + a seeded `comp.heap.clone()`), or an
/// empty bundle (`StateStore::bottom()` + `Heap::new()`, for a mount-time init
/// eval). This primitive fixes none of them: it is the mechanical eval core,
/// not the store/heap policy (which is deliberately per-site — an empty vs a
/// converged heap are NOT interchangeable).
pub fn eval_in_stores(
    expr: &Expr,
    env: &AbstractEnv<StateValue>,
    component: ComponentId,
    state: &StateStore<StateValue>,
    memo: &MemoStore<StateValue>,
    heap: &mut Heap,
) -> StateValue {
    let mut s = state.clone();
    let mut m = memo.clone();
    StateValueTransfer.eval_expr(
        expr,
        env,
        &mut AnalysisCtx::null(component, &mut s, &mut m, heap),
    )
}

/// Evaluate an expression against a component's *converged* stores — heap
/// included.
///
/// The convenience layer over [`eval_in_stores`] for the common case where the
/// state store, memo store, heap and component name all come from one
/// [`AnalysisResult`]. The heap **used** to be a caller argument, on the theory
/// that an empty and a converged seed were two legitimate choices; four of the
/// six call sites chose the empty one and were wrong for it (#135). A member
/// read (`obj.f`, `form.onSubmit`) only resolves through the heap, so an empty
/// seed silently answers ⊤ — and ⊤ is the silent side of every predicate the
/// rules build on a value. One converged answer here is the whole fix: a site
/// that genuinely evaluates against *empty* stores calls [`eval_in_stores`]
/// directly, and that bundle has no converged half to be inconsistent with.
pub trait ConvergedEval {
    /// A reusable evaluator holding one scratch heap. Use this wherever more
    /// than one expression is probed — a loop over deps, over a path's
    /// prefixes — so the clone happens once instead of once per call.
    fn evaluator(&self) -> Eval<'_>;

    /// One-shot probe. Same answer as [`Self::evaluator`], one clone.
    fn eval_in(&self, env: &AbstractEnv<StateValue>, expr: &Expr) -> StateValue {
        self.evaluator().at(env, expr)
    }
}

impl ConvergedEval for AnalysisResult<StateValue> {
    fn evaluator(&self) -> Eval<'_> {
        Eval {
            result: self,
            heap: self.heap.clone(),
        }
    }
}

/// A scratch evaluator over one component's converged stores.
///
/// It owns the throwaway heap because evaluation *writes* to one — an
/// `ObjectLit` in the probed expression mints an entry — so the converged heap
/// must not be evaluated through directly. Reuse across calls is safe and is
/// the point: an `ExprId` names one allocation site (#134), so re-probing a
/// site rewrites its own entry and two different sites never collide.
pub struct Eval<'a> {
    result: &'a AnalysisResult<StateValue>,
    heap: Heap,
}

impl Eval<'_> {
    pub fn at(&mut self, env: &AbstractEnv<StateValue>, expr: &Expr) -> StateValue {
        eval_in_stores(
            expr,
            env,
            self.result.component,
            &self.result.state_store,
            &self.result.memo_store,
            &mut self.heap,
        )
    }
}
