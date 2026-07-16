pub mod always_unstable_deps;
pub mod analysis_limit_info;
mod churn_graph;
pub mod conditional_hook;
pub mod derived_state;
pub mod docs;
pub mod infinite_loop;
pub mod lazy_init;
pub mod missing_deps;
pub mod redundant_set_state;
pub mod setter_in_render;
pub mod unnecessary_rerender;
pub mod widening_info;

pub use always_unstable_deps::AlwaysUnstableDeps;
pub use analysis_limit_info::AnalysisLimitInfo;
pub use conditional_hook::ConditionalHook;
pub use derived_state::DerivedState;
pub use docs::{RULE_DOCS, RuleDoc, rule_doc};
pub use infinite_loop::InfiniteLoop;
pub use lazy_init::LazyInit;
pub use missing_deps::MissingDeps;
pub use redundant_set_state::RedundantSetState;
pub use setter_in_render::SetterInRender;
pub use unnecessary_rerender::UnnecessaryRerender;
pub use widening_info::WideningInfo;

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

/// User-facing name for a state slot identified by its hook label. Prefers the
/// source variable it binds to (`` `count` ``); falls back to `state #N` when the
/// slot has no syntactic name (destructured indirectly, cross-component, …).
///
/// Messages must never print a bare internal `HookLabel` ("state 46"): the
/// number is a post-inlining counter meaningless next to source.
pub(crate) fn state_slot_name(
    label: HookLabel,
    state_val_labels: &HashMap<Var, HookLabel>,
) -> String {
    state_val_labels
        .iter()
        .find(|(_, l)| **l == label)
        .map(|(v, _)| format!("`{v}`"))
        .unwrap_or_else(|| format!("state #{label}"))
}

/// Confidence level of a diagnostic.
///
/// - `Error`   violation on ALL execution paths.
/// - `Warning` possible but uncertain (conditional path or over-approx).
/// - `Info`    known analysis limitation (widening, depth cap). Hidden by default; show with --info.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Severity {
    Error,
    #[default]
    Warning,
    Info,
}

/// Secondary evidence item attached to a diagnostic.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub message: String,
    /// Hook label this note points to, if any.
    pub hook_label: Option<HookLabel>,
    /// Source location this note points to, if available.
    pub range: Option<SourceRange>,
}

/// Finding produced by a rule against the fixpoint analysis result.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub rule: &'static str,
    pub message: String,
    /// Hook label most directly involved, if any.
    pub hook_label: Option<HookLabel>,
    /// Variable name most directly involved, if any.
    pub var: Option<Var>,
    /// Source location of the primary finding, if available.
    pub range: Option<SourceRange>,
    /// Secondary evidence items explaining the causal chain.
    pub notes: Vec<Note>,
}

impl Diagnostic {
    pub fn new(rule: &'static str, message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::default(),
            rule,
            message: message.into(),
            hook_label: None,
            var: None,
            range: None,
            notes: vec![],
        }
    }

    pub fn with_severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    pub fn with_label(mut self, label: HookLabel) -> Self {
        self.hook_label = Some(label);
        self
    }

    pub fn with_var(mut self, var: impl Into<Var>) -> Self {
        self.var = Some(var.into());
        self
    }

    pub fn with_range(mut self, range: SourceRange) -> Self {
        self.range = Some(range);
        self
    }

    pub fn with_note(
        mut self,
        message: impl Into<String>,
        hook_label: Option<HookLabel>,
        range: Option<SourceRange>,
    ) -> Self {
        self.notes.push(Note {
            message: message.into(),
            hook_label,
            range,
        });
        self
    }
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
        Expr::TSAnnotated(inner, _) => arg_is_call_free(inner, bindings, seen),
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
                if let Stmt::Let {
                    var,
                    rhs: Expr::Var(src),
                    ..
                } = stmt
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

/// `true` when every dep in `deps` is unstable in the render-exit env.
/// Empty `deps` returns `false` (`[]` is mount-only, not all-unstable).
pub(super) fn all_deps_unstable(deps: &[Expr], result: &AnalysisResult<StateValue>) -> bool {
    if deps.is_empty() {
        return false;
    }
    let exit_env = result.exit_env();
    let transfer = StateValueTransfer;
    deps.iter().all(|dep| {
        let mut s: StateStore<StateValue> = result.state_store.clone();
        let mut m: MemoStore<StateValue> = result.memo_store.clone();
        let mut h = Heap::new();
        let val = transfer.eval_expr(
            dep,
            &exit_env,
            &mut AnalysisCtx::null(result.component.clone(), &mut s, &mut m, &mut h),
        );
        val.is_unstable()
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
        Box::new(InfiniteLoop),
        Box::new(DerivedState),
        Box::new(WideningInfo),
        Box::new(AnalysisLimitInfo),
    ]
}
