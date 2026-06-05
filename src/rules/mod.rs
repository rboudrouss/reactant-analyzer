pub mod always_unstable_deps;
pub mod analysis_limit_info;
pub mod conditional_hook;
pub mod derived_state;
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
    engine::{AnalysisResult, ProgramAnalysisResult},
    ir::{
        SourceRange,
        cfg::{CFG, Terminator},
        expr::Expr,
        stmt::Stmt,
        types::{BlockId, HookLabel, Symbol, Var},
    },
};

/// Confidence level of a diagnostic finding.
///
/// Determined at the emission site by what the abstract domain proves:
/// - `Error`   — violation proven on ALL execution paths (e.g. setter call
///               in a block that dominates every render exit).
/// - `Warning` — violation possible but uncertain (conditional path, or
///               over-approximation inherent to the rule).
/// - `Info`    — known analysis limitation (widening triggered, depth cap,
///               etc.).  Hidden by default; shown with --info.
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

/// Post-pass analysis rule operating on a fully-computed `AnalysisResult`.
///
/// Rules are stateless; adding a new rule = new struct + `impl Rule`.
pub trait Rule {
    fn name(&self) -> &'static str;
    fn check(&self, result: &ProgramAnalysisResult, component: &Symbol) -> Vec<Diagnostic>;
}

/// A setter call found by `collect_setter_calls`.
#[derive(Debug, Clone)]
pub struct SetterCall {
    pub var: Var,
    pub span: Option<SourceRange>,
    /// Block ID in the TOP-LEVEL cfg where the call was found.
    /// `None` when the call is inside a nested `FnLit` body (separate CFG) —
    /// conditionality cannot be determined without cross-CFG dominance.
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

/// Like `collect_setter_calls` but merges `extra_fn_bindings` (e.g. from the
/// render CFG) so that B5 variable callbacks defined outside `cfg` are resolved.
/// Entries in `cfg` take precedence over `extra_fn_bindings`.
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
    // (span, block_id) — block_id is None when found inside a nested FnLit.
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
            // Direct ComponentSetter stab value.
            if let StateValue::ComponentSetter { component, label } = env.lookup(var) {
                result.insert(var.clone(), (component, label));
                continue;
            }
            // Loc pointing to a FnLit that captures a ComponentSetter
            // (e.g. the parent passed `() => setCount(0)` as a prop).
            if let Some(EnvVal::Loc(ids)) = env.lookup_env_val(var) {
                for id in ids {
                    if let Some(HeapValue::Fn { captured, .. }) = heap.get(id) {
                        for val in captured.values() {
                            if let StateValue::ComponentSetter { component, label } = val {
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

/// `top_level = true` → we're scanning the CFG passed by the public caller;
/// block IDs recorded in `found` belong to that CFG and are meaningful for
/// dominance checks.  `top_level = false` → we've descended into a nested
/// FnLit body (a separate CFG); block IDs are meaningless to the caller, so
/// we record `None`.
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
            // Terminators also carry expressions: scan Return value and Branch condition.
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
        // Also descend into Let rhs that are FnLit — direct setter call at top level of a closure.
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
            // B6: direct call to a locally-bound function — descend its body.
            // The call site IS in the top-level CFG, so propagate the outer block_id to
            // any setters found inside the body (enables Error severity when unconditional).
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
                // Inline FnLit arg — descend body (costs one depth level).
                // Nested CFG → top_level = false.
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
                // B5: variable arg — pointer-following, no depth cost.
                // Resolving Var("cb") to its FnLit is just name resolution,
                // not an extra call frame → same depth passes through.
                // Still a nested FnLit CFG → top_level = false.
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

/// Returns `true` when every expression in `deps` evaluates to an unstable abstract
/// value in the render-exit env.  An entirely-unstable deps array does not scope the
/// hook — it runs on every render, equivalent to having no deps argument at all.
///
/// An empty `deps` slice returns `false` (mount-only `[]` is not "all-unstable").
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
            &mut AnalysisCtx::null(&mut s, &mut m, &mut h),
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
