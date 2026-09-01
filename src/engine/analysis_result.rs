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
        hooks::{DepsArg, DepsList, HookEntry},
        types::{BlockId, HookLabel, Symbol, Var},
    },
};

/// Provenance of a forced widening (ADR-019): which fixpoint iteration gave
/// up on convergence for a slot, and which effects were writing it then.
/// Feeds the `infinite-loop` / `widening-info` witness chains (`Step::Widen`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WidenEvent {
    /// Outer fixpoint iteration at which the slot was widened (first time).
    pub iteration: usize,
    /// Effects whose pass wrote this slot during the widening iteration.
    /// Empty when the growth came from render or handlers only.
    pub writers: Vec<HookLabel>,
}

/// What kind of symbol was inlined into a component's CFG (ADR-019).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineKind {
    /// A custom hook expanded by `expand_custom_hooks`.
    Hook,
    /// A utility function spliced by `expand_utility_calls`.
    Utility,
}

/// One symbol inlined into the component during analysis: feeds
/// `Step::Resolve` witness steps ("`useMedia` was inlined from ./hooks.ts"),
/// and lets rules know spans inside the CFG may point into `from`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineOrigin {
    pub name: String,
    /// File the inlined body came from.
    pub from: std::path::PathBuf,
    pub kind: InlineKind,
}

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
    /// The engine has no information about this hook's internals — it could
    /// neither inline a body nor apply a summary. What
    /// `analysis-limit/unknown-hook` reports.
    ///
    /// Distinct from `kind == Custom`, which is what that Info used to key on:
    /// a *summarized* library hook is a custom hook whose abstraction is known,
    /// and it keeps its row so rules-of-hooks checks can see the call site.
    pub opaque: bool,
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
    /// Deps argument as the IR could read it — see [`DepsArg`], whose three
    /// states are three different facts. One field rather than a list plus a
    /// present-flag: the two could disagree, and the reading they used to
    /// disagree about (an unreadable argument shown as an empty array that is
    /// definitely present) was wrong in both directions.
    pub deps: DepsArg,
    /// Source location of the hook call site, if available.
    pub span: Option<SourceRange>,
}

impl EffectInfo {
    /// `true` when the caller passed a deps argument at all — a written `[]`,
    /// and also one the engine could not read. Only the absent argument makes
    /// a hook re-run on every render, and that is the question this answers.
    pub fn has_deps_array(&self) -> bool {
        self.deps.is_declared()
    }

    /// `true` when the hook is gated by a deps list the engine cannot see one
    /// element of. Such a hook can go stale exactly like one with an
    /// incomplete array, so a rule must check it with *nothing* covered rather
    /// than skip it.
    pub fn deps_are_opaque(&self) -> bool {
        matches!(self.deps, DepsArg::Opaque)
    }

    /// Every deps element the IR can see, in declared order; empty when there
    /// is no readable list. Use it to make a rule **fire** — each element
    /// over-approximates what it stands for, so a truncated list is safe here.
    /// To make a rule *stop*, use [`EffectInfo::covering_deps`] instead.
    pub fn declared_deps(&self) -> &[Expr] {
        self.deps.list().map_or(&[], DepsList::as_slice)
    }

    /// The entries that actually cover a read — [`DepsList::covering`], and
    /// empty when no list was written. This is what a suppression must ask
    /// for: a flattened `[...rows]` declares `rows[0], rows[1], …` and never
    /// `rows`, so crediting the source would silence a stale capture. The
    /// elements written beside the spread still cover their own reads.
    pub fn covering_deps(&self) -> std::borrow::Cow<'_, [Expr]> {
        self.deps
            .list()
            .map_or(std::borrow::Cow::Borrowed(&[][..]), DepsList::covering)
    }

    /// How many dependencies the source array declares, when the engine knows.
    /// `None` for an absent or unreadable argument — neither has an arity to
    /// compare against. An elision keeps the arity exact; only a spread leaves
    /// a lower bound, which [`EffectInfo::deps_at_least`] carries.
    pub fn deps_arity(&self) -> Option<usize> {
        self.deps.list().and_then(|l| l.arity.exact())
    }

    /// The guaranteed lower bound on the declared arity, when a list was
    /// written. Always available for a list, and what lets an open-ended one
    /// still refute an arity claim instead of refusing every one.
    pub fn deps_at_least(&self) -> Option<usize> {
        self.deps.list().map(|l| l.arity.at_least())
    }
}

#[derive(Debug, Clone)]
pub struct AnalysisResult<D: AbstractDomain> {
    /// Name of the component this result belongs to. Rules re-evaluating
    /// expressions against the result use it as the `AnalysisCtx` component
    /// (state-slot provenance).
    pub component: Symbol,
    /// The component's defining file — registry-resolution key for witness
    /// producers (`witness::resolve_and_classify`, ADR-019). Empty for
    /// hand-built IR (unit tests).
    pub file: std::path::PathBuf,
    /// The component's props parameter binding (`props`, or the `__pN` temp
    /// for a destructured parameter). Root of prop-owned objects for rules
    /// that chase reference identity (state-mutation).
    pub param: Var,
    /// Props whose declared TypeScript type is a DOM interface — mutating
    /// them is imperative DOM manipulation, exempt from state-mutation.
    pub dom_props: std::sync::Arc<HashSet<Var>>,
    /// The file's module-level `const` bindings — the same table the engine
    /// seeded the initial env from, carried through so the rules layer reads
    /// one source of truth. Its `Context` rows are the only proof available
    /// that `<X.Provider>` is a React context provider. Empty for hand-built IR.
    pub module_consts: std::sync::Arc<HashMap<Var, crate::ir::ModuleConstInit>>,
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
    /// Labels whose state was widened to force convergence, with the
    /// provenance of each widening (iteration, writing effects) — ADR-019.
    pub widen_trace: HashMap<HookLabel, WidenEvent>,
    /// Symbols (custom hooks, utilities) inlined into this component's CFGs
    /// during analysis, with their source file (ADR-019).
    pub inline_origins: Vec<InlineOrigin>,
    /// Join of all values written to the state store by effects in the final fixpoint
    /// iteration, starting from ⊥ (i.e. excludes the pre-existing state value).
    ///
    /// Used by `InfiniteLoop` to distinguish a setter that writes a bounded value
    /// (branch narrowing held the growth) from one that truly diverges.
    /// `Bottom` for a label = effect never called that setter in the semantic analysis.
    pub effect_setter_writes: StateStore<D>,
    /// Joined abstract return value of each inline `FnLit` argument of an
    /// unexpanded custom hook, keyed by `(hook label, argument index)`
    /// (ADR-023 §3 amendment: computed during analysis, where the context
    /// exists; `api/query.rs` owns only the verdict type and the reader).
    ///
    /// Program-point argument: the body runs with its params bound to ⊤ and
    /// only module consts in scope — every other capture reads the env-miss
    /// default (⊤), which over-approximates the value at *any* program point,
    /// so no invocation timing can make the stored value an under-approximation.
    /// Absent key = not an inline `FnLit` (Var-bound, imported) → `Unknown`.
    pub custom_arg_returns: HashMap<(HookLabel, usize), D>,
    pub render_cfg: CFG,
    /// Original hook entries needed by rules that inspect effect body CFGs.
    pub hooks: Vec<HookEntry>,
    /// Provenance row per hook call in `hooks` (ADR-023 step 1):
    /// `label → (origin hook, source, direct|inlined)`. Inlined custom hooks'
    /// rows are merged in by `expand_custom_hooks` with `inlined: true`, so a
    /// rule can tell a direct `useLayoutEffect` call from one reached through
    /// a wrapper. Empty for hand-built IR.
    pub hook_provenance: Vec<crate::ir::hooks::HookProvenance>,
    /// The slot → writers relation (ADR-027 §1): one row per (region,
    /// alias-resolved setter variable, sync-vs-nested) with a witness span,
    /// `region` lexical-exact and `phase` a MAY verdict (⊤ = `Unknown`).
    /// Computed once at convergence over the post-expansion CFGs. Empty for
    /// hand-built IR.
    pub slot_writers: Vec<crate::engine::setters::SlotWriter>,
    /// The slot → seeds relation (#106, ADR-031): one row per (state slot,
    /// prop path its `useState` initializer reads), carrying a syntactic sync
    /// verdict folded from `slot_writers` and the effects' declared deps.
    /// Computed at convergence in the same slice as `slot_writers`. Empty for
    /// hand-built IR.
    pub slot_seeds: Vec<crate::engine::seeds::SlotSeed>,
    /// The callback-registration relation (#111, ADR-034): one row per call in
    /// an effect body that hands a callback to something outliving the effect,
    /// carrying the registrar, its firing and timing columns, the callback as
    /// written, and whether the effect's cleanup tears it back down. Computed
    /// at convergence in the same slice as `slot_writers`. Empty for
    /// hand-built IR.
    pub registrations: Vec<crate::engine::registrations::Registration>,
    /// Number of outer fixpoint iterations before convergence.  Useful for
    /// --verbose output and for Info diagnostics about analysis depth.
    pub iterations: usize,
    /// Final heap after convergence: allocation-site → HeapValue (Fn/Obj).
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
