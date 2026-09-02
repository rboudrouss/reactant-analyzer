//! Setter machinery shared by the engine and the rules (ADR-027 §1): finding
//! setter calls (through FnLit bodies and local wrappers), mapping
//! hook-value/setter variables to their state labels, resolving `let s = setX`
//! alias chains, the may-written slot proof — and the slot-writer relation
//! (`collect_slot_writers`) the fixpoint stores on `AnalysisResult`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::{
    domains::{
        AbstractEnv, StateValue,
        stores::{EnvVal, Heap, HeapValue},
    },
    engine::AnalysisResult,
    ir::{
        SourceRange,
        cfg::{CFG, Terminator},
        expr::Expr,
        free_vars::collect_used_vars,
        hooks::HookEntry,
        stmt::Stmt,
        types::{BlockId, HookLabel, Symbol, Var},
    },
};

/// A setter call found by `collect_setter_calls`.
#[derive(Debug, Clone)]
pub struct SetterCall {
    pub var: Var,
    pub span: Option<SourceRange>,
    /// Block in the top-level CFG where the call was found.
    /// `None` when the call is inside a nested `FnLit` body dominance unknowable.
    pub block_id: Option<BlockId>,
    /// Phase class of the retained site. The collapse below keeps the most
    /// synchronous site per variable, so this is the strongest reading of
    /// "when does this setter run" the walk found — and the reason a consumer
    /// can now tell an effect-body write from one only a keydown reaches
    /// (ADR-034 §4, #93). It used to be computed and thrown away.
    pub class: SetterCallPhase,
}

/// The externally-visible half of the walk's phase classification. A subset:
/// only the distinction consumers of the collapsed row are allowed to make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetterCallPhase {
    /// Runs when the enclosing body runs.
    Sync,
    /// Runs only on an external event — a registered DOM listener, a reified
    /// JSX handler. A user event stands between the body and this write.
    Handler,
    /// Anything else: deferred, in a cleanup, or unclassified (⊤).
    Other,
}

impl From<WalkClass> for SetterCallPhase {
    fn from(c: WalkClass) -> Self {
        match c {
            WalkClass::Sync => SetterCallPhase::Sync,
            WalkClass::Handler => SetterCallPhase::Handler,
            _ => SetterCallPhase::Other,
        }
    }
}

/// Collect all setter variable names called in `cfg` together with their
/// call-site span and block ID, descending into FnLit argument bodies and
/// variable-bound FnLits up to `max_depth` levels.
pub fn collect_setter_calls(
    cfg: &CFG,
    setter_vars: &HashSet<Var>,
    max_depth: usize,
) -> Vec<SetterCall> {
    collect_setter_calls_with_extra(cfg, setter_vars, max_depth, &HashMap::new())
}

/// Like `collect_setter_calls` but merges `extra_fn_bindings` so that variable
/// callbacks defined outside `cfg` are resolved. `cfg`-local entries take precedence.
pub fn collect_setter_calls_with_extra(
    cfg: &CFG,
    setter_vars: &HashSet<Var>,
    max_depth: usize,
    extra_fn_bindings: &HashMap<Var, Arc<CFG>>,
) -> Vec<SetterCall> {
    let mut fn_bindings = collect_fn_bindings(cfg);
    for (k, v) in extra_fn_bindings {
        fn_bindings
            .entry(k.clone())
            .or_insert_with(|| Arc::clone(v));
    }
    let mut found: Found = Vec::new();
    let empty = HashSet::new();
    let certified = certified_fn_names(cfg, &fn_bindings);
    let mut walk = SetterWalk {
        setter_vars,
        fn_bindings: &fn_bindings,
        walking: HashSet::new(),
        root: cfg as *const CFG as usize,
        effect_body: false,
        shadowed: &empty,
        repeating: false,
        callback_bodies: &HashMap::new(),
        certified_fns: &certified,
    };
    walk.cfg(cfg, max_depth, &mut found, WalkClass::Sync, None);
    // Collapse to the historical one-row-per-var shape, preferring the sync
    // site: its block id serves dominance, where any other class records
    // `None`. The walk itself no longer collapses, so this is the one place
    // the old granularity still exists — and the only consumer that wants it.
    found.sort_by_key(|s| s.class); // WalkClass derives Ord with Sync first
    let mut by_var: HashMap<Var, (Option<SourceRange>, Option<BlockId>, WalkClass)> =
        HashMap::new();
    for site in &found {
        by_var
            .entry(site.var.clone())
            .or_insert((site.span, site.block_id, site.class));
    }
    by_var
        .into_iter()
        .map(|(var, (span, block_id, class))| SetterCall {
            var,
            span,
            block_id,
            class: class.into(),
        })
        .collect()
}

/// One raw write site: the setter variable, a witness span, the phase class
/// the walk context assigned (ADR-027 §1-§2), and the top-level block the
/// walk descended from (region membership for provenance, ADR-027 §4).
#[derive(Debug, Clone)]
pub(crate) struct WriteSite {
    pub var: Var,
    pub span: Option<SourceRange>,
    pub class: WalkClass,
    pub prov_block: Option<BlockId>,
    /// The site may run many times per tick (a sync HOF callback).
    pub repeats: bool,
    pub updater: Updater,
}

/// Every write site in `cfg`, one row per call site — no collapse of any
/// kind. The slot-writer relation needs the distinct rows: a slot written
/// twice in one body is a different fact from a slot written once.
///
/// `outer_fns` carries function bindings certified in an enclosing scope — a
/// handler body is extracted out of the render CFG, so a `set(inc)` whose
/// `inc` was defined in render is invisible to a walk of the body alone. A
/// name the walked body binds itself drops out of the set, fail-closed, the
/// same device as `shadowed`.
pub(crate) fn collect_write_sites(
    cfg: &CFG,
    setter_vars: &HashSet<Var>,
    max_depth: usize,
    effect_body: bool,
    shadowed: &HashSet<Var>,
    outer_fns: &HashMap<Var, Arc<CFG>>,
    callback_bodies: &HashMap<HookLabel, Arc<CFG>>,
) -> Vec<WriteSite> {
    let mut found: Found = Vec::new();
    let mut fn_bindings = collect_fn_bindings(cfg);
    let mut certified = certified_fn_names(cfg, &fn_bindings);
    let mut local_binders: HashSet<Var> = HashSet::new();
    collect_binders(cfg, &mut local_binders);
    for (v, body) in outer_fns {
        if local_binders.contains(v) {
            continue;
        }
        fn_bindings
            .entry(v.clone())
            .or_insert_with(|| Arc::clone(body));
        certified.insert(v.clone());
    }
    let mut walk = SetterWalk {
        setter_vars,
        fn_bindings: &fn_bindings,
        walking: HashSet::new(),
        root: cfg as *const CFG as usize,
        effect_body,
        shadowed,
        repeating: false,
        callback_bodies,
        certified_fns: &certified,
    };
    walk.cfg(cfg, max_depth, &mut found, WalkClass::Sync, None);
    found
        .into_iter()
        .map(|s| WriteSite {
            var: s.var,
            span: s.span,
            class: s.class,
            prov_block: s.prov_block,
            repeats: s.repeats,
            updater: s.updater,
        })
        .collect()
}

/// Collect variables in `cfg` whose abstract value at any block exit is
/// `ComponentSetter { component, label }`, or whose Loc in the heap points to a
/// FnLit that captures a ComponentSetter (e.g. `() => setCount(0)` passed as prop).
///
/// Returns `var → (component, label)`.
///
/// Used by cross-component rules to find props that are parent setters.
///
/// **Blocks are visited in `CFG::blocks` order, not `block_states` order.**
/// The first env that resolves a var decides its row, and `block_states` is a
/// `HashMap` — reading it directly made the answer depend on Rust's per-process
/// hash seed whenever two blocks disagreed (#120). Program order is the one
/// stable order available here. Which block *should* win is a separate
/// question, filed as #119.
pub(crate) fn collect_component_setter_vars(
    cfg: &CFG,
    block_states: &HashMap<BlockId, AbstractEnv<StateValue>>,
    heap: &Heap,
) -> HashMap<Var, (Symbol, HookLabel)> {
    let mut var_names: HashSet<Var> = HashSet::new();
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { var, .. } | Stmt::Assign { var, .. } => {
                    var_names.insert(var.clone());
                }
                _ => {}
            }
        }
    }

    let mut var_names: Vec<Var> = var_names.into_iter().collect();
    var_names.sort();

    let mut result: HashMap<Var, (Symbol, HookLabel)> = HashMap::new();
    for env in cfg.blocks.keys().filter_map(|b| block_states.get(b)) {
        for var in &var_names {
            if result.contains_key(var) {
                continue;
            }
            // Direct component-setter value (exact setter slot).
            if let Some((component, label)) = env.lookup(var).as_setter() {
                result.insert(var.clone(), (component.clone(), *label));
                continue;
            }
            // Loc pointing to a FnLit that captures a ComponentSetter
            // (e.g. the parent passed `() => setCount(0)` as a prop).
            // Allocation sites and captures are hash sets/maps, and the first
            // setter found wins — so both are walked in sorted order (#120).
            if let Some(EnvVal::Loc { ids, .. }) = env.lookup_env_val(var) {
                let mut ids: Vec<_> = ids.iter().copied().collect();
                ids.sort();
                for id in ids {
                    if let Some(HeapValue::Fn { captured, .. }) = heap.get(id) {
                        let mut caps: Vec<_> = captured.iter().collect();
                        caps.sort_by_key(|(k, _)| *k);
                        for (_, val) in caps {
                            if let Some((component, label)) = val.as_setter() {
                                result.insert(var.clone(), (component.clone(), *label));
                                break;
                            }
                        }
                    }
                    if result.contains_key(var) {
                        break;
                    }
                }
            }
        }
    }
    result
}

/// Cross-component setter props: the [`collect_component_setter_vars`] result
/// restricted to setters owned by a component *other* than `component`. A
/// component passing its own setter down as a prop is not a cross-component
/// write, so self-owned entries are filtered out. Shared by the two rules that
/// reason about parent setters called in render (`infinite-loop`,
/// `setter-in-render`).
pub(crate) fn cross_component_setters(
    comp: &AnalysisResult<StateValue>,
    component: &Symbol,
) -> HashMap<Var, (Symbol, HookLabel)> {
    collect_component_setter_vars(&comp.render_cfg, &comp.block_states, &comp.heap)
        .into_iter()
        .filter(|(_, (parent_comp, _))| parent_comp != component)
        .collect()
}

/// Scan all Let stmts in `cfg` for `let X = FnLit{...}` and return X → body_cfg.
/// The subset of `bindings` whose name is bound exactly once, to a function
/// literal, and never re-bound *anywhere below* — including inside nested
/// closures, which `fn_binding_in` alone does not scan.
///
/// `Functional` is a must-claim sitting on a suppression path, so it takes the
/// strong bar: a name a nested callback reassigns is not the function it was
/// bound to by the time the write runs.
fn certified_fn_names(cfg: &CFG, bindings: &HashMap<Var, Arc<CFG>>) -> HashSet<Var> {
    bindings
        .keys()
        .filter(|v| crate::ir::bindings::certified_fn_binding(v, cfg, &[]).is_some())
        .cloned()
        .collect()
}

pub(crate) fn collect_fn_bindings(cfg: &CFG) -> HashMap<Var, Arc<CFG>> {
    let mut map: HashMap<Var, Arc<CFG>> = HashMap::new();
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            if let Stmt::Let {
                var,
                rhs: Expr::FnLit { body_cfg, .. },
                ..
            } = stmt
            {
                map.insert(var.clone(), Arc::clone(body_cfg));
            }
        }
    }
    map
}

/// Every name bound by a `Let`/`Assign` or an `FnLit` parameter anywhere in
/// `cfg`, nested `FnLit` bodies included — the shadow set that disables
/// deferring-global summaries (fail-closed).
pub(crate) fn collect_binders(cfg: &CFG, out: &mut HashSet<Var>) {
    fn exprs(e: &Expr, out: &mut HashSet<Var>) {
        if let Expr::FnLit {
            params, body_cfg, ..
        } = e
        {
            out.extend(params.iter().cloned());
            collect_binders(body_cfg, out);
            return;
        }
        e.for_each_child(&mut |c| exprs(c, out));
    }
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { var, rhs, .. } | Stmt::Assign { var, rhs, .. } => {
                    out.insert(var.clone());
                    exprs(rhs, out);
                }
                Stmt::MemberWrite { obj, rhs, .. } => {
                    exprs(obj, out);
                    exprs(rhs, out);
                }
                Stmt::ExprStmt(e, _) => exprs(e, out),
            }
        }
        match &block.term {
            Terminator::Return(e) | Terminator::Branch { cond: e, .. } => exprs(e, out),
            _ => {}
        }
    }
}

/// Extend a `setter var → state label` map with alias `let a = b` bindings in
/// `var → state label` for every `let var = useState(...)[1]` (the setter) in
/// `cfg`. The render body's authoritative setter-name → label map; pass it as
/// the `base` of [`resolve_setter_aliases`].
pub(crate) fn setter_var_labels(cfg: &CFG) -> HashMap<Var, HookLabel> {
    state_binding_labels(cfg, |rhs| match rhs {
        Expr::StateSetter(label) => Some(*label),
        _ => None,
    })
}

/// `var → state label` for every `let var = useState(...)[0]` (the value) in `cfg`.
pub(crate) fn state_val_labels(cfg: &CFG) -> HashMap<Var, HookLabel> {
    state_binding_labels(cfg, |rhs| match rhs {
        Expr::StateVal(label) => Some(*label),
        _ => None,
    })
}

/// `var → memo label` for every `let var = useMemo/useCallback(...)` in `cfg`.
/// The render env binds these BEFORE the memo store is recomputed, so their
/// env value can be stale ⊤ — rules needing memo values must go through the
/// memo store, keyed by this map.
pub(crate) fn memo_val_labels(cfg: &CFG) -> HashMap<Var, HookLabel> {
    state_binding_labels(cfg, |rhs| match rhs {
        Expr::MemoVal(label) | Expr::CallbackVal(label) => Some(*label),
        _ => None,
    })
}

/// `var → hook label` for every `let var = <hook call>` in `cfg`, whatever the
/// kind: the value slot of a `useState`, a memo, a callback, and — through
/// `HookMarker`, which every other hook binds — a ref or a custom hook's
/// return. This is the "what does the source call this hook" table; a label
/// with no `hook_calls` row can never be looked up in it.
pub(crate) fn hook_val_labels(cfg: &CFG) -> HashMap<Var, HookLabel> {
    state_binding_labels(cfg, |rhs| match rhs {
        Expr::StateVal(label)
        | Expr::MemoVal(label)
        | Expr::CallbackVal(label)
        | Expr::HookMarker(label, _) => Some(*label),
        _ => None,
    })
}

/// Shared kernel: collect `var → label` for `let var = <rhs>` where `pick`
/// extracts a label from the rhs.
fn state_binding_labels(
    cfg: &CFG,
    pick: impl Fn(&Expr) -> Option<HookLabel>,
) -> HashMap<Var, HookLabel> {
    let mut map = HashMap::new();
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            if let Stmt::Let { var, rhs, .. } = stmt
                && let Some(label) = pick(rhs)
            {
                map.insert(var.clone(), label);
            }
        }
    }
    map
}

/// `cfg` (b a known setter ⇒ a is too). Iterates to a fixpoint so chains
/// `let s1 = setX; let s2 = s1` all resolve.
///
/// Utility inlining binds setter params via such aliases (`let setter = setX`)
/// inside spliced bodies; rules matching setters by name must follow them or
/// the spliced setter call goes unseen (false negative).
pub(crate) fn resolve_setter_aliases(
    cfg: &CFG,
    base: &HashMap<Var, HookLabel>,
) -> HashMap<Var, HookLabel> {
    let mut map = base.clone();
    loop {
        let mut changed = false;
        for block in cfg.blocks.values() {
            for stmt in &block.stmts {
                // `let s = setX` and `s = setX` both alias the setter — mirror
                // the interpreter's `bind_rhs`, which treats Let/Assign alike.
                let alias = match stmt {
                    Stmt::Let {
                        var,
                        rhs: Expr::Var(src),
                        ..
                    }
                    | Stmt::Assign {
                        var,
                        rhs: Expr::Var(src),
                        ..
                    } => Some((var, src)),
                    _ => None,
                };
                if let Some((var, src)) = alias
                    && !map.contains_key(var)
                    && let Some(&label) = map.get(src)
                {
                    map.insert(var.clone(), label);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    map
}

/// Alias-resolved `setter var → state label` across the render body and every
/// hook body. Utility inlining binds setter params via aliases (`let setter =
/// setX`) inside spliced bodies; rules matching a setter by name must follow
/// those aliases through every body or a spliced setter call goes unseen (false
/// negative). The shared recipe of `derived-state`, `state-mutation`,
/// `stale-closure` and `frozen-initial-state`.
pub(crate) fn all_setter_labels(comp: &AnalysisResult<StateValue>) -> HashMap<Var, HookLabel> {
    let mut labels = setter_var_labels(&comp.render_cfg);
    for cfg in
        std::iter::once(&comp.render_cfg).chain(comp.hooks.iter().filter_map(|h| h.body_cfg()))
    {
        labels = resolve_setter_aliases(cfg, &labels);
    }
    labels
}

// ── The slot-writer relation (ADR-027 §1) ─────────────────────────────────────

/// Lexical region a write sits in: which body of the component holds the
/// call. Exact by construction — a fact about the code, never about timing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WriterRegion {
    Render,
    Effect(HookLabel),
    Memo(HookLabel),
    Callback(HookLabel),
    Handler(HookLabel),
}

impl WriterRegion {
    /// The word rules and templates print.
    pub fn word(self) -> &'static str {
        match self {
            WriterRegion::Render => "render",
            WriterRegion::Effect(_) => "effect",
            WriterRegion::Memo(_) => "memo",
            WriterRegion::Callback(_) => "callback",
            WriterRegion::Handler(_) => "handler",
        }
    }

    /// The phase of a write that runs synchronously in this region — there,
    /// lexis = execution, provably.
    ///
    /// That claim was false past an `await` until #117: lowering erased the
    /// expression, so a post-await write kept its region's sync phase. The walk
    /// now switches to `Deferred` on entering a post-await block
    /// ([`CFG::post_await_blocks`]), so the callers of this function really do
    /// run synchronously.
    fn sync_phase(self) -> WriterPhase {
        match self {
            WriterRegion::Render => WriterPhase::Render,
            WriterRegion::Effect(_) => WriterPhase::Effect,
            WriterRegion::Memo(_) => WriterPhase::Memo,
            WriterRegion::Callback(_) => WriterPhase::Callback,
            WriterRegion::Handler(_) => WriterPhase::Handler,
        }
    }
}

/// Execution phase of a write — a MAY verdict (ADR-027 §1): a write
/// synchronous in its body carries that body's phase; a write inside a
/// nested `FnLit` is `Unknown` (⊤ — it may run in any phase) until a callee
/// summary sharpens it (ADR-027 §2). Classifying every nested callback as
/// "deferred" instead would under-approximate: `arr.forEach(x => setX(x))`
/// inside an effect runs synchronously in the effect phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WriterPhase {
    Render,
    Effect,
    Memo,
    Callback,
    Handler,
    /// The callee summary proved deferral (ADR-027 §2): a timer, a microtask,
    /// a promise continuation — the write never runs inside a React phase.
    Deferred,
    /// An effect's returned cleanup function.
    Cleanup,
    /// ⊤ — the write may run in any phase. Satisfies every phase query.
    Unknown,
}

/// Argument 0 of a write, classified where the walk still has it.
///
/// One column, deliberately: the functional/non-functional classifier and the
/// body-purity classifier are two questions about the same expression, and
/// recording it twice would put two bespoke passes on one walk site — the
/// thing ADR-027 §4's one-relation rule exists to prevent.
#[derive(Debug, Clone)]
pub enum Updater {
    /// Proven a function literal: an inline `set(prev => …)`, or a variable
    /// bound exactly once to one and never re-bound. The body is kept for the
    /// consumers that need to look inside it.
    Functional(Arc<CFG>),
    /// ⊤ — everything else: a value expression, a call, an argument the walk
    /// could not resolve to a literal, or no argument at all. Only a *proven*
    /// function literal escapes this, so a rule keyed on "not functional"
    /// over-reports rather than missing a write.
    Unknown,
}

impl Updater {
    pub fn is_functional(&self) -> bool {
        matches!(self, Updater::Functional(_))
    }
}

/// One writer of a state slot, one row **per call site**.
///
/// The granularity used to be one row per (region, alias-resolved setter
/// variable, phase class), collapsing two `setCount(count + 1)` calls in one
/// body into a single row. That is reversed here, and the reversal is the
/// point: the canonical stale-read shape is *two* non-functional writes of one
/// slot in one handler, and a relation that cannot say there are two cannot
/// express it. Multiplying rows is monotone — same slots, same phases, more
/// witnesses — and every shipped consumer of the stored relation reads it
/// existentially, so nothing that matched before stops matching (ADR-028 §2).
#[derive(Debug, Clone)]
pub struct SlotWriter {
    pub slot: HookLabel,
    /// The setter variable at the call site (an alias chain resolves it to
    /// `slot`; a spliced wrapper's `setter#salt` param resolves too).
    pub setter: Var,
    /// Witness call-site span.
    pub span: Option<SourceRange>,
    pub region: WriterRegion,
    pub phase: WriterPhase,
    /// Whether the write is caller-authored or reached through inlined
    /// wrappers (ADR-027 §4).
    pub via: WriteProvenance,
    /// Argument 0 of this write.
    pub updater: Updater,
    /// MAY: another write of the same slot in this region is CFG-reachable
    /// from this one, so the two can land in the same tick — self-reachability
    /// through a back edge included, which is how a single write inside a loop
    /// pairs with itself.
    ///
    /// A per-row boolean, never a fold over the edge: precomputing it here is
    /// what keeps a Tier-A rule about co-executing writes single-anchor and
    /// existential, the same move that dissolved the effect+handler join in
    /// ADR-027 §1. It is may-typed in one direction only — the walk is
    /// depth-capped, so a write it never saw cannot make this `false` a
    /// promise, which is why no guard may assert the negative.
    pub same_tick: bool,
}

/// Provenance of one write site (ADR-027 §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteProvenance {
    /// Caller-authored: the site sits outside every recorded splice region.
    /// This is a certainty, not a may-fact — regions are recorded at the
    /// single splice primitive, exhaustively — which is what
    /// `must_direct_write` certifies.
    Direct,
    /// Reached through inlined wrappers, outermost first
    /// (`["putState", "helper"]` for `putState → helper → setX`). A write in
    /// an inlined custom hook's body carries the hook's origin name first.
    Via(Vec<Symbol>),
    /// The site could not be placed (a defensive splice path grafted
    /// statements outside any recordable range) — fails every provenance
    /// guard and certifies nothing.
    Unknown,
}

/// One spliced-callee block range in a CFG: `start..end` are the callee's
/// blocks (the join past them belongs to the caller), `name` the callee's
/// EXPORTED name (aliased imports resolve — ADR-027 §3), `parent` the region
/// the call site itself sat in (nesting = the wrapper chain).
#[derive(Debug, Clone)]
pub struct InlineRegion {
    pub start: BlockId,
    pub end: BlockId,
    pub name: Symbol,
    /// File the spliced body came from — the import-resolution context for
    /// calls the splice carried in (a wrapper's own helper call resolves
    /// through the WRAPPER's file, not the component's).
    pub from: std::path::PathBuf,
    pub parent: Option<usize>,
}

/// Every splice region of one component, per CFG.
#[derive(Debug, Default, Clone)]
pub struct InlineRegions {
    /// Regions in the render CFG (utility splices and custom-hook body
    /// splices both land here).
    pub render: Vec<InlineRegion>,
    /// Regions in a hook body CFG, by the OWNING hook's label.
    pub bodies: HashMap<HookLabel, Vec<InlineRegion>>,
    /// A splice fell back to entry-block grafting: render block ranges no
    /// longer cover every spliced statement, so `Direct` is unprovable in
    /// the render CFG — its unchained rows read `Unknown` (fail-closed).
    pub render_poisoned: bool,
}

impl InlineRegions {
    /// The wrapper chain for a site at `block` of `cfg` (`None` = render),
    /// outermost first; empty = outside every region.
    fn chain(&self, cfg: Option<HookLabel>, block: BlockId) -> Vec<Symbol> {
        let regions = match cfg {
            None => &self.render,
            Some(label) => match self.bodies.get(&label) {
                Some(r) => r,
                None => return Vec::new(),
            },
        };
        // Ranges are disjoint (each splice allocates strictly above), so at
        // most one region contains the block; parents encode the nesting.
        let mut at = regions
            .iter()
            .position(|r| r.start <= block && block < r.end);
        let mut chain: Vec<Symbol> = Vec::new();
        while let Some(i) = at {
            chain.push(regions[i].name.clone());
            at = regions[i].parent;
        }
        chain.reverse();
        chain
    }
}

/// Names bound exactly once, anywhere below, to the result of a `useCallback`
/// — mapped to that callback's body.
///
/// The same single-binding bar the literal certificate uses: a name any body
/// re-binds is not the memoized function by the time the write runs.
fn callback_bound_vars(
    render_cfg: &CFG,
    also: &[&CFG],
    callback_bodies: &HashMap<HookLabel, Arc<CFG>>,
) -> HashMap<Var, Arc<CFG>> {
    fn scan<'a>(cfg: &'a CFG, out: &mut HashMap<&'a str, Vec<&'a Expr>>) {
        for block in cfg.blocks.values() {
            for stmt in &block.stmts {
                if let Stmt::Let { var, rhs, .. } | Stmt::Assign { var, rhs, .. } = stmt {
                    out.entry(var.as_str()).or_default().push(rhs);
                    rhs.for_each_child(&mut |c| {
                        if let Expr::FnLit { body_cfg, .. } = c {
                            scan(body_cfg, out);
                        }
                    });
                    if let Expr::FnLit { body_cfg, .. } = rhs {
                        scan(body_cfg, out);
                    }
                }
            }
        }
    }
    let mut binds: HashMap<&str, Vec<&Expr>> = HashMap::new();
    scan(render_cfg, &mut binds);
    for cfg in also {
        scan(cfg, &mut binds);
    }
    binds
        .into_iter()
        .filter_map(|(name, rhs)| {
            let [only] = rhs.as_slice() else { return None };
            let Expr::CallbackVal(label) = only.peel_ts() else {
                return None;
            };
            callback_bodies
                .get(label)
                .map(|b| (name.to_string(), Arc::clone(b)))
        })
        .collect()
}

/// Two source sites are the same write when they name the same span in the
/// same region and class — which is what a local helper called twice produces,
/// since the walk pulls its inner site in once per call. The duplicates carry
/// a fact that must survive the collapse: being reached from two call sites is
/// co-execution.
fn dedup_source_sites(sites: &mut Vec<WriteSite>) {
    let mut seen: HashMap<(Var, SourceRange, WalkClass), usize> = HashMap::new();
    let mut out: Vec<WriteSite> = Vec::with_capacity(sites.len());
    for site in std::mem::take(sites) {
        // A site with no span cannot be identified with another; keep it.
        let Some(span) = site.span else {
            out.push(site);
            continue;
        };
        match seen.get(&(site.var.clone(), span, site.class)) {
            Some(&i) => out[i].repeats = true,
            None => {
                seen.insert((site.var.clone(), span, site.class), out.len());
                out.push(site);
            }
        }
    }
    *sites = out;
}

/// Forward reachability within one CFG, computed once per region.
///
/// A BFS per row was O(rows × blocks × edges), and `CFG::successors` is a
/// linear scan of the edge vector, so a component with many writers paid for
/// it twice over. One BFS per block answers every query by lookup.
struct Reachability {
    /// `from → every block reachable along at least one edge`. Starting at the
    /// successors rather than at the block itself is what makes a block reach
    /// itself only through a genuine cycle.
    forward: HashMap<BlockId, HashSet<BlockId>>,
}

impl Reachability {
    fn of(cfg: &CFG) -> Self {
        let succs: HashMap<BlockId, Vec<BlockId>> = cfg
            .blocks
            .keys()
            .map(|&bid| (bid, cfg.successors(bid)))
            .collect();
        let forward = cfg
            .blocks
            .keys()
            .map(|&start| {
                let mut seen: HashSet<BlockId> = HashSet::new();
                let mut queue: VecDeque<BlockId> =
                    succs.get(&start).cloned().unwrap_or_default().into();
                seen.extend(queue.iter().copied());
                while let Some(b) = queue.pop_front() {
                    for &succ in succs.get(&b).map(Vec::as_slice).unwrap_or(&[]) {
                        if seen.insert(succ) {
                            queue.push_back(succ);
                        }
                    }
                }
                (start, seen)
            })
            .collect();
        Reachability { forward }
    }

    fn reaches(&self, from: BlockId, to: BlockId) -> bool {
        self.forward.get(&from).is_some_and(|s| s.contains(&to))
    }
}

fn class_phase(class: WalkClass, region: WriterRegion) -> WriterPhase {
    match class {
        WalkClass::Sync => region.sync_phase(),
        WalkClass::Deferred => WriterPhase::Deferred,
        WalkClass::Handler => WriterPhase::Handler,
        WalkClass::Cleanup => WriterPhase::Cleanup,
        WalkClass::Unknown => WriterPhase::Unknown,
    }
}

/// The slot → writers relation, computed once at convergence over the
/// post-expansion CFGs and stored on `AnalysisResult` (ADR-027 §1).
///
/// A nested-`FnLit` render row that duplicates a site already reified as a
/// `Handler` entry (extraction copies the body, the `FnLit` stays in the
/// render CFG) is dropped in favor of the handler row: same call site — same
/// witness span — and the handler row's phase is what extraction proved.
/// Keeping the ⊤ duplicate would make `writer_phases includes <anything>`
/// true for every extracted `onClick={() => set(..)}`.
pub(crate) fn collect_slot_writers(
    render_cfg: &CFG,
    hooks: &[HookEntry],
    regions: &InlineRegions,
    hook_provenance: &[crate::ir::hooks::HookProvenance],
) -> Vec<SlotWriter> {
    let mut labels = setter_var_labels(render_cfg);
    for cfg in std::iter::once(render_cfg).chain(hooks.iter().filter_map(|h| h.body_cfg())) {
        labels = resolve_setter_aliases(cfg, &labels);
    }
    let setter_vars: HashSet<Var> = labels.keys().cloned().collect();
    // Any local binding of a deferring global's name disables its summary,
    // wherever the call sits (fail-closed across every body, nested `FnLit`
    // scopes included).
    let mut shadowed: HashSet<Var> = HashSet::new();
    for cfg in std::iter::once(render_cfg).chain(hooks.iter().filter_map(|h| h.body_cfg())) {
        collect_binders(cfg, &mut shadowed);
    }

    // A hook body's rows carry the hook's own origin first when the hook was
    // inlined — the body arrived through `expand_custom_hooks`, so every
    // write in it is wrapper-mediated.
    let inlined_origin = |label: HookLabel| -> Option<&Symbol> {
        hook_provenance
            .iter()
            .find(|p| p.label == label && p.inlined)
            .map(|p| &p.origin_hook)
    };

    // Render-scope function bindings that clear the single-binding bar across
    // every body below — the same certificate a consumer needs before it may
    // treat a name as the function it was bound to (ADR-023 §3, #103).
    let hook_bodies: Vec<&CFG> = hooks.iter().filter_map(|h| h.body_cfg()).collect();
    let callback_bodies: HashMap<HookLabel, Arc<CFG>> = hooks
        .iter()
        .filter_map(|h| match h {
            HookEntry::Callback {
                label, body_cfg, ..
            } => Some((*label, Arc::new(body_cfg.clone()))),
            _ => None,
        })
        .collect();
    let mut outer_fns: HashMap<Var, Arc<CFG>> = collect_fn_bindings(render_cfg)
        .into_iter()
        .filter(|(v, _)| {
            crate::ir::bindings::certified_fn_binding(v, render_cfg, &hook_bodies).is_some()
        })
        .collect();
    // `const inc = useCallback(…)` binds a `CallbackVal`, not an `FnLit`, so
    // the literal certificate never sees it — and a memoized updater read as ⊤
    // fired the non-functional rules on correct code.
    outer_fns.extend(callback_bound_vars(
        render_cfg,
        &hook_bodies,
        &callback_bodies,
    ));

    let mut out: Vec<SlotWriter> = Vec::new();
    let push_sites = |region: WriterRegion, cfg: &CFG, out: &mut Vec<SlotWriter>| {
        let (cfg_key, hook_origin) = match region {
            WriterRegion::Render => (None, None),
            WriterRegion::Effect(l)
            | WriterRegion::Memo(l)
            | WriterRegion::Callback(l)
            | WriterRegion::Handler(l) => (Some(l), inlined_origin(l)),
        };
        let effect_body = matches!(region, WriterRegion::Effect(_));
        let mut sites = collect_write_sites(
            cfg,
            &setter_vars,
            2,
            effect_body,
            &shadowed,
            &outer_fns,
            &callback_bodies,
        );
        // One source site is one row. A local helper called twice pulls its
        // inner write in twice, and both copies name the same source span —
        // the same write, not two. They collapse, but their facts join first:
        // being reached from two call sites is exactly co-execution.
        dedup_source_sites(&mut sites);

        // Where every SYNC write of each slot sits in this region's CFG. The
        // key is `prov_block`, never `block_id`: a site inside a nested body
        // records a block of *that* body's CFG, and block ids are per-CFG, so
        // resolving one in the region CFG answers about an unrelated block.
        // Only sync sites can co-execute within a tick — a deferred or handler
        // write is a separate turn by construction.
        let mut sync_blocks: HashMap<HookLabel, Vec<BlockId>> = HashMap::new();
        for s in &sites {
            if s.class == WalkClass::Sync
                && let (Some(&slot), Some(b)) = (labels.get(&s.var), s.prov_block)
            {
                sync_blocks.entry(slot).or_default().push(b);
            }
        }
        let reach = Reachability::of(cfg);
        for site in sites {
            let Some(&slot) = labels.get(&site.var) else {
                continue;
            };
            let same_tick = site.repeats
                || match (site.class, site.prov_block) {
                    (WalkClass::Sync, Some(b)) => {
                        let blocks = sync_blocks.get(&slot).map_or(&[][..], Vec::as_slice);
                        // Co-execution is symmetric — "these two land in the
                        // same tick" does not care which runs first — so the
                        // question is reachability in EITHER direction, plus
                        // the same block, plus this block through a back edge.
                        blocks.iter().filter(|t| **t == b).count() > 1
                            || reach.reaches(b, b)
                            || blocks
                                .iter()
                                .any(|&t| reach.reaches(b, t) || reach.reaches(t, b))
                    }
                    _ => false,
                };
            let updater = site.updater.clone();
            let via = {
                let mut chain: Vec<Symbol> = hook_origin.into_iter().cloned().collect();
                match site.prov_block {
                    Some(b) => chain.extend(regions.chain(cfg_key, b)),
                    // No placeable block — never claim Direct on it.
                    None if chain.is_empty() => {
                        out.push(SlotWriter {
                            slot,
                            setter: site.var,
                            span: site.span,
                            region,
                            phase: class_phase(site.class, region),
                            via: WriteProvenance::Unknown,
                            updater,
                            same_tick,
                        });
                        continue;
                    }
                    None => {}
                }
                if chain.is_empty() {
                    // The defensive entry-graft poisons Direct in the render
                    // CFG (fail-closed).
                    if cfg_key.is_none() && regions.render_poisoned {
                        WriteProvenance::Unknown
                    } else {
                        WriteProvenance::Direct
                    }
                } else {
                    WriteProvenance::Via(chain)
                }
            };
            out.push(SlotWriter {
                slot,
                setter: site.var,
                span: site.span,
                region,
                phase: class_phase(site.class, region),
                via,
                updater,
                same_tick,
            });
        }
    };

    push_sites(WriterRegion::Render, render_cfg, &mut out);
    for hook in hooks {
        let (region, body) = match hook {
            HookEntry::Effect {
                label, body_cfg, ..
            } => (WriterRegion::Effect(*label), body_cfg),
            HookEntry::Memo {
                label, body_cfg, ..
            } => (WriterRegion::Memo(*label), body_cfg),
            HookEntry::Callback {
                label, body_cfg, ..
            } => (WriterRegion::Callback(*label), body_cfg),
            HookEntry::Handler {
                label, body_cfg, ..
            } => (WriterRegion::Handler(*label), body_cfg),
            _ => continue,
        };
        push_sites(region, body, &mut out);
    }

    out.sort_by(|a, b| {
        let key = |w: &SlotWriter| {
            (
                w.slot,
                w.region,
                w.phase,
                w.span.map_or((u32::MAX, u32::MAX), |s| s.pos_key()),
                w.setter.clone(),
            )
        };
        key(a).cmp(&key(b))
    });
    out
}

/// Threads the walk's fixed context (`setter_vars`, `fn_bindings`) and its
/// expansion stack through the mutually recursive CFG/stmt/expr descent, so
/// each step only takes what actually varies per call.
///
/// `walking` is the set of CFGs on the current expansion *stack*, keyed by
/// identity. It must stay a stack (pushed on entry, popped on exit), not a
/// global visited set: a body first reached with no depth left and later with
/// budget to spare has to be walked again, so a global set would lose findings.
/// Skipping only re-entrant walks loses none — a cycle re-enters a body at a
/// budget no larger than the one it is already being walked at, so the spliced
/// cycle-free path reaches the same CFGs and `found` only ever grows.
struct SetterWalk<'a> {
    setter_vars: &'a HashSet<Var>,
    fn_bindings: &'a HashMap<Var, Arc<CFG>>,
    walking: HashSet<usize>,
    /// The walk's outermost CFG (pointer identity) — where effect cleanup
    /// returns and reified listeners are recognized.
    root: usize,
    /// The walked root is an effect body: its top-level `addEventListener`
    /// listeners were reified as Handler entries (`extract_subscriptions`),
    /// and its returned function is its cleanup.
    effect_body: bool,
    /// Locally-bound names that disable a deferring-global summary
    /// (fail-closed: `let setTimeout = …` anywhere in the component makes
    /// the bare name mean nothing).
    shadowed: &'a HashSet<Var>,
    /// The walk is currently inside a callback a synchronous higher-order
    /// function runs, so anything it finds may execute many times per tick.
    repeating: bool,
    /// Bodies of the component's `useCallback` hooks, by label — a memoized
    /// function is as proven a function literal as an inline one, and reading
    /// it as ⊤ fired the non-functional rules on correct code.
    callback_bodies: &'a HashMap<HookLabel, Arc<CFG>>,
    /// Names bound exactly once, to a function literal, in the walked root —
    /// the bar a `set(fn)` argument must clear before it counts as a proven
    /// functional updater. `collect_fn_bindings` is not that bar: it keeps the
    /// last binding of a re-bound name.
    certified_fns: &'a HashSet<Var>,
}

impl SetterWalk<'_> {
    /// Does this callee run its function argument once per element?
    fn is_sync_hof(&self, fn_: &Expr) -> bool {
        matches!(fn_, Expr::FieldAccess { field, .. } if SYNC_HOF_METHODS.contains(&field.as_str()))
    }

    /// Argument 0 of a write, claimed `Functional` only when it is provably a
    /// function literal.
    fn updater_of(&self, args: &[Expr]) -> Updater {
        match args.first().map(Expr::peel_ts) {
            Some(Expr::FnLit { body_cfg, .. }) => Updater::Functional(Arc::clone(body_cfg)),
            Some(Expr::CallbackVal(l)) => self
                .callback_bodies
                .get(l)
                .map_or(Updater::Unknown, |b| Updater::Functional(Arc::clone(b))),
            Some(Expr::Var(v)) if self.certified_fns.contains(v) => self
                .fn_bindings
                .get(v)
                .map_or(Updater::Unknown, |b| Updater::Functional(Arc::clone(b))),
            _ => Updater::Unknown,
        }
    }
}

/// The phase class a walk context assigns to writes found under it —
/// decided by HOW the walk entered the CFG (ADR-027 §2).
///
/// `Sync` is the only class with meaningful block ids. `Deferred` and
/// `Handler` come from callee summaries and are context-free: a timer defers
/// and a listener fires on events whatever phase registered them. Sync HOFs
/// (`arr.map(fn)`) run their argument in the ENCLOSING class, so they
/// propagate the current mode instead of assigning one. Everything unknown
/// is ⊤. The summaries axiomatize host semantics (a bare `setTimeout` is the
/// host timer unless a local binding shadows it, `.then` is a Promise, array
/// HOFs are `Array.prototype`'s) — the same footing as the
/// `addEventListener` subscription axiom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum WalkClass {
    /// Synchronous in the walked body (top level, or through a directly
    /// called local helper).
    Sync,
    /// Proved deferred by a callee summary.
    Deferred,
    /// A listener registered on an event target.
    Handler,
    /// An effect's returned cleanup function.
    Cleanup,
    /// Nested under an unknown callee — may run in any phase (⊤).
    Unknown,
}

/// One row per *call site*, in walk order.
///
/// This used to be a `HashMap<(Var, WalkClass), …>`, which collapsed two
/// `setCount(count + 1)` calls in one body into a single row. That collapse
/// was deliberate and documented, and it is what made the canonical stale-read
/// shape — two non-functional writes of one slot in one handler — inexpressible:
/// the relation could not say there were two. Per-site rows are a monotone
/// refinement (more rows, same slots), and every shipped consumer of the stored
/// relation is existential, so no query that matched before can stop matching.
/// The one consumer that wants the old shape, `collect_setter_calls`, collapses
/// explicitly at the end.
type Found = Vec<FoundSite>;

/// One raw call site the walk saw.
struct FoundSite {
    var: Var,
    class: WalkClass,
    span: Option<SourceRange>,
    /// `Some` only for `Sync` rows, where it is meaningful for dominance.
    /// NOT usable for reachability: a site inside a nested body records a
    /// block of *that* body's CFG, and `BlockId` is per-CFG.
    block_id: Option<BlockId>,
    /// The top-level block the walk descended from — always a block of the
    /// walked root, which makes it the only id that means anything in the
    /// region's CFG. Region membership reads it (a nested callback defined
    /// inside a spliced wrapper belongs to the wrapper), and so does same-tick
    /// reachability.
    prov_block: Option<BlockId>,
    /// The site sits inside a callback a synchronous higher-order function
    /// runs — `xs.forEach(x => setC(x))`. Such a write executes 0..N times in
    /// one tick, so it co-executes with itself without any CFG cycle.
    repeats: bool,
    /// Argument 0 of the call, classified where the walk still has it.
    updater: Updater,
}

/// `Array.prototype` HOFs that call their function argument synchronously —
/// the argument runs in the ENCLOSING phase.
///
/// Two readers: the setter walk's phase classification, and the JSX relations,
/// which descend a render-body callback only when this table says the render
/// body is what runs it (#125). One list, so a name added for one reader is
/// added for both.
pub(crate) const SYNC_HOF_METHODS: &[&str] = &[
    "map",
    "forEach",
    "filter",
    "reduce",
    "reduceRight",
    "find",
    "findIndex",
    "findLast",
    "findLastIndex",
    "some",
    "every",
    "flatMap",
    "sort",
];

impl<'a> SetterWalk<'a> {
    /// What class a function argument of a call to `fn_` takes, given the
    /// current `mode`. `WalkClass::Unknown` = no summary — the argument is ⊤.
    ///
    /// The registration summary ADR-027 §2 promised, read off the one registrar
    /// table (ADR-034 §2). It is not uniform: a timer or a promise continuation
    /// is `Deferred`, `addEventListener` is `Handler` because the DOM has no
    /// synchronous dispatch, and `subscribe`/`on`/`addListener` stay ⊤ because
    /// a store may emit to a new subscriber on the spot. Narrowing a row off ⊤
    /// is the one direction that can lose a finding, so it is allowed only
    /// where the timing is a contract rather than a name-table guess.
    ///
    /// A bare-global registrar is fail-closed: any local binding of the name
    /// anywhere disables its summary (`shadowed`).
    fn arg_class(&self, fn_: &Expr, mode: WalkClass) -> WalkClass {
        use crate::engine::registrations::Timing;
        if let Some((reg, _)) = crate::engine::registrations::match_registrar(fn_) {
            let shadowed = !reg.method_only
                && matches!(fn_.peel_ts(), Expr::Var(n) if self.shadowed.contains(n.as_str()));
            if !shadowed {
                return match reg.timing {
                    Timing::Deferred => WalkClass::Deferred,
                    Timing::Handler => WalkClass::Handler,
                    Timing::Unknown => WalkClass::Unknown,
                };
            }
        }
        match fn_ {
            Expr::FieldAccess { field, .. } if SYNC_HOF_METHODS.contains(&field.as_str()) => mode,
            _ => WalkClass::Unknown,
        }
    }

    /// `mode == Sync` → block IDs recorded are from the caller's CFG,
    /// meaningful for dominance; any other mode records `None`.
    /// `at_root` marks the walk's outermost CFG — where effect-body cleanup
    /// returns and reified listeners live. `prov` is the top-level block the
    /// walk descended from (`None` only at the root, where each block is its
    /// own provenance).
    fn cfg(
        &mut self,
        cfg: &'a CFG,
        depth: usize,
        found: &mut Found,
        mode: WalkClass,
        prov: Option<BlockId>,
    ) {
        let key = cfg as *const CFG as usize;
        if !self.walking.insert(key) {
            return;
        }
        let at_root = key == self.root;
        // A block that reaches itself is a loop body: everything in it runs
        // 0..N times per tick, whichever CFG the loop happens to live in — the
        // caller's, or a helper's the walk pulled in.
        let reach = Reachability::of(cfg);
        // Blocks this body reaches only across an `await` (#117, ADR-035). A
        // write there runs on a later turn of the event loop, so it is
        // `Deferred` however the walk entered the body — the same summary a
        // `.then` continuation gets, and for the same reason. Empty for a body
        // with no `await`, which is one edge scan to establish.
        let post_await = cfg.post_await_blocks();
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(cfg.entry);
        visited.insert(cfg.entry);

        while let Some(bid) = queue.pop_front() {
            // Only a sync walk can be deferred by an await: a body already
            // classified `Deferred`, `Handler` or ⊤ does not become more so.
            let mode = if mode == WalkClass::Sync && post_await.contains(&bid) {
                WalkClass::Deferred
            } else {
                mode
            };
            let block_id = if mode == WalkClass::Sync {
                Some(bid)
            } else {
                None
            };
            let prov_block = if at_root { Some(bid) } else { prov };
            let outer_repeating = self.repeating;
            self.repeating = outer_repeating || reach.reaches(bid, bid);
            if let Some(block) = cfg.blocks.get(&bid) {
                for stmt in &block.stmts {
                    self.stmt(stmt, block_id, depth, found, mode, at_root, prov_block);
                }
                match &block.term {
                    Terminator::Return(expr) => {
                        // An effect body's returned function is its cleanup
                        // (ADR-027 §2) — descend it as such, whether inline
                        // or bound to a variable. Without this the cleanup's
                        // writes are invisible to the relation.
                        if self.effect_body && at_root && mode == WalkClass::Sync {
                            let body = match expr {
                                Expr::FnLit { body_cfg, .. } => Some(&**body_cfg),
                                Expr::Var(name) => self.fn_bindings.get(name).map(|b| &**b),
                                _ => None,
                            };
                            if let Some(body) = body
                                && depth > 0
                            {
                                self.cfg(body, depth - 1, found, WalkClass::Cleanup, prov_block);
                            }
                        }
                        self.expr(
                            expr, None, block_id, depth, found, mode, at_root, prov_block,
                        );
                    }
                    Terminator::Branch { cond, .. } => {
                        self.expr(
                            cond, None, block_id, depth, found, mode, at_root, prov_block,
                        );
                    }
                    _ => {}
                }
                for succ in cfg.successors(bid) {
                    if visited.insert(succ) {
                        queue.push_back(succ);
                    }
                }
            }
            self.repeating = outer_repeating;
        }
        self.walking.remove(&key);
    }

    #[allow(clippy::too_many_arguments)]
    fn stmt(
        &mut self,
        stmt: &'a Stmt,
        block_id: Option<BlockId>,
        depth: usize,
        found: &mut Found,
        mode: WalkClass,
        at_root: bool,
        prov: Option<BlockId>,
    ) {
        // The containing statement's span is the witness for any call found
        // in its expression — rhs positions included (a quarter of corpus
        // setter calls sit in a Let/Assign rhs and used to report no range).
        let (expr, span) = match stmt {
            Stmt::ExprStmt(e, span) => (e, *span),
            Stmt::Let { rhs, span, .. } => (rhs, *span),
            Stmt::Assign { rhs, span, .. } => (rhs, *span),
            Stmt::MemberWrite { rhs, span, .. } => (rhs, *span),
        };
        self.expr(expr, span, block_id, depth, found, mode, at_root, prov);
    }

    #[allow(clippy::too_many_arguments)]
    fn expr(
        &mut self,
        expr: &'a Expr,
        stmt_span: Option<SourceRange>,
        block_id: Option<BlockId>,
        depth: usize,
        found: &mut Found,
        mode: WalkClass,
        at_root: bool,
        prov: Option<BlockId>,
    ) {
        if let Expr::Call { fn_, args } = expr {
            if let Expr::Var(name) = fn_.as_ref() {
                if self.setter_vars.contains(name) {
                    found.push(FoundSite {
                        var: name.clone(),
                        class: mode,
                        span: stmt_span,
                        block_id,
                        prov_block: prov,
                        repeats: self.repeating,
                        updater: self.updater_of(args),
                    });
                }
                // B6: direct call to a locally-bound function — its body runs
                // in the CURRENT mode, so sync-in-helper writes take this call
                // site's class and block id; writes the helper defers or
                // nests keep their own class (a `setTimeout` inside a sync
                // helper is not a sync write — lifting it was an
                // under-approximation).
                if depth > 0
                    && let Some(body) = self.fn_bindings.get(name)
                {
                    let mut inner = Found::new();
                    self.cfg(body, depth - 1, &mut inner, WalkClass::Sync, prov);
                    for site in inner {
                        // Inner rows' provenance is this call site: the local
                        // helper's definition is only reachable from code that
                        // shares its region (salted names stay region-local).
                        let sync = site.class == WalkClass::Sync;
                        found.push(FoundSite {
                            class: if sync { mode } else { site.class },
                            block_id: if sync { block_id } else { None },
                            prov_block: prov,
                            ..site
                        });
                    }
                }
            }
            // An immediately-invoked function expression runs NOW, at this
            // call site, in this mode — `(async () => { … })()` is how an
            // effect awaits, and it is the standard shape. The walk descended a
            // *named* local helper (B6 above) and not this one, so every write
            // inside an IIFE was invisible to the relation: a false negative,
            // and the reason #117's await split bought nothing on the shape it
            // was aimed at.
            if depth > 0
                && let Expr::FnLit { body_cfg, .. } = fn_.peel_ts()
            {
                let mut inner = Found::new();
                self.cfg(body_cfg, depth - 1, &mut inner, WalkClass::Sync, prov);
                for site in inner {
                    let sync = site.class == WalkClass::Sync;
                    found.push(FoundSite {
                        class: if sync { mode } else { site.class },
                        block_id: if sync { block_id } else { None },
                        prov_block: prov,
                        ..site
                    });
                }
            }
            // The listener `FnLit` of an effect-top-level `addEventListener`
            // is exactly what `extract_subscriptions` reified as a Handler
            // entry — its body is walked as its own Handler region, so
            // descending it here would double-count. Anywhere else the same
            // shape was NOT reified: classify the listener as Handler.
            let listener = expr.subscription_listener().is_some();
            let reified = listener && self.effect_body && at_root && mode == WalkClass::Sync;
            // A teardown takes the callback back, it does not call it
            // (ADR-034 §5). Descending it here gave every registered listener
            // a second ⊤ row — from the very cleanup that unregisters it —
            // and a ⊤ row satisfies `writer_phases includes <anything>`.
            if crate::engine::registrations::is_teardown(fn_) {
                return;
            }
            for (i, arg) in args.iter().enumerate() {
                if reified && i == 1 {
                    continue;
                }
                let class = if listener && i == 1 {
                    WalkClass::Handler
                } else {
                    self.arg_class(fn_, mode)
                };
                // A callback a sync HOF runs executes once per element: its
                // writes co-execute with themselves in a single tick, with no
                // CFG cycle to show for it.
                let outer_repeating = self.repeating;
                self.repeating = outer_repeating || self.is_sync_hof(fn_);
                match arg {
                    // Inline FnLit arg descend body, costs one depth level.
                    Expr::FnLit { body_cfg, .. } if depth > 0 => {
                        self.cfg(body_cfg, depth - 1, found, class, prov);
                    }
                    // B5: variable arg name resolution, no depth cost — so this is
                    // the arm that can cycle (`const tick = t => raf(tick)`); the
                    // `walking` stack is what terminates it.
                    Expr::Var(name) => {
                        if let Some(body) = self.fn_bindings.get(name) {
                            self.cfg(body, depth, found, class, prov);
                        }
                    }
                    _ => {}
                }
                self.repeating = outer_repeating;
            }
        }
    }
}

/// State slots that *may* ever be written: their setter variable (or an
/// alias of it) is referenced anywhere in the component — called, passed as
/// a prop, captured by a closure. A slot whose setter is never referenced
/// provably never changes (React state only moves through its setter), so a
/// capture of it can never go stale — sound to skip.
///
/// Shared by `stale-closure`, `frozen-initial-state` (which runs the same
/// proof on the *parent* component to decide whether a versioned prop can
/// actually change) and the `must_frozen_seed` query primitive.
pub(crate) fn may_written_slots(
    render_cfg: &CFG,
    hooks: &[HookEntry],
    setter_labels: &HashMap<Var, HookLabel>,
) -> HashSet<HookLabel> {
    fn scan_cfg(cfg: &CFG, used: &mut HashSet<Var>) {
        for block in cfg.blocks.values() {
            for stmt in &block.stmts {
                match stmt {
                    Stmt::Let { rhs, .. } | Stmt::Assign { rhs, .. } => {
                        collect_used_vars(rhs, used)
                    }
                    Stmt::MemberWrite { obj, key, rhs, .. } => {
                        collect_used_vars(obj, used);
                        if let crate::ir::stmt::MemberKey::Index(idx) = key {
                            collect_used_vars(idx, used);
                        }
                        collect_used_vars(rhs, used);
                    }
                    Stmt::ExprStmt(e, _) => collect_used_vars(e, used),
                }
            }
            match &block.term {
                Terminator::Return(e) | Terminator::Branch { cond: e, .. } => {
                    collect_used_vars(e, used)
                }
                _ => {}
            }
        }
    }
    let mut used: HashSet<Var> = HashSet::new();
    scan_cfg(render_cfg, &mut used);
    for hook in hooks {
        if let Some(body_cfg) = hook.body_cfg() {
            scan_cfg(body_cfg, &mut used);
            continue;
        }
        match hook {
            HookEntry::State { init, .. } | HookEntry::Ref { init, .. } => {
                collect_used_vars(init, &mut used)
            }
            HookEntry::Custom { args, .. } => {
                for a in args {
                    collect_used_vars(a, &mut used);
                }
            }
            _ => {}
        }
    }
    setter_labels
        .iter()
        .filter(|(v, _)| used.contains(*v))
        .map(|(_, l)| *l)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::expr::Prim;
    use crate::ir::types::ExprId;
    use crate::test_support::single_block_cfg;

    fn call(callee: &str, args: Vec<Expr>) -> Expr {
        Expr::Call {
            fn_: Box::new(Expr::Var(callee.to_string())),
            args,
        }
    }

    /// A self-referential local closure — `const tick = t => { setN(t); raf(tick) }`
    /// — used to make the "B5" variable-argument arm recurse forever, because it
    /// resolves the argument to its bound body without spending depth. The walk
    /// must terminate *and* still report the setter it contains.
    #[test]
    fn self_referential_callback_terminates_and_is_still_scanned() {
        let tick_body = single_block_cfg(vec![
            Stmt::ExprStmt(call("setN", vec![Expr::Lit(Prim::Int(1))]), None),
            Stmt::ExprStmt(call("raf", vec![Expr::Var("tick".to_string())]), None),
        ]);
        let cfg = single_block_cfg(vec![
            Stmt::Let {
                var: "tick".to_string(),
                rhs: Expr::FnLit {
                    id: ExprId(0),
                    params: vec!["t".to_string()],
                    body_cfg: Arc::new(tick_body),
                },
                span: None,
            },
            Stmt::ExprStmt(call("raf", vec![Expr::Var("tick".to_string())]), None),
        ]);

        let setters: HashSet<Var> = ["setN".to_string()].into_iter().collect();
        let found = collect_setter_calls(&cfg, &setters, 2);

        assert_eq!(
            found.iter().map(|c| c.var.as_str()).collect::<Vec<_>>(),
            vec!["setN"],
            "the cycle guard must not hide the setter inside the recursive closure"
        );
    }

    /// Mutual recursion between two bound closures — the same hazard one hop
    /// further out, which a self-reference-only guard would miss.
    #[test]
    fn mutually_recursive_callbacks_terminate() {
        let a_body = single_block_cfg(vec![Stmt::ExprStmt(
            call("raf", vec![Expr::Var("b".to_string())]),
            None,
        )]);
        let b_body = single_block_cfg(vec![
            Stmt::ExprStmt(call("setN", vec![]), None),
            Stmt::ExprStmt(call("raf", vec![Expr::Var("a".to_string())]), None),
        ]);
        let cfg = single_block_cfg(vec![
            Stmt::Let {
                var: "a".to_string(),
                rhs: Expr::FnLit {
                    id: ExprId(0),
                    params: vec![],
                    body_cfg: Arc::new(a_body),
                },
                span: None,
            },
            Stmt::Let {
                var: "b".to_string(),
                rhs: Expr::FnLit {
                    id: ExprId(1),
                    params: vec![],
                    body_cfg: Arc::new(b_body),
                },
                span: None,
            },
            Stmt::ExprStmt(call("raf", vec![Expr::Var("a".to_string())]), None),
        ]);

        let setters: HashSet<Var> = ["setN".to_string()].into_iter().collect();
        let found = collect_setter_calls(&cfg, &setters, 2);

        assert_eq!(
            found.iter().map(|c| c.var.as_str()).collect::<Vec<_>>(),
            vec!["setN"]
        );
    }
}
