use std::cell::RefCell;

use crate::{
    domains::{
        AbstractDomain, StateValue, StateValueTransfer, Transfer,
        stores::{AbstractEnv, Heap, MemoStore, SharedStateStore, StateStore},
    },
    engine::{
        AnalysisResult, AnalysisStats, ComponentCache, ComponentCallGraph, ComponentRegistry,
        fixpoint::Config,
    },
    ir::{component::ComponentIR, expr::Expr, types::Symbol},
};

// ── AnalyzeChildFn ─────────────────────────────────────────────────────────────

/// Function pointer type for inlining a child component's analysis.
/// Provided by `engine::fixpoint` at `InterCtx` creation time to break the
/// circular dependency between `domains::transfer` and `engine::fixpoint`.
/// `initial_heap` carries pre-populated heap entries (props abstract object) for the child.
pub type AnalyzeChildFn =
    fn(&ComponentIR, AbstractEnv<StateValue>, Heap, &InterCtx<'_>) -> AnalysisResult<StateValue>;

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

// ── InterCtx ──────────────────────────────────────────────────────────────────

/// Inter-component analysis context, threaded through the analysis when doing
/// top-down inlining across component boundaries.
///
/// Uses `RefCell` for all mutable shared state so that `InterCtx` can be passed
/// as a shared `&InterCtx` reference — avoiding nested `&mut` lifetime issues while
/// still allowing mutation through `borrow_mut()`.
pub struct InterCtx<'a> {
    pub registry: &'a ComponentRegistry,
    pub cache: &'a RefCell<ComponentCache>,
    pub shared_state: &'a RefCell<SharedStateStore>,
    pub call_graph: &'a RefCell<ComponentCallGraph>,
    pub stats: &'a RefCell<AnalysisStats>,
    /// All analysis results accumulated across the entire program analysis.
    pub results: &'a RefCell<std::collections::HashMap<Symbol, AnalysisResult<StateValue>>>,
    /// Components currently being analyzed (for recursion detection).
    pub call_stack: RefCell<Vec<Symbol>>,
    /// Name of the component being analyzed at this level.
    pub component_name: Symbol,
    /// Analysis config (widen_threshold etc.).
    pub config: &'a Config,
    /// Callback provided by `engine::fixpoint` to inline a child component's analysis.
    pub analyze_child: AnalyzeChildFn,
}

impl<'a> InterCtx<'a> {
    pub fn new(
        registry: &'a ComponentRegistry,
        cache: &'a RefCell<ComponentCache>,
        shared_state: &'a RefCell<SharedStateStore>,
        call_graph: &'a RefCell<ComponentCallGraph>,
        stats: &'a RefCell<AnalysisStats>,
        results: &'a RefCell<std::collections::HashMap<Symbol, AnalysisResult<StateValue>>>,
        component_name: Symbol,
        config: &'a Config,
        analyze_child: AnalyzeChildFn,
    ) -> Self {
        InterCtx {
            registry,
            cache,
            shared_state,
            call_graph,
            stats,
            results,
            call_stack: RefCell::new(vec![]),
            component_name,
            config,
            analyze_child,
        }
    }

    /// Create a child context for inlining a nested component.
    /// Shares all RefCell state; new call_stack with parent pushed.
    pub fn child(&self, child_name: Symbol) -> InterCtx<'a> {
        let mut new_stack = self.call_stack.borrow().clone();
        new_stack.push(self.component_name.clone());
        InterCtx {
            registry: self.registry,
            cache: self.cache,
            shared_state: self.shared_state,
            call_graph: self.call_graph,
            stats: self.stats,
            results: self.results,
            call_stack: RefCell::new(new_stack),
            component_name: child_name,
            config: self.config,
            analyze_child: self.analyze_child,
        }
    }

    pub fn is_recursive(&self, name: &Symbol) -> bool {
        self.call_stack.borrow().contains(name) || &self.component_name == name
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
    /// Optional inter-component context. `None` = intra-component analysis only.
    pub inter: Option<&'a InterCtx<'a>>,
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
            inter: None,
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
