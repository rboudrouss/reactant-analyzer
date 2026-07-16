use std::collections::{HashMap, HashSet};

use crate::{
    domains::{
        AbstractDomain,
        stores::{AbstractEnv, Heap, MemoStore, StateStore},
    },
    ir::{
        SourceRange,
        cfg::{CFG, Terminator},
        expr::Expr,
        free_vars::AccessPath,
        hooks::HookEntry,
        types::{BlockId, HookLabel, Symbol, Var},
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKind {
    State,
    Effect,
    Memo,
    Callback,
    Ref,
    Custom,
    Handler,
}

/// Where a hook was called in the render CFG.
///
/// `block_id` is the block containing the hook's binding statement (StateVal,
/// MemoVal, CallbackVal) in the render CFG.  For Effect hooks, which emit no
/// statement in the render CFG, `block_id` defaults to the CFG entry.
#[derive(Debug, Clone)]
pub struct HookCallInfo {
    pub label: HookLabel,
    pub kind: HookKind,
    pub block_id: BlockId,
    /// Source location of the hook call site (e.g. `useState(0)`), if available.
    pub span: Option<SourceRange>,
}

/// Captured information about a JSX event handler entry point.
#[derive(Debug, Clone)]
pub struct HandlerInfo {
    pub label: HookLabel,
    /// DOM event name without "on" prefix, lowercased: "click", "change"…
    pub event: String,
    /// Variables used in the handler body but not locally defined within it.
    pub free_vars: HashSet<Var>,
    /// Source location of the JSX `onX={fn}` prop, if available.
    pub span: Option<SourceRange>,
}

/// Captured information about a hook body with deps (useEffect, useMemo, useCallback)
/// for dep-checking rules.
#[derive(Debug, Clone)]
pub struct EffectInfo {
    pub label: HookLabel,
    /// Which hook this info came from (Effect / Memo / Callback).
    pub kind: HookKind,
    /// Access paths read in the body but not locally defined within it —
    /// member-chain granular (`x.a`, not just `x`) so `missing-deps` matches
    /// a dep against the exact field used (TODO.md F1b).
    pub free_paths: HashSet<AccessPath>,
    /// Deps array as declared by the caller (`[]` = empty, `None` = absent).
    pub declared_deps: Vec<Expr>,
    /// `true` when caller wrote an explicit deps array (even `[]`).
    /// `false` when no deps argument was passed (`deps: None`).
    /// Always `true` for Memo/Callback (their deps array is mandatory).
    pub has_deps_array: bool,
    /// Source location of the hook call site, if available.
    pub span: Option<SourceRange>,
}

#[derive(Debug, Clone)]
pub struct AnalysisResult<D: AbstractDomain> {
    /// Name of the component this result belongs to. Rules re-evaluating
    /// expressions against the result use it as the `AnalysisCtx` component
    /// (state-slot provenance).
    pub component: Symbol,
    pub state_store: StateStore<D>,
    pub memo_store: MemoStore<D>,
    /// Abstract environment at the *exit* of each render-CFG block.
    pub block_states: HashMap<BlockId, AbstractEnv<D>>,
    /// Abstract environment at the *exit* of each block, per effect body CFG.
    /// Populated at convergence (overwritten each iteration; last write is final).
    pub effect_block_states: HashMap<HookLabel, HashMap<BlockId, AbstractEnv<D>>>,
    pub hook_calls: Vec<HookCallInfo>,
    pub effect_info: HashMap<HookLabel, EffectInfo>,
    /// Abstract environment at the *exit* of each block, per JSX handler body CFG.
    /// Populated at each fixpoint iteration; last iteration's values survive.
    pub handler_block_states: HashMap<HookLabel, HashMap<BlockId, AbstractEnv<D>>>,
    pub handler_info: HashMap<HookLabel, HandlerInfo>,
    /// Labels whose state was widened to force convergence.
    pub widened_labels: HashSet<HookLabel>,
    /// Join of all values written to the state store by effects in the final fixpoint
    /// iteration, starting from ⊥ (i.e. excludes the pre-existing state value).
    ///
    /// Used by `InfiniteLoop` to distinguish a setter that writes a bounded value
    /// (branch narrowing held the growth) from one that truly diverges.
    /// `Bottom` for a label = effect never called that setter in the semantic analysis.
    pub effect_setter_writes: StateStore<D>,
    pub render_cfg: CFG,
    /// Original hook entries needed by rules that inspect effect body CFGs.
    pub hooks: Vec<HookEntry>,
    /// Number of outer fixpoint iterations before convergence.  Useful for
    /// --verbose output and for Info diagnostics about analysis depth.
    pub iterations: usize,
    /// Final heap after convergence: allocation-site → HeapValue (Fn/Obj/Arr).
    ///
    /// Primarily used by rules (e.g. `CrossSetterInRender`) to resolve Loc
    /// variables in `block_states` to their function bodies and captured envs.
    /// Defaults to `Heap::new()` for components analyzed without initial heap context.
    pub heap: Heap,
}

impl<D: AbstractDomain> AnalysisResult<D> {
    /// Join the abstract exit envs of all `Return`-terminated blocks.
    ///
    /// Uses `reduce` (not `fold(bottom, join)`) since `bottom.join(env)` maps
    /// any key not in `bottom` to `D::top()`, making bottom a non-identity.
    pub fn exit_env(&self) -> AbstractEnv<D> {
        self.render_cfg
            .blocks
            .values()
            .filter(|b| matches!(b.term, Terminator::Return(_)))
            .filter_map(|b| self.block_states.get(&b.id))
            .cloned()
            .reduce(|acc, env| acc.join(&env))
            .unwrap_or_else(AbstractEnv::bottom)
    }
}
