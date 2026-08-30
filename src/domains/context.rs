use std::cell::RefCell;

use crate::{
    domains::{
        AbstractDomain, StateValue,
        stores::{AbstractEnv, Heap, MemoStore, SharedStateStore, StateStore},
    },
    engine::{
        AnalysisResult, AnalysisStats, ComponentCache, ComponentCallGraph, ComponentRegistry,
        HookRegistry, fixpoint::Config,
    },
    ir::{component::ComponentIR, types::Symbol},
};

// ── AnalyzeChildFn ─────────────────────────────────────────────────────────────

/// Function pointer for inlining a child component's analysis. Breaks the
/// circular dep between `domains::transfer` and `engine::fixpoint`.
pub type AnalyzeChildFn =
    fn(&ComponentIR, AbstractEnv<StateValue>, Heap, &InterCtx<'_>) -> AnalysisResult<StateValue>;

// ── QueryContext trait ────────────────────────────────────────────────────────

/// Cross-domain context passed to every Transfer method for abstract-state queries.
pub trait QueryContext {
    /// Body CFG of a `useCallback` hook, if the context knows it. Lets the
    /// interpreter execute calls through a callback-bound variable
    /// (`const cb = useCallback(...); ...; cb()`): the rewrite to
    /// `CallbackVal(label)` moved the body out of the expression tree, so it
    /// is not reachable through the heap like a plain FnLit.
    fn callback_body(
        &self,
        _label: crate::ir::types::HookLabel,
    ) -> Option<std::sync::Arc<crate::ir::cfg::CFG>> {
        None
    }
}

// ── NullCtx ───────────────────────────────────────────────────────────────────

/// No-op context: returns `Top` for every query. Used in tests and as recursion base.
pub struct NullCtx;

impl QueryContext for NullCtx {}

// ── InterCtx ──────────────────────────────────────────────────────────────────

/// Inter-component analysis context, threaded through the analysis when doing
/// top-down inlining across component boundaries.
///
/// Uses `RefCell` for all mutable shared state so that `InterCtx` can be passed
/// as a shared `&InterCtx` reference avoiding nested `&mut` lifetime issues while
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
    /// User-defined custom hook registry for inlining (None = no inlining).
    pub hook_registry: Option<&'a HookRegistry>,
}

impl<'a> InterCtx<'a> {
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
            hook_registry: self.hook_registry,
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
    /// Name of the component under analysis. An analysis is always the
    /// analysis of SOME component — intra or inter — so this is not an
    /// `Option`: state-slot provenance (`Versioned` labels, `SetterVal`)
    /// always carries the real owner.
    pub component: Symbol,
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
        component: Symbol,
        state: &'a mut StateStore<D>,
        memo: &'a mut MemoStore<D>,
        heap: &'a mut Heap,
    ) -> Self {
        static NULL: NullCtx = NullCtx;
        AnalysisCtx {
            component,
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
/// threaded here conservative but sound).
pub struct FixpointCtx<'a> {
    pub state: &'a StateStore<StateValue>,
    pub memo: &'a MemoStore<StateValue>,
    /// `useCallback` body CFGs by hook label (see `QueryContext::callback_body`).
    pub callbacks: &'a std::collections::HashMap<
        crate::ir::types::HookLabel,
        std::sync::Arc<crate::ir::cfg::CFG>,
    >,
}

impl QueryContext for FixpointCtx<'_> {
    fn callback_body(
        &self,
        label: crate::ir::types::HookLabel,
    ) -> Option<std::sync::Arc<crate::ir::cfg::CFG>> {
        self.callbacks.get(&label).cloned()
    }
}
