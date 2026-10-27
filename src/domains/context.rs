use std::cell::RefCell;

use crate::{
    domains::{
        AbstractDomain, StateValue,
        stores::{AbstractEnv, EnvVal, Heap, HeapValue, MemoStore, SharedStateStore, StateStore},
    },
    engine::{
        AnalysisResult, AnalysisStats, ComponentCache, ComponentCallGraph, ComponentRegistry,
        HookRegistry, ProgramReducers, Reducer, Reducers, fixpoint::Config,
    },
    ir::{ComponentId, component::ComponentIR},
};

// ── AnalyzeChildFn ─────────────────────────────────────────────────────────────

/// Function pointer for inlining a child component's analysis. Breaks the
/// circular dep between `domains::transfer` and `engine::fixpoint`.
pub type AnalyzeChildFn = fn(
    &ComponentIR,
    ComponentId,
    AbstractEnv<StateValue>,
    Heap,
    &InterCtx<'_>,
) -> AnalysisResult<StateValue>;

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

    /// The reducer of slot `(owner, label)` if it is a `useReducer` slot:
    /// a write through its `dispatch` stores `reducer(state, action)`.
    fn reducer(&self, _owner: ComponentId, _label: crate::ir::types::HookLabel) -> Option<Reducer> {
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
    /// Every `useReducer` slot's reducer, registered by its owner: what a
    /// child's write through a parent's `dispatch` stores.
    pub reducers: &'a ProgramReducers,
    pub call_graph: &'a RefCell<ComponentCallGraph>,
    pub stats: &'a RefCell<AnalysisStats>,
    /// All analysis results accumulated across the entire program analysis.
    /// A child's is its analysis under [`InterCtx::inputs`]: every call
    /// site's props at once (#175).
    pub results: &'a RefCell<std::collections::HashMap<ComponentId, AnalysisResult<StateValue>>>,
    /// Per child, the join of the inputs of every call site analysed so
    /// far: what its entry in `results` was computed under.
    pub inputs: &'a RefCell<std::collections::HashMap<ComponentId, ChildInput>>,
    /// Per element, the join of the inputs it handed its child: what the
    /// child is re-analysed under once phase 1 is over, to keep the facts the
    /// join above loses ([`crate::engine::SiteFacts`]).
    pub site_inputs: &'a RefCell<std::collections::HashMap<SiteKey, ChildInput>>,
    /// A re-analysis under one call site's input, for the facts it keeps and
    /// nothing else. Its input is below the one its result was stored under,
    /// so what it would hand its children and write into other components'
    /// slots is already recorded: it records nothing, and analyses no child.
    pub probe: bool,
    /// Components currently being analyzed (for recursion detection).
    pub call_stack: RefCell<Vec<ComponentId>>,
    /// The component being analyzed at this level.
    pub component: ComponentId,
    /// Analysis config (widen_threshold etc.).
    pub config: &'a Config,
    /// Callback provided by `engine::fixpoint` to inline a child component's analysis.
    pub analyze_child: AnalyzeChildFn,
    /// User-defined custom hook registry for inlining (None = no inlining).
    pub hook_registry: Option<&'a HookRegistry>,
    /// Some body of the program fires event listeners itself
    /// (`dispatchEvent`), so no listener is event-only (#219).
    pub dispatches: bool,
}

impl<'a> InterCtx<'a> {
    /// Create a child context for inlining a nested component.
    /// Shares all RefCell state; new call_stack with parent pushed.
    pub fn child(&self, child: ComponentId) -> InterCtx<'a> {
        let mut new_stack = self.call_stack.borrow().clone();
        new_stack.push(self.component);
        InterCtx {
            registry: self.registry,
            cache: self.cache,
            shared_state: self.shared_state,
            reducers: self.reducers,
            call_graph: self.call_graph,
            stats: self.stats,
            results: self.results,
            inputs: self.inputs,
            site_inputs: self.site_inputs,
            probe: self.probe,
            call_stack: RefCell::new(new_stack),
            component: child,
            config: self.config,
            analyze_child: self.analyze_child,
            hook_registry: self.hook_registry,
            dispatches: self.dispatches,
        }
    }

    pub fn is_recursive(&self, id: ComponentId) -> bool {
        self.call_stack.borrow().contains(&id) || self.component == id
    }

    /// Analyse this context's component under `input`: its props object,
    /// with the heap the props reach, so the child resolves a member read
    /// through it.
    pub fn analyze_under(&self, ir: &ComponentIR, input: ChildInput) -> AnalysisResult<StateValue> {
        let mut env = AbstractEnv::bottom();
        let mut heap = input.heap;
        heap.insert(input.props_id, HeapValue::Obj(input.props));
        env.extend_loc(ir.param.clone(), input.props_id);
        (self.analyze_child)(ir, self.component, env, heap, self)
    }
}

/// One element: `(child, caller, span)`.
pub type SiteKey = (
    ComponentId,
    ComponentId,
    Option<crate::ir::source_range::SourceRange>,
);

/// What a child's analysis depends on: its props, and the heap they reach,
/// under one props object per child so that two call sites' inputs join
/// (#175).
#[derive(Debug, Clone)]
pub struct ChildInput {
    pub props_id: crate::ir::types::ExprId,
    pub props: std::collections::HashMap<crate::ir::types::Symbol, EnvVal<StateValue>>,
    pub heap: Heap,
}

impl ChildInput {
    /// Either site's input. A prop one site does not pass is `undefined`
    /// there.
    pub fn join(
        &self,
        props: &std::collections::HashMap<crate::ir::types::Symbol, EnvVal<StateValue>>,
        heap: &Heap,
    ) -> Self {
        let absent = EnvVal::Val(StateValue::undefined());
        let mut joined = self.props.clone();
        for (k, v) in props {
            let mine = self.props.get(k).unwrap_or(&absent);
            joined.insert(k.clone(), mine.join(v));
        }
        for (k, v) in joined.iter_mut() {
            if !props.contains_key(k) {
                *v = v.join(&absent);
            }
        }
        ChildInput {
            props_id: self.props_id,
            props: joined,
            heap: self.heap.join(heap),
        }
    }

    /// Is a site's input below this one: the child's analysis under it
    /// already covers the site?
    pub fn covers(
        &self,
        props: &std::collections::HashMap<crate::ir::types::Symbol, EnvVal<StateValue>>,
        heap: &Heap,
    ) -> bool {
        let absent = EnvVal::Val(StateValue::undefined());
        props
            .iter()
            .all(|(k, v)| self.props.get(k).is_some_and(|mine| v.leq(mine)))
            && self
                .props
                .iter()
                .filter(|(k, _)| !props.contains_key(*k))
                .all(|(_, mine)| absent.leq(mine))
            && heap.leq(&self.heap)
    }
}

// ── AnalysisCtx ───────────────────────────────────────────────────────────────

/// Bundle of mutable analysis state threaded through `Transfer` methods.
///
/// Replaces the five separate `(env, state, memo, heap, ctx)` parameters.
/// `env` is kept as a separate parameter since its mutability and lifetime
/// differ between `eval_expr` (`&`) and `exec_stmt` (`&mut`).
pub struct AnalysisCtx<'a, D: AbstractDomain> {
    /// The component under analysis. An analysis is always the analysis of
    /// SOME component — intra or inter — so this is not an `Option`:
    /// state-slot provenance (`Versioned` labels, `SetterVal`) always carries
    /// the real owner. A hand-built IR analysed on its own carries
    /// [`ComponentId::SYNTHETIC`].
    pub component: ComponentId,
    pub state: &'a mut StateStore<D>,
    /// Read-only: a pass reads memo values, and only the fixpoint, between
    /// passes, recomputes them.
    pub memo: &'a MemoStore<D>,
    /// Read-only: the component's own state when this pass began, which an
    /// updater or a reducer reads as its current value along with `state`.
    /// A pass that starts from ⊥ to collect only its writes (the pure-writes
    /// re-run) still reads the converged state here. `None`: `state` is all
    /// the pass knows.
    pub start: Option<&'a StateStore<D>>,
    pub heap: &'a mut Heap,
    pub query: &'a dyn QueryContext,
    /// Optional inter-component context. `None` = intra-component analysis only.
    pub inter: Option<&'a InterCtx<'a>>,
    /// How a loop in any body this context runs is widened — a hook body and
    /// every closure it executes share one policy, because they share one
    /// solver.
    pub widening: Widening<'a>,
}

/// When a loop header stops joining and starts widening (ADR-014): after
/// `after` arrivals over a back edge, up to the tightest of `thresholds`.
#[derive(Debug, Clone, Copy)]
pub struct Widening<'a> {
    pub after: usize,
    pub thresholds: &'a [f64],
}

impl Widening<'static> {
    /// No thresholds harvested: a growing bound widens straight to ±∞ — the
    /// policy of a probe evaluated outside any component's fixpoint.
    pub const DEFAULT: Widening<'static> = Widening {
        after: 3,
        thresholds: &[],
    };
}

impl<'a, D: AbstractDomain> AnalysisCtx<'a, D> {
    /// Construct with `NullCtx` as the query context (tests, simple impls).
    pub fn null(
        component: ComponentId,
        state: &'a mut StateStore<D>,
        memo: &'a MemoStore<D>,
        heap: &'a mut Heap,
    ) -> Self {
        static NULL: NullCtx = NullCtx;
        AnalysisCtx {
            component,
            state,
            memo,
            start: None,
            heap,
            query: &NULL,
            inter: None,
            widening: Widening::DEFAULT,
        }
    }

    /// Weakly store `val` into slot `(owner, label)`: the component's own slot
    /// in its store, another component's in the shared store its owner's
    /// fixpoint imports. Every slot write lands through here.
    pub fn write_slot(&mut self, owner: ComponentId, label: crate::ir::types::HookLabel, val: D) {
        if owner == self.component {
            self.state.update(label, val);
        } else if let Some(inter) = self.inter
            && !inter.probe
        {
            let val = val.as_state_value().unwrap_or_else(StateValue::top);
            inter.shared_state.borrow_mut().update(owner, label, val);
        }
    }
}

// ── FixpointCtx ───────────────────────────────────────────────────────────────

/// What a component's fixpoint knows that a body cannot read off its own
/// syntax: the `useCallback` bodies and the `useReducer` reducers in reach.
/// Fixed for the whole fixpoint — built once, borrowed by every pass.
pub struct FixpointCtx<'a> {
    /// `useCallback` body CFGs by hook label (see `QueryContext::callback_body`).
    pub callbacks: &'a std::collections::HashMap<
        crate::ir::types::HookLabel,
        std::sync::Arc<crate::ir::cfg::CFG>,
    >,
    /// `useReducer` reducers reachable from this component (see
    /// `QueryContext::reducer`).
    pub reducers: &'a Reducers<'a>,
}

impl QueryContext for FixpointCtx<'_> {
    fn callback_body(
        &self,
        label: crate::ir::types::HookLabel,
    ) -> Option<std::sync::Arc<crate::ir::cfg::CFG>> {
        self.callbacks.get(&label).cloned()
    }

    fn reducer(&self, owner: ComponentId, label: crate::ir::types::HookLabel) -> Option<Reducer> {
        self.reducers.get(owner, label)
    }
}
