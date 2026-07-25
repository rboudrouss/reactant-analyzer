pub mod always_unstable_deps;
pub mod analysis_limit_info;
mod churn;
mod churn_graph;
pub mod conditional_hook;
pub mod derived_state;
pub mod diagnostic;
pub mod docs;
pub mod frozen_initial_state;
pub mod infinite_loop;
pub mod lazy_init;
pub mod missing_deps;
pub mod query;
pub mod redundant_set_state;
pub mod setter_in_render;
pub mod stale_closure;
pub mod state_mutation;
pub mod unnecessary_rerender;
pub mod widening_info;
pub mod witness;

pub use always_unstable_deps::AlwaysUnstableDeps;
pub use analysis_limit_info::AnalysisLimitInfo;
pub use conditional_hook::ConditionalHook;
pub use derived_state::DerivedState;
pub use diagnostic::{Diagnostic, Severity};
pub use docs::{RULE_DOCS, RuleDoc, rule_doc};
pub use frozen_initial_state::FrozenInitialState;
pub use infinite_loop::InfiniteLoop;
pub use lazy_init::LazyInit;
pub use missing_deps::MissingDeps;
pub use query::{
    Certified, ConditionalHookCall, DominatesAllExits, EffectCycleProof, ExitDominance,
    InitSetterCall, May, Motion, MovingFeeder, MustResult, OnAllPaths, Provenance, RuleConfig,
    RuleCtx, SameRefMutation, StabilityVerdict, classify_motion, may_change_of,
    must_dominates_all_exits, must_frozen_seed, must_init_calls_setter, must_on_all_paths,
    must_same_ref_mutation, must_setter_on_all_paths, stability_verdict_of,
};
pub use redundant_set_state::RedundantSetState;
pub use setter_in_render::SetterInRender;
pub use stale_closure::StaleClosure;
pub use state_mutation::StateMutation;
pub use unnecessary_rerender::UnnecessaryRerender;
pub use widening_info::WideningInfo;
pub use witness::{EffectClass, ResolveTarget, Step, ValueClass};

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::{
    domains::{
        AbstractEnv, AnalysisCtx, StateValue, StateValueTransfer, Transfer,
        stores::{EnvVal, Heap, HeapValue, MemoStore, StateStore},
    },
    engine::{AnalysisResult, HookKind, ProgramAnalysisResult},
    ir::{
        SourceRange,
        cfg::{CFG, Terminator},
        expr::Expr,
        stmt::Stmt,
        types::{BlockId, HookLabel, Symbol, Var},
    },
};

/// User-facing wording for an abstract value in a diagnostic message.
///
/// Rules must never print a domain value with `{:?}`: the lattice encoding
/// (`⊤`, kind unions like `number|string|ref(Unknown)`) is an implementation
/// detail. This is the rule/message boundary where abstract values map to
/// user language.
pub(crate) fn describe_value(val: &StateValue) -> &'static str {
    use crate::domains::Stability;
    match val.to_stability() {
        Stability::Bottom | Stability::Stable => "its value never changes between renders",
        Stability::PerRender => "it is recreated on every render",
        Stability::Versioned(_) | Stability::VersionedTop => {
            "its value changes when state is updated"
        }
        Stability::Unknown => "its value may change between renders",
    }
}

/// User-facing noun for a hook kind in a diagnostic message
/// (`useEffect` → "effect", `useMemo` → "memo", `useCallback` → "callback").
/// Falls back to "hook" for kinds without a distinct word.
pub(crate) fn hook_kind_word(kind: HookKind) -> &'static str {
    match kind {
        HookKind::Effect => "effect",
        HookKind::Memo => "memo",
        HookKind::Callback => "callback",
        _ => "hook",
    }
}

/// User-facing name for a state slot identified by its hook label. Prefers the
/// source variable it binds to (`` `count` ``); falls back to `state #N` when the
/// slot has no syntactic name (destructured indirectly, cross-component, …).
///
/// Messages must never print a bare internal `HookLabel` ("state 46") or a
/// lowering temp (`__obj_N` for `const [{ a, b }] = useState(...)`): both are
/// meaningless next to source.
pub(crate) fn state_slot_name(
    label: HookLabel,
    state_val_labels: &HashMap<Var, HookLabel>,
) -> String {
    state_val_labels
        .iter()
        .find(|(v, l)| **l == label && !v.starts_with("__"))
        .map(|(v, _)| format!("`{}`", crate::ir::source_name(v)))
        .unwrap_or_else(|| format!("state #{label}"))
}

/// One step of a diagnostic's witness chain (ADR-019).
///
/// `message` is the pre-rendered prose ([`Step::render`] is the single
/// rendering point — rules never format trace text); `step` is the typed
/// judgment JSON consumers read.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub message: String,
    /// The typed witness step this note carries.
    pub step: Step,
    /// Hook label this note points to, if any.
    pub hook_label: Option<HookLabel>,
    /// Source location this note points to, if available. Carries the
    /// [`crate::ir::FileId`] of the file it points into (may differ from the
    /// component's file after cross-file inlining).
    pub range: Option<SourceRange>,
}

/// A check that was *applicable* to a component and found nothing wrong —
/// surfaced under `--info` as positive assurance ("verified: …").
///
/// Distinct from an absent diagnostic: emptiness alone cannot tell "the
/// infinite-loop check ran and the component is safe" from "there was no
/// useState/useEffect for it to check". A `SafeCheck` records the former only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeCheck {
    /// Diagnostic name the assurance corresponds to (matches `RuleDoc::name`).
    pub rule: &'static str,
    /// Present-tense assurance, e.g. "no effect diverges into an infinite loop".
    pub message: &'static str,
}

/// `true` when `component` called at least one hook of `kind`. The applicability
/// primitive for `Rule::safe_check`.
pub(crate) fn has_hook_kind(
    result: &ProgramAnalysisResult,
    component: &Symbol,
    kind: HookKind,
) -> bool {
    result
        .components
        .get(component)
        .is_some_and(|c| c.hook_calls.iter().any(|h| h.kind == kind))
}

/// Every RHS assigned to each variable in `cfg` (a var may be written on
/// multiple paths — a lowered ternary/logical temp is). Used to chase a
/// call hidden behind a local binding (`const x = f(); useState(x)`), which a
/// syntactic linter cannot follow.
pub(crate) fn local_bindings(cfg: &CFG) -> HashMap<&str, Vec<&Expr>> {
    let mut map: HashMap<&str, Vec<&Expr>> = HashMap::new();
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            if let Stmt::Let { var, rhs, .. } | Stmt::Assign { var, rhs, .. } = stmt {
                map.entry(var.as_str()).or_default().push(rhs);
            }
        }
    }
    map
}

/// Like [`Expr::is_call_free`], but a `Var` bound to local temp(s) is call-free
/// only when *every* binding is — so a call hidden behind a branch temp or a
/// local `const` is seen. Vars with no local binding (params, props, state) are
/// plain values. Cycle-safe via `seen`.
pub(crate) fn arg_is_call_free(
    e: &Expr,
    bindings: &HashMap<&str, Vec<&Expr>>,
    seen: &mut HashSet<Var>,
) -> bool {
    match e {
        Expr::Call { .. } | Expr::CompApp { .. } | Expr::NativeElem { .. } => false,
        Expr::Var(v) => match bindings.get(v.as_str()) {
            Some(rhss) => {
                if !seen.insert(v.clone()) {
                    return true; // cycle: no new call evidence
                }
                rhss.iter().all(|r| arg_is_call_free(r, bindings, seen))
            }
            None => true,
        },
        Expr::Lit(_)
        | Expr::StateVal(_)
        | Expr::StateSetter(_)
        | Expr::MemoVal(_)
        | Expr::CallbackVal(_)
        | Expr::HookMarker(_)
        | Expr::SummaryVal(_)
        | Expr::FnLit { .. } => true,
        Expr::ObjectLit { fields, .. } => fields
            .iter()
            .all(|(_, v)| arg_is_call_free(v, bindings, seen)),
        Expr::ArrayLit { elems, .. } => elems.iter().all(|x| arg_is_call_free(x, bindings, seen)),
        Expr::FieldAccess { obj, .. } => arg_is_call_free(obj, bindings, seen),
        Expr::IndexAccess { arr, idx } => {
            arg_is_call_free(arr, bindings, seen) && arg_is_call_free(idx, bindings, seen)
        }
        Expr::BinOp { lhs, rhs, .. } => {
            arg_is_call_free(lhs, bindings, seen) && arg_is_call_free(rhs, bindings, seen)
        }
        Expr::UnaryOp { arg, .. } => arg_is_call_free(arg, bindings, seen),
        Expr::TSAnnotated(inner) => arg_is_call_free(inner, bindings, seen),
    }
}

/// Post-pass analysis rule operating on a fully-computed `AnalysisResult`.
///
/// Rules are stateless; adding a new rule = new struct + `impl Rule`.
pub trait Rule {
    fn name(&self) -> &'static str;
    fn check(&self, result: &ProgramAnalysisResult, component: &Symbol) -> Vec<Diagnostic>;

    /// When this rule is *applicable* to `component` but `check` found nothing,
    /// the positive assurance to surface under `--info`.
    ///
    /// Only consulted after `check` returned no diagnostics for the component,
    /// so implementations decide *applicability* only — they need not re-check.
    /// Default `None`: the rule opts out (e.g. Info-limitation rules, which have
    /// no "safe" state to report).
    fn safe_check(
        &self,
        _result: &ProgramAnalysisResult,
        _component: &Symbol,
    ) -> Option<SafeCheck> {
        None
    }
}

/// A setter call found by `collect_setter_calls`.
#[derive(Debug, Clone)]
pub struct SetterCall {
    pub var: Var,
    pub span: Option<SourceRange>,
    /// Block in the top-level CFG where the call was found.
    /// `None` when the call is inside a nested `FnLit` body dominance unknowable.
    pub block_id: Option<BlockId>,
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
    let mut found: HashMap<Var, (Option<SourceRange>, Option<BlockId>)> = HashMap::new();
    collect_setter_calls_inner(cfg, setter_vars, max_depth, &fn_bindings, &mut found, true);
    found
        .into_iter()
        .map(|(var, (span, block_id))| SetterCall {
            var,
            span,
            block_id,
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
pub(super) fn collect_component_setter_vars(
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

    let mut result: HashMap<Var, (Symbol, HookLabel)> = HashMap::new();
    for env in block_states.values() {
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
            if let Some(EnvVal::Loc(ids)) = env.lookup_env_val(var) {
                for id in ids {
                    if let Some(HeapValue::Fn { captured, .. }) = heap.get(id) {
                        for val in captured.values() {
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
pub(super) fn cross_component_setters(
    comp: &AnalysisResult<StateValue>,
    component: &Symbol,
) -> HashMap<Var, (Symbol, HookLabel)> {
    collect_component_setter_vars(&comp.render_cfg, &comp.block_states, &comp.heap)
        .into_iter()
        .filter(|(_, (parent_comp, _))| parent_comp != component)
        .collect()
}

/// Scan all Let stmts in `cfg` for `let X = FnLit{...}` and return X → body_cfg.
pub(super) fn collect_fn_bindings(cfg: &CFG) -> HashMap<Var, Arc<CFG>> {
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

/// `top_level = true` → block IDs recorded are from the caller's CFG, meaningful for dominance.
/// `top_level = false` → inside a nested FnLit; block IDs are `None`.
fn collect_setter_calls_inner(
    cfg: &CFG,
    setter_vars: &HashSet<Var>,
    depth: usize,
    fn_bindings: &HashMap<Var, Arc<CFG>>,
    found: &mut HashMap<Var, (Option<SourceRange>, Option<BlockId>)>,
    top_level: bool,
) {
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(cfg.entry);
    visited.insert(cfg.entry);

    while let Some(bid) = queue.pop_front() {
        let block_id = if top_level { Some(bid) } else { None };
        if let Some(block) = cfg.blocks.get(&bid) {
            for stmt in &block.stmts {
                check_stmt_for_setters(stmt, block_id, setter_vars, depth, fn_bindings, found);
            }
            match &block.term {
                Terminator::Return(expr) => {
                    check_expr_for_setters(
                        expr,
                        None,
                        block_id,
                        setter_vars,
                        depth,
                        fn_bindings,
                        found,
                    );
                }
                Terminator::Branch { cond, .. } => {
                    check_expr_for_setters(
                        cond,
                        None,
                        block_id,
                        setter_vars,
                        depth,
                        fn_bindings,
                        found,
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

fn check_stmt_for_setters(
    stmt: &Stmt,
    block_id: Option<BlockId>,
    setter_vars: &HashSet<Var>,
    depth: usize,
    fn_bindings: &HashMap<Var, Arc<CFG>>,
    found: &mut HashMap<Var, (Option<SourceRange>, Option<BlockId>)>,
) {
    let (expr, span) = match stmt {
        Stmt::ExprStmt(e, span) => (e, *span),
        // Also descend Let rhs FnLits.
        Stmt::Let { rhs, .. } => (rhs, None),
        Stmt::Assign { rhs, .. } => (rhs, None),
        Stmt::MemberWrite { rhs, .. } => (rhs, None),
    };
    check_expr_for_setters(expr, span, block_id, setter_vars, depth, fn_bindings, found);
}

fn check_expr_for_setters(
    expr: &Expr,
    stmt_span: Option<SourceRange>,
    block_id: Option<BlockId>,
    setter_vars: &HashSet<Var>,
    depth: usize,
    fn_bindings: &HashMap<Var, Arc<CFG>>,
    found: &mut HashMap<Var, (Option<SourceRange>, Option<BlockId>)>,
) {
    if let Expr::Call { fn_, args } = expr {
        if let Expr::Var(name) = fn_.as_ref() {
            if setter_vars.contains(name) {
                found.entry(name.clone()).or_insert((stmt_span, block_id));
            }
            // B6: direct call to a locally-bound function descend its body, propagate outer block_id.
            if depth > 0
                && let Some(body) = fn_bindings.get(name)
            {
                let mut inner: HashMap<Var, (Option<SourceRange>, Option<BlockId>)> =
                    HashMap::new();
                collect_setter_calls_inner(
                    body,
                    setter_vars,
                    depth - 1,
                    fn_bindings,
                    &mut inner,
                    false,
                );
                for (var, (span, _)) in inner {
                    found.entry(var).or_insert((span, block_id));
                }
            }
        }
        for arg in args {
            match arg {
                // Inline FnLit arg descend body, costs one depth level.
                Expr::FnLit { body_cfg, .. } if depth > 0 => {
                    collect_setter_calls_inner(
                        body_cfg,
                        setter_vars,
                        depth - 1,
                        fn_bindings,
                        found,
                        false,
                    );
                }
                // B5: variable arg name resolution, no depth cost.
                Expr::Var(name) => {
                    if let Some(body) = fn_bindings.get(name) {
                        collect_setter_calls_inner(
                            body,
                            setter_vars,
                            depth,
                            fn_bindings,
                            found,
                            false,
                        );
                    }
                }
                _ => {}
            }
        }
    }
}

/// Evaluate `expr` in `env` against *copies* of the given stores and `heap`,
/// through a null [`AnalysisCtx`]. The shared core of every post-fixpoint value
/// probe in the rules.
///
/// The state and memo stores are cloned internally and the heap is borrowed
/// mutably (callers pass a throwaway), so the caller's fixpoint result is never
/// disturbed. Each call site passes its OWN store bundle — the component's
/// converged stores (`comp.state_store` + a seeded `comp.heap.clone()`), or an
/// empty bundle (`StateStore::bottom()` + `Heap::new()`, for a mount-time init
/// eval). This primitive fixes none of them: it is the mechanical eval core,
/// not the store/heap policy (which is deliberately per-site — an empty vs a
/// converged heap are NOT interchangeable).
pub(crate) fn eval_in_stores(
    expr: &Expr,
    env: &AbstractEnv<StateValue>,
    component: &Symbol,
    state: &StateStore<StateValue>,
    memo: &MemoStore<StateValue>,
    heap: &mut Heap,
) -> StateValue {
    let mut s = state.clone();
    let mut m = memo.clone();
    StateValueTransfer.eval_expr(
        expr,
        env,
        &mut AnalysisCtx::null(component.clone(), &mut s, &mut m, heap),
    )
}

/// Evaluate an expression against a component's *converged* stores.
///
/// The convenience layer over [`eval_in_stores`] for the common case where the
/// state store, memo store and component name all come from one
/// [`AnalysisResult`]: it binds those three and leaves `env` and `heap` — the
/// two things that genuinely vary per call site — as explicit parameters.
///
/// The `heap` seed stays a caller argument on purpose: an empty
/// [`Heap::new()`] and the component's converged `heap.clone()` are NOT
/// interchangeable (the converged heap resolves a props-rooted `FieldAccess`
/// instead of degrading to ⊤), so each site keeps its own choice.
/// [`eval_in_stores`] remains the primitive for the mount-time site, which
/// evaluates against *empty* stores rather than a converged result.
pub(crate) trait ConvergedEval {
    fn eval_in(&self, env: &AbstractEnv<StateValue>, expr: &Expr, heap: &mut Heap) -> StateValue;
}

impl ConvergedEval for AnalysisResult<StateValue> {
    fn eval_in(&self, env: &AbstractEnv<StateValue>, expr: &Expr, heap: &mut Heap) -> StateValue {
        eval_in_stores(
            expr,
            env,
            &self.component,
            &self.state_store,
            &self.memo_store,
            heap,
        )
    }
}

/// `true` when **every** dep in `deps` is provably `Stable` in the render-exit
/// env — the only situation where a deps array genuinely gates an effect for
/// good: React re-runs an effect when **any** dep changed (OR semantics), so a
/// single dep that may change (⊤/`Versioned`/`PerRender`) keeps the effect
/// live no matter how stable its neighbours are. Empty `deps` returns `true`
/// (`[]` is mount-only — gated by definition; `infinite-loop` handles it
/// upstream anyway).
///
/// ADR-021 §5 + quantifier fix: the first cut of the ⊤-fix
/// (`all_deps_may_change`) still used the wrong quantifier — it skipped when
/// *one* dep was provably stable, silently dropping the mixed-deps case
/// (`[stableConst, topProp]`), the same FN family one stable dep away. The
/// sound gate quantifies ∀-stable, keyed on [`query::stability_verdict_of`]
/// (⊤ is a returned variant folded to the may side).
pub(super) fn all_deps_provably_stable(deps: &[Expr], result: &AnalysisResult<StateValue>) -> bool {
    let exit_env = result.exit_env();
    deps.iter().all(|dep| {
        let val = result.eval_in(&exit_env, dep, &mut Heap::new());
        query::stability_verdict_of(&val).is_stable()
    })
}

/// Instantiate all built-in rules.
pub fn all_rules() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(ConditionalHook),
        Box::new(MissingDeps),
        Box::new(AlwaysUnstableDeps),
        Box::new(LazyInit),
        Box::new(RedundantSetState),
        Box::new(UnnecessaryRerender),
        Box::new(SetterInRender),
        Box::new(StaleClosure),
        Box::new(StateMutation),
        Box::new(InfiniteLoop),
        Box::new(DerivedState),
        Box::new(FrozenInitialState),
        Box::new(WideningInfo),
        Box::new(AnalysisLimitInfo),
    ]
}
